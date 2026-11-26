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
import cli.System

# Windows startup objects and import libraries come from Zig's bundled GNU
# runtime. The probe is disposable. The archives on its link line are the
# runtime inputs for the independently built Roc executable.
zig_version = "0.17.1"

zig_probe_cache = "build/link-inputs/zig-cache"

examples = ["starter", "counter", "todos", "workers", "files", "http"]

windows_targets = [
	{ roc: "x64mingw", rust: "x86_64-pc-windows-gnullvm", zig: "x86_64-windows-gnu" },
	{ roc: "arm64mingw", rust: "aarch64-pc-windows-gnullvm", zig: "aarch64-windows-gnu" },
]

main! = |args| {
	values = args.map(OsStr.display).drop_if(|arg| arg == "--")
	roc = Env.var_str!("ROC_WINDOWS_ROC") ?? "roc"
	match dispatch!(values, roc) {
		Ok({}) => Ok({})
		Err(message) => {
			Stderr.line!(message)?
			Err(Exit(1))
		}
	}
}

dispatch! = |args, roc| match args {
	["check"] => check!(roc)
	["build"] => build!(roc)
	["test-native", .. as options] => match options.drop_if(|arg| arg == "--skip-expects") {
		[] | ["--target", "x64mingw"] | ["--target", "arm64mingw"] => test_native!(roc, options)
		_ => Err("Usage: scripts/build.roc -- test-native [--target TARGET] [--skip-expects]")
	}
	["test-accessibility", .. as options] => test_accessibility!(roc, options)
	["bundle"] => {
		hosts!()?
		bundle!(roc)
	}
	_ => Err("Usage: scripts/build.roc -- check | build | bundle | test-native [--target TARGET] [--skip-expects] | test-accessibility")
}

bundle! = |roc| {
	stage = "build/platform-bundle"
	if Path.exists!(Path.utf8(stage)) ?? Bool.False {
		Path.delete_all!(Path.utf8(stage)) ? |error| "Could not refresh ${stage}: ${Str.inspect(error)}"
	}
	mkdir!(stage)?
	mkdir!("dist/platform")?
	out = Path.absolute!(Path.utf8("dist/platform")) ? |error| "Invalid platform output directory: ${Str.inspect(error)}"
	entries = Path.list!(Path.utf8("platform")) ? |error| "Could not list platform source: ${Str.inspect(error)}"
	modules = entries.keep_oks(|path| Path.filename(path).map_ok(Path.display)).keep_if(|name| name.ends_with(".roc"))
	inputs = ["crt2.obj", "libhost.a", "windows.res", "runtime.lib"]
	for target in windows_targets {
		mkdir!("${stage}/targets/${target.roc}")?
	}
	link_files = windows_targets.fold([], |files, target| files.concat(inputs.map(|name| "targets/${target.roc}/${name}")))
	for file in modules.concat(link_files) {
		copy!("platform/${file}", "${stage}/${file}")?
	}
	notices = ["LICENSE", "THIRD_PARTY_LICENSES.md"]
	for notice in notices {
		copy!(notice, "${stage}/${notice}")?
	}
	arguments = ["bundle", "--output-dir", out.display()].concat(modules).concat(link_files).concat(notices)
	run!(command(roc, arguments).cwd(Path.utf8(stage)), "bundle the Windows platform and both hosts")
}

check! = |roc| {
	step!("roc-stable", ["fmt", "--check", "scripts"])?
	step!(roc, ["fmt", "--check", "platform", "examples", "cli"])?
	step!(roc, ["fmt", "--check", "tests/native-input/src"])?
	for file in ["scripts/build.roc", "scripts/check_glue.roc", "scripts/release.roc", "scripts/serve.roc"] {
		step!("roc-stable", ["test", file])?
	}
	check_glue!(roc)?
	step!("cargo", ["fmt", "--manifest-path", "host/Cargo.toml", "--check"])?
	step!("cargo", ["clippy", "--manifest-path", "host/Cargo.toml", "--locked", "--all-targets", "--", "-D", "warnings"])?
	step!("cargo", ["test", "--manifest-path", "host/Cargo.toml", "--locked"])?
	step!("cargo", ["fmt", "--manifest-path", "packaging/Cargo.toml", "--check"])?
	step!("cargo", ["clippy", "--manifest-path", "packaging/Cargo.toml", "--locked", "--all-targets", "--", "-D", "warnings"])?
	step!("cargo", ["test", "--manifest-path", "packaging/Cargo.toml", "--locked"])?
	for target in windows_targets {
		step!("cargo", ["clippy", "--manifest-path", "host/Cargo.toml", "--locked", "--target", target.rust, "--", "-D", "warnings"])?
	}
	for file in ["platform/main.roc", "cli/windows.roc"] {
		step!(roc, ["check", file])?
		step!(roc, ["test", file])?
	}
	system = System.target!()
	can_fuzz = match (system.os(), system.arch()) {
		(MacOS(_), Arm64) | (Linux(_), X64) => Bool.True
		_ => Bool.False
	}
	for example in examples {
		app_tool!(roc, "examples/${example}", ["test", "--check"])?
		if can_fuzz and ["starter", "files"].contains(example) {
			app_tool!(roc, "examples/${example}", ["fuzz", "--check"])?
		}
	}
	app_tool!(roc, "tests/native-input", ["test", "--check"])?
	build!(roc)
}

app_tool! = |roc, dir, args| {
	cmd = command(roc, ["../../cli/windows.roc", "--"].concat(args)).cwd(Path.utf8(dir)).env("ROC_WINDOWS_ROC", OsStr.from_str(roc))
	run!(cmd, "${dir}: ${Str.join_with(args, " ")}")
}

test_native! = |roc, options| {
	system = System.target!()
	match system.os() {
		Windows(_) => {}
		_ => return Err("Run the test-native task on Windows")
	}
	for example in examples {
		app_tool!(roc, "examples/${example}", ["test"].concat(options))?
	}
	app_tool!(roc, "tests/native-input", ["test"].concat(options))
}

test_accessibility! = |roc, options| {
	system = System.target!()
	match system.os() {
		Windows(_) => step!("powershell.exe", ["-NoProfile", "-File", "tests/accessibility/run.ps1", "-Roc", roc].concat(options))
		_ => Err("Run test-accessibility in an interactive Windows desktop session")
	}
}

check_glue! = |roc| {
	spec = Env.var_str!("ROC_WINDOWS_GLUE_SPEC") ?? "host/glue/RustGlue.roc"
	step!("roc-stable", ["scripts/check_glue.roc", "--", "--roc", roc, "--spec", spec])
}

build! = |roc| {
	hosts!()?
	for target in windows_targets {
		for example in examples {
			app_tool!(roc, "examples/${example}", ["build", "--target", target.roc])?
			check_linked_features!("examples/${example}/build/${target.roc}/${example}.exe", example)?
		}
		app_tool!(roc, "tests/native-input", ["build", "--target", target.roc])?
		check_linked_features!("tests/native-input/build/${target.roc}/native-input.exe", "native-input")?
	}
	Ok({})
}

check_linked_features! = |path, example| {
	bytes = Path.read_bytes!(Path.utf8(path)) ? |error| "Could not read ${path}: ${Str.inspect(error)}"
	pe = pe_number(bytes, 60, 4)
	header = pe + 24
	if bytes.sublist({ start: pe, len: 4 }) != [80, 69, 0, 0] or pe_number(bytes, pe + 12, 8) != 0 {
		return Err("Expected a shipping PE without a COFF symbol table: ${path}")
	}
	dlls = pe_imports(bytes, pe, pe_number(bytes, header + 120, 4)) ? |detail| "${path}: ${detail}"
	if dlls.contains("winhttp.dll") != (example == "http") {
		return Err("Unexpected WinHTTP import configuration: ${path}")
	}
	if dlls.contains("d3d11.dll") != ["files", "workers"].contains(example) {
		return Err("Unexpected Direct3D import configuration: ${path}")
	}
	Ok({})
}

pe_imports : List(U8), U64, U64 -> Try(List(Str), Str)
pe_imports = |bytes, pe, imports_rva| {
	count = pe_number(bytes, pe + 6, 2)
	table = pe + 24 + pe_number(bytes, pe + 20, 2)
	if count == 0 or count > 96 or table + 40 * count > bytes.len() or imports_rva == 0 {
		return Err("invalid PE import or section table")
	}
	var $sections = []
	for index in (0.U64).until(count) {
		at = table + 40 * index
		$sections = $sections.append({ rva: pe_number(bytes, at + 12, 4), size: pe_number(bytes, at + 16, 4), offset: pe_number(bytes, at + 20, 4) })
	}
	sections = $sections
	locate = |rva| sections.find_first(|section| rva >= section.rva and rva - section.rva < section.size).map_ok(|section| section.offset + rva - section.rva)
	start = locate(imports_rva) ? |_| "PE imports are outside the image sections"
	var $dlls = []
	for index in (0.U64).until(128) {
		descriptor = start + 20 * index
		if (descriptor + 20 > bytes.len()) return Err("truncated PE import table")
		name = pe_number(bytes, descriptor + 12, 4)
		if (name == 0) return Ok($dlls)
		offset = locate(name) ? |_| "PE import name is outside the image sections"
		field = bytes.sublist({ start: offset, len: 128 })
		if (!field.contains(0)) return Err("unterminated PE import name")
		$dlls = $dlls.append(Str.from_utf8_lossy(field.split_on(0).first() ?? []))
	}
	Err("missing PE import terminator")
}

pe_number : List(U8), U64, U64 -> U64
pe_number = |bytes, at, width|
	bytes.sublist({ start: at, len: width }).fold({ value: 0.U64, shift: 0.U8 }, |acc, byte| { value: acc.value + byte.to_u64().shl_wrap(acc.shift), shift: acc.shift + 8 }).value

hosts! = || {
	version = command("zig", ["version"]).capture!() ? |error| "Could not determine the Zig version: ${Str.inspect(error)}"
	if (Str.trim(version) != zig_version) return Err("Use Zig ${zig_version} for the Windows runtime")
	for target in windows_targets {
		stage!(target)?
	}
	Ok({})
}

stage! = |target| {
	dir = "platform/targets/${target.roc}"
	scratch = "build/link-inputs/${target.roc}"
	mkdir!(dir)?
	mkdir!(scratch)?
	step!("cargo", ["build", "--manifest-path", "host/Cargo.toml", "--release", "--locked", "--target", target.rust])?
	copy!("host/target/${target.rust}/release/libroc_native_windows.a", "${dir}/libhost.a")?
	resource = command("zig", ["rc", "windows.rc", "../../${dir}/windows.res"]).cwd(Path.utf8("host/windows"))
	# The resource compiler resolves the manifest relative to the source's directory.
	run!(resource, "compile the Windows manifest for ${target.roc}")?
	probe_args = ["cc", "-target", target.zig, "-O2", "-fno-sanitize=all", "-s", "-v", "-mwindows", "host/windows/link_probe.c", "-o", "${scratch}/probe.exe"]
	libraries = ["unwind", "comctl32", "comdlg32", "ole32", "oleaut32", "shell32", "user32", "gdi32", "winhttp", "bcrypt", "windowscodecs", "d3d11", "uiautomationcore", "oleacc", "dwmapi", "usp10", "shlwapi", "ws2_32", "advapi32", "ntdll", "userenv", "psapi", "api-ms-win-core-winrt-l1-1-0", "api-ms-win-core-winrt-string-l1-1-0"]
	probe = command("zig", probe_args.concat(libraries.map(|name| "-l${name}")))
		.env("ZIG_GLOBAL_CACHE_DIR", OsStr.from_str(zig_probe_cache))
		.stdout(Null)
		.stderr(Capture)
		.output_limit(MaxBytes(16_000_000))
		.run!() ? |error| "Could not run the Zig link probe: ${Str.inspect(error)}"
	trace = Str.from_utf8(probe.stderr_bytes) ? |_| "The Zig link probe emitted invalid UTF-8"
	if (probe.status != Exited(0)) return Err("Zig link probe failed: ${trace}")
	link = trace.split_on("\n").find_first(|line| line.starts_with("lld-link ")) ? |_| "No lld-link invocation in Zig output"
	files = link_files(link)
	crt = files.find_first(|file| file.name == "crt2.obj") ? |_| "Zig's startup object is missing from the link probe"
	copy!(crt.path, "${dir}/crt2.obj")?
	archives = files.keep_if(|file| file.name.ends_with(".lib"))
	for required in ["libmingw32.lib", "unwind.lib"] {
		if (!archives.any(|file| file.name == required)) return Err("Zig's ${required} is missing from the link probe")
	}
	for library in archives {
		copy!(library.path, "${scratch}/${library.name}")?
	}
	# Add archives in reverse here because MRI's ADDLIB prepends members.
	commands = archives.fold([], |all, file| ["ADDLIB ${file.name}"].concat(all))
	mri = Str.join_with(["CREATE runtime.lib"].concat(commands).concat(["SAVE", "END", ""]), "\n")
	archive = command("zig", ["ar", "-M"]).cwd(Path.utf8(scratch)).stdin(Bytes(Str.to_utf8(mri)))
	outcome = archive.stdout(Inherit).stderr(Inherit).run!() ? |error| "Could not create runtime.lib: ${Str.inspect(error)}"
	if (outcome.status != Exited(0)) return Err("Creating runtime.lib failed: ${Str.inspect(outcome.status)}")
	copy!("${scratch}/runtime.lib", "${dir}/runtime.lib")
}

# Zig 0.17.1 prints unquoted arguments. Its MinGW linker inputs are generated
# under our relative cache path, so we can recover them even when the absolute
# checkout path contains spaces.
link_files = |line| {
	prefix = "${zig_probe_cache}/"
	line.replace_each("\\", "/").split_on(prefix).drop_first(1).keep_if(|part| part.starts_with("o/")).map(
		|part| {
			relative = part.trim().split_on(" ").first() ?? ""
			{ path: "${prefix}${relative}", name: relative.split_on("/").last() ?? "" }
		},
	)
}

expect link_files("lld-link -IMPLIB:C:\\Dev Folder\\probe.lib C:\\Dev Folder\\build\\link-inputs\\zig-cache\\o\\one\\crt2.obj C:\\Dev Folder\\build\\link-inputs\\zig-cache\\o\\two\\unwind.lib\r") == [
	{ path: "build/link-inputs/zig-cache/o/one/crt2.obj", name: "crt2.obj" },
	{ path: "build/link-inputs/zig-cache/o/two/unwind.lib", name: "unwind.lib" },
]

command = |program, args| Cmd.new(OsStr.from_str(program)).args(args.map(OsStr.from_str))

step! = |program, args| run!(command(program, args), "${program} ${Str.join_with(args, " ")}")

run! = |cmd, label| {
	_ = Stdout.line!("+ ${label}")
	result = cmd.inherit_stdio().run!() ? |error| "${label}: ${Str.inspect(error)}"
	if (result.status == Exited(0)) Ok({}) else Err("${label}: ${Str.inspect(result.status)}")
}

mkdir! = |dir| Path.create_all!(Path.utf8(dir)).map_err(|error| "Could not create ${dir}: ${Str.inspect(error)}")

copy! = |source, destination| Path.copy!(Path.utf8(source), Path.utf8(destination)).map_ok(|_| {}).map_err(|error| "Could not copy ${source} to ${destination}: ${Str.inspect(error)}")
