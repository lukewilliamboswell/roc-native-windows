#!/usr/bin/env roc-stable
app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	roc: "nightly-2026-11-26-890d6c8",
	weaver: "https://github.com/lukewilliamboswell/weaver/releases/download/0.10.0/7CdYzjBAJmprmnK45oTpmH6BXt76ZASN2Ck5uZQEoWsK.tar.zst",
}

import cli.Cmd
import cli.Env
import cli.OsStr
import cli.Path
import cli.Stderr
import cli.Stdout
import cli.System
import weaver.Cli
import weaver.Opt
import weaver.Param

release_targets = [
	{ system: "macos-arm64", rust: "aarch64-apple-darwin", zig: "", linker: "", ext: "" },
	{ system: "macos-x64", rust: "x86_64-apple-darwin", zig: "", linker: "", ext: "" },
	{ system: "windows-arm64", rust: "aarch64-pc-windows-gnullvm", zig: "aarch64-windows-gnu", linker: "CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER", ext: ".exe" },
	{ system: "windows-x64", rust: "x86_64-pc-windows-gnullvm", zig: "x86_64-windows-gnu", linker: "CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER", ext: ".exe" },
	{ system: "linux-arm64", rust: "aarch64-unknown-linux-musl", zig: "", linker: "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER", ext: "" },
	{ system: "linux-x64", rust: "x86_64-unknown-linux-musl", zig: "", linker: "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", ext: "" },
]

release_parser : Cli.CliParser({ local : Bool, packagers : Str, args : List(Str) })
release_parser =
	Cli.assert_valid(
		Cli.finish(
			{
				local: Opt.flag({ short: "", long: "local", help: "Stage for HTTP testing at http://127.0.0.1:8000." }),
				packagers: Opt.str({ short: "", long: "packagers", help: "Directory of six prebuilt release MSI packagers.", default: Value("") }),
				args: Param.str_list({ name: "release", help: "VERSION PLATFORM_BUNDLE BASE_URL OUTPUT_DIR, or VERSION PLATFORM_BUNDLE OUTPUT_DIR with --local." }),
			}.Cli,
			{
				name: "release",
				description: "Assemble the complete release into a local directory.",
				version: "development",
				authors: [],
				text_style: Plain,
			},
		),
	)

main! = |raw_args| {
	args = if (raw_args.first() == Ok(OsStr.from_str("--"))) raw_args.drop_first(1) else raw_args
	options = match Cli.parse_or_display_message(release_parser, args, OsStr.to_raw) {
		Ok(parsed) => parsed
		Err(Help(message)) | Err(Version(message)) => return Stdout.line!(message)
		Err(InvalidUsage(message)) => {
			Stderr.line!(message)?
			return Err(Exit(2))
		}
	}
	result = match (options.local, options.args) {
		(Bool.True, [version, archive, out]) => release!(version, archive, "http://127.0.0.1:8000", out, options.packagers)
		(Bool.False, [version, archive, base, out]) => release!(version, archive, base.drop_suffix("/"), out, options.packagers)
		_ => Err("Usage: blueprint run release -- [--packagers DIR] VERSION PLATFORM_BUNDLE BASE_URL OUTPUT_DIR\n       blueprint run release -- --local [--packagers DIR] VERSION PLATFORM_BUNDLE OUTPUT_DIR")
	}
	match result {
		Ok({}) => Ok({})
		Err(problem) => {
			Stderr.line!(problem)?
			Err(Exit(1))
		}
	}
}

release! = |version, archive, base, output, packagers| {
	if (!valid_version(version)) return Err("Use a release version such as 0.1.0 or 0.1.0-rc.1")
	if (!valid_url(base)) return Err("Use an HTTPS base URL or an HTTP loopback URL for testing")
	if packagers.is_empty() {
		target = System.target!()
		match target.os() {
			MacOS(_) => {}
			_ => return Err("Use --packagers DIR with the six release packagers staged on macOS")
		}
	}
	archive_name = Path.filename(Path.utf8(archive)).map_ok(Path.display) ? |_| "Missing platform archive file name"
	if (!bundle_name(archive_name)) return Err("Use the content-addressed platform archive produced by roc bundle")
	out = absolute!(output)?
	if (Path.exists!(Path.utf8(out)) ?? Bool.False) return Err("Release output already exists: ${out}")
	parent = Path.parent(Path.utf8(out)) ? |_| "Release output must have a parent directory"
	mkdir!(parent.display())?
	staging = Env.create_temp_dir_in!(parent, ".windows-release-") ? |error| "Could not create the release staging directory: ${Str.inspect(error)}"
	result = assemble!(staging, version, base, archive, archive_name, out, packagers)
	Path.delete_all!(staging) ?? {}
	result
}

valid_url = |base|
	(base.starts_with("https://") or base.starts_with("http://127.0.0.1:") or base.starts_with("http://localhost:")) and
		base.to_utf8().all(|byte| byte > 32 and byte < 127 and !['"', '\\', '$', '{', '}', '?', '#'].contains(byte))

valid_version = |version| {
	parts = version.drop_prefix("v").split_on("-")
	core = parts.first() ?? ""
	tail = Str.join_with(parts.drop_first(1), "-")
	match core.split_on(".") {
		[major, minor, patch] =>
			[major, minor, patch].all(|field| !field.is_empty() and field.to_utf8().all(is_digit)) and
				[major, minor, patch].any(|field| field.to_utf8().any(|byte| byte != '0')) and
					(parts.len() == 1 or tail.split_on(".").all(|field| !field.is_empty() and field.to_utf8().all(|byte| is_alphanumeric(byte) or byte == '-')))
		_ => Bool.False
	}
}

is_digit = |byte| byte >= '0' and byte <= '9'

is_alphanumeric = |byte| is_digit(byte) or (byte >= 'a' and byte <= 'z') or (byte >= 'A' and byte <= 'Z')

bundle_name = |name|
	name.ends_with(".tar.zst") and [43, 44].contains(name.drop_suffix(".tar.zst").count_utf8_bytes()) and
		name.drop_suffix(".tar.zst").to_utf8().all(is_alphanumeric)

assemble! = |staging, version, base, archive, archive_name, out, packagers| {
	work = staging.display()
	assets = "${work}/assets"
	mkdir!(assets)?
	# The release tag can already be a dotted version. Roc accepts one
	# MAJOR.MINOR.PATCH occurrence before the content hash in a URL.
	bundle_version = version.replace_each(".", "_")
	platform_name = "roc-native-windows-${bundle_version}-${archive_name}"
	launcher_name = "windows-${version}.roc"
	copy!(archive, "${assets}/${platform_name}")?
	var $packager_entries = []
	for target in release_targets {
		name = "roc-windows-msi-${version}-${target.system}${target.ext}"
		destination = "${assets}/${name}"
		if packagers.is_empty() {
			build_packager!(target, work)?
			copy!("packaging/target/${target.rust}/release/roc-windows-msi${target.ext}", destination)?
		} else {
			copy!("${packagers}/${name}", destination)?
		}
		bytes = Path.read_bytes!(Path.utf8(destination)) ? |error| "Could not read ${destination}: ${Str.inspect(error)}"
		sha256 = Crypto.SHA256.hash(bytes).to_hex()
		$packager_entries = $packager_entries.append("{ system: \"${target.system}\", url: \"${base}/${name}\", sha256: \"${sha256}\" }")
	}
	copy!("LICENSE", "${assets}/LICENSE-${version}.txt")?
	copy!("THIRD_PARTY_LICENSES.md", "${assets}/THIRD_PARTY_LICENSES-${version}.md")?
	source = Path.read_utf8!(Path.utf8("cli/windows.roc")) ? |error| "Could not read cli/windows.roc: ${Str.inspect(error)}"
	platform_url = "${base}/${platform_name}"
	launcher_url = "${base}/${launcher_name}"
	scaffold = embed_scaffold!()?
	packager_source = "release_packagers = [${Str.join_with($packager_entries, ", ")}]"
	version_source = "release_version = \"${version}\""
	lines = source.split_on("\n").map(
		|line| match line {
			"release_version = \"development\"" => version_source
			"release_platform = \"\"" => "release_platform = \"${platform_url}\""
			"release_launcher = \"\"" => "release_launcher = \"${launcher_url}\""
			"release_packagers = []" => packager_source
			"release_scaffold = []" => scaffold
			other => other
		},
	)
	text = Str.join_with(lines, "\n")
	if !text.contains(version_source) or !text.contains(platform_url) or !text.contains(launcher_url) or !text.contains(packager_source) or !text.contains(scaffold) {
		return Err("Could not stamp the release assets into windows.roc")
	}
	entry = "${work}/main.roc"
	write!(entry, text)?
	roc = Env.var_str!("ROC_WINDOWS_ROC") ?? "roc"
	step!(cmd(roc, ["fmt", entry]), "format the release launcher")?
	step!(cmd(roc, ["test", entry]), "check the release launcher")?
	launcher = Path.read_utf8!(Path.utf8(entry)) ? |error| "Could not read the stamped launcher: ${Str.inspect(error)}"
	# Keep this bound aligned with the launcher download in cli/windows.roc.
	if launcher.count_utf8_bytes() > 1024 * 1024 {
		return Err("The generated windows.roc exceeds 1 MiB. Reduce the embedded scaffold before staging the release.")
	}
	copy!(entry, "${assets}/${launcher_name}")?
	digest = Crypto.SHA256.hash(launcher.to_utf8()).to_hex()
	tool_source = Str.join_with(launcher.split_on("\n").map(|line| if (line == "release_launcher_sha256 = \"\"") "release_launcher_sha256 = \"${digest}\"" else line), "\n")
	if (!tool_source.contains(digest)) return Err("Could not stamp the launcher checksum into the tool bundle")
	write!(entry, tool_source)?
	step!(cmd(roc, ["bundle", "--output-dir", assets, "main.roc"]).cwd(staging), "bundle the URL-runnable release tool")?
	entries = Path.list!(Path.utf8(assets)) ? |error| "Could not list staged release assets: ${Str.inspect(error)}"
	bundles = entries.keep_oks(|path| Path.filename(path).map_ok(Path.display)).keep_if(bundle_name)
	hashed_name = match bundles {
		[hashed] => hashed
		_ => return Err("Expected one content-addressed CLI archive from roc bundle")
	}
	tool_name = "windows-${bundle_version}-${hashed_name}"
	Path.rename!(Path.utf8("${assets}/${hashed_name}"), Path.utf8("${assets}/${tool_name}")) ? |error| "Could not name the CLI bundle: ${Str.inspect(error)}"
	Path.rename!(Path.utf8(assets), Path.utf8(out)) ? |error| "Could not publish the local release directory: ${Str.inspect(error)}"
	Stdout.line!("Release assets staged in ${out} with URLs under ${base}.").map_err(|error| Str.inspect(error))
}

embed_scaffold! = || {
	root = "examples/starter"
	manifest = Path.read_utf8!(Path.utf8("${root}/scaffold.txt")) ? |error| "Could not read the starter manifest: ${Str.inspect(error)}"
	var $entries = []
	for path in manifest.split_on("\n").map(Str.trim).drop_if(Str.is_empty) {
		bytes = Path.read_bytes!(Path.utf8("${root}/${path}")) ? |error| "Could not read starter file ${path}: ${Str.inspect(error)}"
		values = Str.join_with(bytes.map(U8.to_str), ", ")
		$entries = $entries.append("{ path: ${Str.inspect(path)}, bytes: [${values}] }")
	}
	if ($entries.is_empty()) return Err("The starter manifest is empty")
	Ok("release_scaffold = [${Str.join_with($entries, ", ")}]")
}

build_packager! = |target, work| {
	build = cmd("cargo", ["build", "--release", "--locked", "--manifest-path", "packaging/Cargo.toml", "--target", target.rust])
	if target.rust.ends_with("-linux-musl") {
		host_lib = cmd("rustc", ["--print", "target-libdir"]).capture!() ? |error| "Could not find the Rust host libraries: ${Str.inspect(error)}"
		linker = "${host_lib.trim()}/../bin/rust-lld"
		linux = build.env(OsStr.from_str(target.linker), OsStr.from_str(linker)).env("RUSTFLAGS", "-C linker-flavor=ld.lld")
		step!(linux, "cross-build MSI packager for ${target.system}")
	} else if target.zig.is_empty() {
		step!(build, "build MSI packager for ${target.system}")
	} else {
		linker = "${work}/link-${target.system}"
		write!(linker, linker_script(target.zig))?
		step!(cmd("/bin/chmod", ["700", linker]), "prepare the linker for ${target.system}")?
		full = absolute!(linker)?
		step!(build.env(OsStr.from_str(target.linker), OsStr.from_str(full)), "cross-build MSI packager for ${target.system}")
	}
}

# Rust's Windows GNU driver names the system libraries as if Clang were
# invoking a separately installed MinGW. Let Zig provide its bundled CRT.
linker_script = |triple|
	\\#!/bin/bash
	\\args=()
	\\for arg in "$@"; do
	\\  case "$arg" in
	\\    -nolibc|-lmsvcrt|-lmingwex|-lmingw32|--unwindlib=none) ;;
	\\    *) args+=("$arg") ;;
	\\  esac
	\\done
	\\exec zig cc -target ${triple} -lunwind "${"$"}{args[@]}"
	\\

cmd = |program, args| Cmd.new(OsStr.from_str(program)).args(args.map(OsStr.from_str))

step! = |command, label| {
	_ = Stdout.line!("+ ${label}")
	ran = command.inherit_stdio().run!() ? |error| "${label}: ${Str.inspect(error)}"
	if (ran.status == Exited(0)) Ok({}) else Err("${label}: ${Str.inspect(ran.status)}")
}

absolute! = |path| Path.absolute!(Path.utf8(path)).map_ok(Path.display).map_err(|error| "Could not resolve ${path}: ${Str.inspect(error)}")

mkdir! = |path| Path.create_all!(Path.utf8(path)).map_err(|error| "Could not create ${path}: ${Str.inspect(error)}")

copy! = |src, dst| Path.copy!(Path.utf8(src), Path.utf8(dst)).map_ok(|_| {}).map_err(|error| "Could not copy ${src}: ${Str.inspect(error)}")

write! = |path, text| Path.write_utf8!(Path.utf8(path), text).map_err(|error| "Could not write ${path}: ${Str.inspect(error)}")

expect valid_url("https://example.com/releases/v0.1.0") and valid_url("http://127.0.0.1:8000") and !valid_url("http://example.com")
expect bundle_name("G8hs34dPmXNywQPetPRm6j25gtaAG7gDrZ1AUk7qefnX.tar.zst") and !bundle_name("platform.tar.zst")
expect ["0.1.0", "v1.2.3", "0.1.0-rc.1"].all(valid_version)
expect ["", "latest", "0.0.0", "0.1.0/rc", "0.1.0-", "0.1.0-rc..1"].all(|version| !valid_version(version))
