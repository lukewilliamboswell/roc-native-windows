#!/usr/bin/env roc-stable
app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	roc: "nightly-2026-11-26-890d6c8",
}

import cli.Cmd
import cli.Env
import cli.OsStr
import cli.Path
import cli.Stderr
import cli.Stdout

release_targets = [
	{ system: "macos-arm64", rust: "aarch64-apple-darwin", zig: "", linker: "", ext: "" },
	{ system: "macos-x64", rust: "x86_64-apple-darwin", zig: "", linker: "", ext: "" },
	{ system: "windows-arm64", rust: "aarch64-pc-windows-gnullvm", zig: "aarch64-windows-gnu", linker: "CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER", ext: ".exe" },
	{ system: "windows-x64", rust: "x86_64-pc-windows-gnullvm", zig: "x86_64-windows-gnu", linker: "CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER", ext: ".exe" },
	{ system: "linux-arm64", rust: "aarch64-unknown-linux-musl", zig: "", linker: "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER", ext: "" },
	{ system: "linux-x64", rust: "x86_64-unknown-linux-musl", zig: "", linker: "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", ext: "" },
]

main! = |args| {
	values = args.map(OsStr.display).drop_if(|arg| arg == "--")
	result = match values {
		[archive, base, out] => release!(archive, base.drop_suffix("/"), out)
		_ => Err("Usage: blueprint run release -- PLATFORM_BUNDLE BASE_URL OUTPUT_DIR")
	}
	match result {
		Ok({}) => Ok({})
		Err(problem) => {
			Stderr.line!(problem)?
			Err(Exit(1))
		}
	}
}

release! = |archive, base, output| {
	if (!valid_url(base)) return Err("Use an HTTPS base URL or an HTTP loopback URL for testing")
	archive_name = Path.filename(Path.utf8(archive)).map_ok(Path.display) ? |_| "Missing platform archive file name"
	if (!bundle_name(archive_name)) return Err("Use the content-addressed platform archive produced by roc bundle")
	out = absolute!(output)?
	if (Path.exists!(Path.utf8(out)) ?? Bool.False) return Err("Release output already exists: ${out}")
	parent = Path.parent(Path.utf8(out)) ? |_| "Release output must have a parent directory"
	mkdir!(parent.display())?
	staging = Env.create_temp_dir_in!(parent, ".windows-release-") ? |error| "Could not create the release staging directory: ${Str.inspect(error)}"
	result = assemble!(staging, base, archive, archive_name, out)
	Path.delete_all!(staging) ?? {}
	result
}

valid_url = |base|
	(base.starts_with("https://") or base.starts_with("http://127.0.0.1:") or base.starts_with("http://localhost:")) and
		base.to_utf8().all(|byte| byte > 32 and byte < 127 and !['"', '\\', '$', '{', '}', '?', '#'].contains(byte))

bundle_name = |name|
	name.ends_with(".tar.zst") and [43, 44].contains(name.drop_suffix(".tar.zst").count_utf8_bytes()) and
		name.drop_suffix(".tar.zst").to_utf8().all(|byte| (byte >= '0' and byte <= '9') or (byte >= 'a' and byte <= 'z') or (byte >= 'A' and byte <= 'Z'))

assemble! = |staging, base, archive, archive_name, out| {
	work = staging.display()
	assets = "${work}/assets"
	mkdir!(assets)?
	copy!(archive, "${assets}/${archive_name}")?
	var $packagers = []
	for target in release_targets {
		asset = packager!(target, work, assets)?
		$packagers = $packagers.append("{ system: \"${target.system}\", url: \"${base}/${asset.name}\", sha256: \"${asset.sha256}\" }")
	}
	copy!("LICENSE", "${assets}/LICENSE")?
	copy!("THIRD_PARTY_LICENSES.md", "${assets}/THIRD_PARTY_LICENSES.md")?
	source = Path.read_utf8!(Path.utf8("cli/windows.roc")) ? |error| "Could not read cli/windows.roc: ${Str.inspect(error)}"
	platform_url = "${base}/${archive_name}"
	launcher_url = "${base}/windows.roc"
	lines = source.split_on("\n").map(
		|line| match line {
			"release_platform = \"\"" => "release_platform = \"${platform_url}\""
			"release_launcher = \"\"" => "release_launcher = \"${launcher_url}\""
			"release_packagers = []" => "release_packagers = [${Str.join_with($packagers, ", ")}]"
			other => other
		},
	)
	text = Str.join_with(lines, "\n")
	if !text.contains(platform_url) or !text.contains(launcher_url) or !text.contains("${base}/roc-windows-msi-macos-arm64") {
		return Err("Could not stamp the release assets into windows.roc")
	}
	entry = "${work}/main.roc"
	write!(entry, text)?
	roc = Env.var_str!("ROC_WINDOWS_ROC") ?? "roc"
	step!(cmd(roc, ["fmt", entry]), "format the release launcher")?
	step!(cmd(roc, ["test", entry]), "check the release launcher")?
	copy!(entry, "${assets}/windows.roc")?
	launcher = Path.read_utf8!(Path.utf8(entry)) ? |error| "Could not read the stamped launcher: ${Str.inspect(error)}"
	digest = Crypto.SHA256.hash(launcher.to_utf8()).to_hex()
	tool_source = Str.join_with(launcher.split_on("\n").map(|line| if (line == "release_launcher_sha256 = \"\"") "release_launcher_sha256 = \"${digest}\"" else line), "\n")
	if (!tool_source.contains(digest)) return Err("Could not stamp the launcher checksum into the tool bundle")
	write!(entry, tool_source)?
	step!(cmd(roc, ["bundle", "--output-dir", assets, "main.roc"]).cwd(staging), "bundle the URL-runnable release tool")?
	Path.rename!(Path.utf8(assets), Path.utf8(out)) ? |error| "Could not publish the local release directory: ${Str.inspect(error)}"
	Stdout.line!("Release assets are in ${out}. Upload the directory's files under ${base}.").map_err(|error| Str.inspect(error))
}

packager! = |target, work, out| {
	build = cmd("cargo", ["build", "--release", "--locked", "--manifest-path", "packaging/Cargo.toml", "--target", target.rust])
	if target.rust.ends_with("-linux-musl") {
		root = cmd("rustc", ["--print", "sysroot"]).capture!() ? |error| "Could not find the Rust sysroot: ${Str.inspect(error)}"
		linker = "${root.trim()}/lib/rustlib/aarch64-apple-darwin/bin/rust-lld"
		linux = build.env(OsStr.from_str(target.linker), OsStr.from_str(linker)).env("RUSTFLAGS", "-C linker-flavor=ld.lld")
		step!(linux, "cross-build MSI packager for ${target.system}")?
	} else if target.zig.is_empty() {
		step!(build, "build MSI packager for ${target.system}")?
	} else {
		linker = "${work}/link-${target.system}"
		write!(linker, linker_script(target.zig))?
		step!(cmd("/bin/chmod", ["700", linker]), "prepare the linker for ${target.system}")?
		full = absolute!(linker)?
		step!(build.env(OsStr.from_str(target.linker), OsStr.from_str(full)), "cross-build MSI packager for ${target.system}")?
	}
	name = "roc-windows-msi-${target.system}${target.ext}"
	file = "${out}/${name}"
	copy!("packaging/target/${target.rust}/release/roc-windows-msi${target.ext}", file)?
	bytes = Path.read_bytes!(Path.utf8(file)) ? |error| "Could not read ${file}: ${Str.inspect(error)}"
	Ok({ name, sha256: Crypto.SHA256.hash(bytes).to_hex() })
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
