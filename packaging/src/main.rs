//! Build an MSI from a staged app. This tool runs on the packaging machine.

use embala_msi::{FileSpec, MsiArch, MsiSpec};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("roc-windows-msi: {error}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str =
    "usage: roc-windows-msi TARGET NAME ID VERSION PUBLISHER STAGE OUTPUT [DISPLAY_NAME]";

fn run() -> Result<()> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let [
        target,
        name,
        identifier,
        version,
        publisher,
        stage,
        output,
        optional @ ..,
    ] = args.as_slice()
    else {
        return Err(USAGE.into());
    };
    if optional.len() > 1 {
        return Err(USAGE.into());
    }
    let text = |arg: &OsStr| {
        arg.to_str()
            .map(str::to_owned)
            .ok_or("expected UTF-8 installer metadata")
    };
    let target = text(target)?;
    let arch = match target.as_str() {
        "x64mingw" => MsiArch::X86_64,
        "arm64mingw" => MsiArch::Aarch64,
        _ => return Err(format!("unknown Windows target {target}").into()),
    };
    let name = text(name)?;
    let identifier = text(identifier)?;
    let version = text(version)?;
    let publisher = text(publisher)?;
    let display_name = optional.first().map(|arg| text(arg)).transpose()?;
    let spec = spec(
        arch,
        &name,
        display_name.as_deref(),
        &identifier,
        &version,
        &publisher,
        Path::new(stage),
    )?;
    build(&spec, Path::new(output))
}

fn build(spec: &MsiSpec, output: &Path) -> Result<()> {
    embala_msi::build(spec, output)?;
    // ARM64 packages require Windows Installer schema 500. Embala 0.15
    // writes 200 for both architectures. Edit only the summary stream so
    // Embala's ordering of the installer tables is preserved.
    let file = fs::OpenOptions::new().read(true).write(true).open(output)?;
    let mut package = msi::Package::open(file)?;
    package.summary_info_mut().set_page_count(500);
    package.flush()?;
    Ok(())
}

fn spec(
    arch: MsiArch,
    name: &str,
    display_name: Option<&str>,
    identifier: &str,
    version: &str,
    publisher: &str,
    stage: &Path,
) -> Result<MsiSpec> {
    let display_name = display_name.unwrap_or(name);
    validate_metadata(name, display_name, identifier, version, publisher)?;
    let mut files = Vec::new();
    let main_executable = format!("{name}.exe");
    add_file(stage, &main_executable, &mut files)?;
    for notice in ["LICENSE", "THIRD_PARTY_LICENSES.md"] {
        if stage.join(notice).symlink_metadata().is_ok() {
            add_file(stage, notice, &mut files)?;
        }
    }
    match stage.join("assets").symlink_metadata() {
        Ok(meta) if meta.is_dir() => collect_assets(stage, "assets", &mut files)?,
        Ok(_) => return Err("expected assets to be a directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    files.sort_by(|left, right| left.dest.cmp(&right.dest));
    let mut hash = Sha256::new();
    for text in [
        env!("CARGO_PKG_VERSION"),
        arch.template(),
        name,
        identifier,
        version,
        publisher,
    ] {
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text);
    }
    if display_name != name {
        hash.update(b"display_name\0");
        hash.update((display_name.len() as u64).to_le_bytes());
        hash.update(display_name);
    }
    for file in &files {
        hash.update((file.dest.len() as u64).to_le_bytes());
        hash.update(&file.dest);
        let mut contents = File::open(&file.src)?;
        let mut file_hash = Sha256::new();
        io::copy(&mut contents, &mut HashWriter(&mut file_hash))?;
        hash.update(file_hash.finalize());
    }
    // Embala derives product and package IDs from the full version. A suffix
    // distinguishes architectures and changed payloads while Windows Installer
    // still compares the numeric version and keeps the same UpgradeCode.
    let version = format!(
        "{version}+{}.{:x}",
        arch.template().to_ascii_lowercase(),
        hash.finalize()
    );
    Ok(MsiSpec {
        name: name.into(),
        display_name: display_name.into(),
        version,
        identifier: identifier.into(),
        publisher: publisher.into(),
        description: display_name.into(),
        homepage: None,
        arch,
        main_executable,
        files,
        service: None,
        add_to_path: false,
        registry: Vec::new(),
    })
}

struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_metadata(
    name: &str,
    display_name: &str,
    identifier: &str,
    version: &str,
    publisher: &str,
) -> Result<()> {
    if !valid_segment(name) || name.len() > 64 || name.contains(['[', ']', '|']) {
        return Err(
            "the application name must be a Windows-safe ASCII name of at most 64 bytes".into(),
        );
    }
    if !valid_segment(display_name) || display_name.len() > 64 || display_name.contains(['[', ']'])
    {
        return Err("display_name must be a Windows-safe ASCII name of at most 64 bytes".into());
    }
    let labels = identifier.split('.').collect::<Vec<_>>();
    if identifier.len() > 96
        || labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(
            "--id must be a lowercase reverse-DNS identifier, such as org.example.my-app".into(),
        );
    }
    if publisher.trim().is_empty()
        || publisher.len() > 64
        || !publisher
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return Err("--publisher must be a nonempty ASCII name of at most 64 bytes".into());
    }
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().zip([255, 255, 65_535]).any(|(part, max)| {
            !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
                || part.parse::<u32>().map_or(true, |number| number > max)
        })
    {
        return Err(
            "--app-version must be MAJOR.MINOR.PATCH within MSI limits (255.255.65535)".into(),
        );
    }
    Ok(())
}

fn valid_segment(segment: &str) -> bool {
    let device = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "AUX" | "CON" | "NUL" | "PRN")
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && matches!(device.as_bytes()[3], b'1'..=b'9'));
    !segment.is_empty()
        && !segment.ends_with(['.', ' '])
        && !reserved
        && segment.bytes().all(|byte| {
            (byte.is_ascii_graphic() || byte == b' ') && !b"<>:\"/\\|?*".contains(&byte)
        })
}

fn add_file(root: &Path, dest: &str, files: &mut Vec<FileSpec>) -> Result<()> {
    let src = root.join(dest);
    if !src.symlink_metadata()?.is_file() {
        return Err(format!("expected a regular file at {}", src.display()).into());
    }
    files.push(FileSpec {
        src,
        dest: dest.into(),
    });
    Ok(())
}

fn collect_assets(root: &Path, prefix: &str, files: &mut Vec<FileSpec>) -> Result<()> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root.join(prefix))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .filter(|name| valid_segment(name))
            .ok_or_else(|| {
                format!(
                    "MSI asset names must be Windows-safe ASCII: {}",
                    entry.path().display()
                )
            })?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(format!("assets have the same Windows name in {prefix}: {name}").into());
        }
        let dest = format!("{prefix}/{name}");
        if entry.file_type()?.is_dir() {
            collect_assets(root, &dest, files)?;
        } else {
            add_file(root, &dest, files)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use msi::{Package, Select};
    use std::io::{Cursor, Read};

    #[test]
    fn installers_embed_assets_and_use_distinct_package_codes() {
        let dir = std::env::temp_dir().join(format!(
            "roc-windows-msi-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let cleanup = Cleanup(dir);
        let dir = &cleanup.0;
        fs::create_dir_all(dir.join("assets/images")).unwrap();
        fs::write(dir.join("demo.exe"), b"demo program").unwrap();
        fs::write(dir.join("assets/images/sample.bmp"), b"demo asset").unwrap();
        fs::write(dir.join("LICENSE"), b"demo license").unwrap();
        let default_name = spec(
            MsiArch::X86_64,
            "demo",
            None,
            "org.example.demo",
            "1.2.3",
            "Example",
            dir,
        )
        .unwrap();
        assert_eq!(default_name.display_name, "demo");
        assert_eq!(default_name.main_executable, "demo.exe");
        let mut identities = Vec::new();
        for arch in [MsiArch::X86_64, MsiArch::Aarch64] {
            let input = spec(
                arch,
                "demo",
                Some("Reading List"),
                "org.example.demo",
                "1.2.3",
                "Example",
                dir,
            )
            .unwrap();
            let output = dir.join(format!("{}.msi", arch.template()));
            build(&input, &output).unwrap();
            let first = fs::read(&output).unwrap();
            build(&input, &output).unwrap();
            assert_eq!(first, fs::read(&output).unwrap());

            let mut package = Package::open(Cursor::new(first)).unwrap();
            let summary = package.summary_info();
            assert_eq!(summary.arch(), Some(arch.template()));
            assert_eq!(summary.page_count(), Some(500));
            let package_code = summary.uuid().unwrap();
            let properties = package
                .select_rows(Select::table("Property"))
                .unwrap()
                .map(|row| {
                    (
                        row[0].as_str().unwrap().to_owned(),
                        row[1].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(properties["ProductVersion"], "1.2.3");
            assert_eq!(properties["Manufacturer"], "Example");
            assert_eq!(properties["ProductName"], "Reading List");
            let shortcuts = package
                .select_rows(Select::table("Shortcut"))
                .unwrap()
                .collect::<Vec<_>>();
            let [shortcut] = shortcuts.as_slice() else {
                panic!("expected one Start menu shortcut")
            };
            assert_eq!(shortcut[1].as_str(), Some("ProgramMenuFolder"));
            assert!(shortcut[2].as_str().unwrap().ends_with("|Reading List"));
            assert_eq!(shortcut[8], msi::Value::Null); // Default icon comes from the target.
            assert_eq!(shortcut[11].as_str(), Some("INSTALLDIR"));
            let target = shortcut[4].as_str().unwrap().to_owned();
            let files = package
                .select_rows(Select::table("File"))
                .unwrap()
                .map(|row| {
                    (
                        row[0].as_str().unwrap().to_owned(),
                        row[2].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            let executable = files
                .iter()
                .find(|(_, name)| name.ends_with("|demo.exe"))
                .unwrap();
            assert_eq!(target, format!("[#{}]", executable.0));
            let actions = package
                .select_rows(Select::table("InstallExecuteSequence"))
                .unwrap()
                .map(|row| {
                    (
                        row[0].as_str().unwrap().to_owned(),
                        row[2].as_int().unwrap(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            assert!(actions["RemoveExistingProducts"] < actions["CreateShortcuts"]);
            assert!(actions["RemoveShortcuts"] < actions["RemoveFiles"]);
            let msi_file = files
                .iter()
                .find(|(_, name)| name.ends_with("sample.bmp"))
                .unwrap();
            let stream = package
                .select_rows(Select::table("Media"))
                .unwrap()
                .next()
                .unwrap()[3]
                .as_str()
                .unwrap()
                .trim_start_matches('#')
                .to_owned();
            let mut cabinet = Vec::new();
            package
                .read_stream(&stream)
                .unwrap()
                .read_to_end(&mut cabinet)
                .unwrap();
            let mut cabinet = cab::Cabinet::new(Cursor::new(cabinet)).unwrap();
            let mut payload = Vec::new();
            cabinet
                .read_file(&msi_file.0)
                .unwrap()
                .read_to_end(&mut payload)
                .unwrap();
            assert_eq!(payload, b"demo asset");
            assert!(files.iter().any(|(_, name)| name.ends_with("LICENSE")));
            identities.push((
                package_code,
                properties["ProductCode"].clone(),
                properties["UpgradeCode"].clone(),
            ));
        }
        assert_ne!(identities[0].0, identities[1].0);
        assert_ne!(identities[0].1, identities[1].1);
        assert_eq!(identities[0].2, identities[1].2);
    }

    #[test]
    fn validates_the_display_name_before_packaging() {
        for name in ["", "Invalid/Name", "Unclosed [", "COM1", "Non-ASCII é"] {
            assert!(
                validate_metadata("demo", name, "org.example.demo", "1.2.3", "Example").is_err()
            );
        }
        assert!(
            validate_metadata(
                "demo",
                "Reading List",
                "org.example.demo",
                "1.2.3",
                "Example"
            )
            .is_ok()
        );
    }

    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
