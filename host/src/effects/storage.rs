//! Local key/value storage confined to a host-selected Windows directory.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

use crate::rooted::{self, Directory, single_link};

pub(super) struct Local {
    root: PathBuf,
    layout: Vec<OsString>,
}

impl Local {
    pub(super) fn discover() -> Option<Self> {
        let mut path = std::ptr::null_mut();
        // SAFETY: Request the current user's known folder and take ownership of
        // the returned task-memory string, including on failure if supplied.
        let root = unsafe {
            let result =
                SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut path);
            if path.is_null() {
                return None;
            }
            let folder = (result >= 0)
                .then(|| PathBuf::from(OsString::from_wide(windows_core::PWSTR(path).as_wide())));
            CoTaskMemFree(path.cast());
            folder?
        };
        let executable = std::env::current_exe().ok()?;
        Some(Self {
            root,
            layout: vec![
                "roc-native-windows".into(),
                executable.file_stem()?.to_owned(),
                "storage".into(),
            ],
        })
    }

    pub(super) fn fixture(path: &std::path::Path) -> Option<Self> {
        Some(Self {
            root: path.parent()?.to_path_buf(),
            layout: vec![path.file_name()?.to_owned()],
        })
    }

    pub(super) fn read(&self, key: &str) -> io::Result<String> {
        let name = storage_name(key)?;
        let mut file = self.directory(false)?.read(OsStr::new(&name))?;
        single_link(&file)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(text)
    }

    pub(super) fn write(&self, key: &str, value: &str) -> io::Result<()> {
        let name = storage_name(key)?;
        let name = OsStr::new(&name);
        let directory = self.directory(true)?;
        match directory.read(name) {
            Ok(existing) => single_link(&existing)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        // Reserve a fresh sibling. Only the staged handle receives bytes; the
        // atomic replacement updates a single entry under the held parent.
        let mut random = [0; 16];
        if !crate::regional::random(&mut random) {
            return Err(io::Error::other(
                "could not name a local storage staging file",
            ));
        }
        let staging = format!(".{}.tmp", hex(&random));
        let mut file = directory.create_file(OsStr::new(&staging))?;
        let result = file
            .write_all(value.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| directory.replace(&file, name));
        if result.is_err() {
            let _ = rooted::remove(&file);
        }
        result
    }

    pub(super) fn remove(&self, key: &str) -> io::Result<()> {
        let name = storage_name(key)?;
        let opened = self
            .directory(false)
            .and_then(|directory| directory.open_for_removal(OsStr::new(&name)));
        match opened {
            Ok(file) => {
                single_link(&file)?;
                rooted::remove(&file)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn directory(&self, create: bool) -> io::Result<Directory> {
        let mut directory = Directory::open(&self.root)?;
        for part in &self.layout {
            directory = if create {
                directory.create_subdir(part)?
            } else {
                directory.subdir(part)?
            };
        }
        Ok(directory)
    }
}

fn storage_name(key: &str) -> io::Result<String> {
    let hash = super::files::sha256(key.as_bytes())
        .ok_or_else(|| io::Error::other("could not hash a local storage key"))?;
    Ok(hex(&hash))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("roc-local-storage-{name}-{}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn storage(&self) -> Local {
            Local::fixture(&self.0.join("storage")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_legacy_sha256_entries_then_replaces_and_removes_them() {
        let fixture = Fixture::new("legacy");
        let folder = fixture.0.join("storage");
        std::fs::create_dir(&folder).unwrap();
        let entry = folder.join("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        std::fs::write(&entry, "legacy").unwrap();

        let local = fixture.storage();
        assert_eq!(local.read("hello").unwrap(), "legacy");
        local.write("hello", "updated").unwrap();
        assert_eq!(local.read("hello").unwrap(), "updated");
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), "updated");
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 1);
        local.remove("hello").unwrap();
        local.remove("hello").unwrap();
        assert!(!entry.exists());
    }

    #[test]
    fn finds_the_system_known_folder_and_keeps_the_executable_namespace() {
        let local = Local::discover().unwrap();
        assert!(local.root.is_absolute());
        assert_eq!(local.layout[0], "roc-native-windows");
        let executable = std::env::current_exe().unwrap();
        assert_eq!(local.layout[1], executable.file_stem().unwrap());
        assert_eq!(local.layout[2], "storage");
    }

    #[test]
    fn failed_replacement_keeps_existing_bytes_and_cleans_its_stage() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let fixture = Fixture::new("busy");
        let local = fixture.storage();
        local.write("key", "original").unwrap();
        let folder = fixture.0.join("storage");
        let entry = folder.join(storage_name("key").unwrap());
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&entry)
            .unwrap();
        assert!(local.write("key", "replacement").is_err());
        assert_eq!(local.read("key").unwrap(), "original");
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 1);
        drop(locked);
        local.write("key", "replacement").unwrap();
    }

    #[test]
    fn creates_missing_directories_and_contains_arbitrary_keys() {
        let fixture = Fixture::new("create");
        let local = Local {
            root: fixture.0.clone(),
            layout: ["host", "app", "storage"].map(OsString::from).to_vec(),
        };
        let key = r"..\..\external.txt";
        local.remove(key).unwrap();
        local.write(key, "stored").unwrap();
        assert_eq!(local.read(key).unwrap(), "stored");
        assert!(!fixture.0.join("external.txt").exists());
    }

    #[test]
    fn refuses_hard_linked_entries_and_leaves_external_bytes_unchanged() {
        let fixture = Fixture::new("linked");
        let local = fixture.storage();
        let outer = fixture.0.join("outer.txt");
        std::fs::write(&outer, "outside").unwrap();
        let folder = fixture.0.join("storage");
        std::fs::create_dir(&folder).unwrap();
        let link = folder.join(storage_name("key").unwrap());
        std::fs::hard_link(&outer, &link).unwrap();

        assert!(local.read("key").is_err());
        assert!(local.write("key", "replacement").is_err());
        assert!(local.remove("key").is_err());
        assert_eq!(std::fs::read_to_string(outer).unwrap(), "outside");
        assert!(link.exists());
    }
}
