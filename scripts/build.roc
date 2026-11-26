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

# Windows startup objects and import libraries come from Zig's bundled GNU
# runtime. The probe is disposable. The archives on its link line are the
# runtime inputs for the independently built Roc executable.
zig_version = "0.17.1"

examples = ["counter", "todos", "workers", "files", "http"]

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
	["bundle"] => {
		hosts!()?
		mkdir!("dist/platform")?
		step!(roc, ["bundle", "platform/main.roc", "--output-dir=dist/platform"])
	}
	_ => Err("Usage: scripts/build.roc -- check | build | bundle")
}

check! = |roc| {
	step!(roc, ["fmt", "--check", "platform", "examples", "scripts", "windows.roc"])?
	for file in ["scripts/build.roc", "scripts/check_glue.roc"] {
		step!(roc, ["test", file])?
	}
	check_glue!(roc)?
	step!("cargo", ["fmt", "--manifest-path", "host/Cargo.toml", "--check"])?
	step!("cargo", ["clippy", "--manifest-path", "host/Cargo.toml", "--locked", "--all-targets", "--", "-D", "warnings"])?
	step!("cargo", ["test", "--manifest-path", "host/Cargo.toml", "--locked"])?
	for target in windows_targets {
		step!("cargo", ["clippy", "--manifest-path", "host/Cargo.toml", "--locked", "--target", target.rust, "--", "-D", "warnings"])?
	}
	for file in ["platform/main.roc", "windows.roc"].concat(examples.map(|name| "examples/${name}/src/main.roc")) {
		step!(roc, ["check", file])?
		step!(roc, ["test", file])?
	}
	build!(roc)
}

check_glue! = |roc| {
	override = Env.var_str!("ROC_WINDOWS_GLUE_SPEC") ?? ""
	path = Env.var_str!("PATH") ?? ""
	var $spec = override
	for dir in path.split_on(":") {
		candidate = "${dir}/roc-rust-glue"
		if $spec.is_empty() and (Path.is_file!(Path.utf8(candidate)) ?? Bool.False) {
			$spec = candidate
		}
	}
	if ($spec.is_empty()) return Err("Use Blueprint or set ROC_WINDOWS_GLUE_SPEC to the pinned compiler's RustGlue.roc")
	step!(roc, ["scripts/check_glue.roc", "--", "--roc", roc, "--spec", $spec])
}

build! = |roc| {
	hosts!()?
	for target in windows_targets {
		for example in examples {
			dir = "examples/${example}"
			cmd = command(roc, ["../../windows.roc", "--", "build", "--target", target.roc]).cwd(Path.utf8(dir)).env("ROC_WINDOWS_ROC", OsStr.from_str(roc))
			run!(cmd, "build ${example} for ${target.roc}")?
		}
	}
	Ok({})
}

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
	libraries = ["unwind", "comctl32", "comdlg32", "ole32", "oleaut32", "shell32", "user32", "gdi32", "winhttp", "bcrypt", "windowscodecs", "dwmapi", "shlwapi", "ws2_32", "advapi32", "ntdll", "userenv", "psapi"]
	probe = command("zig", probe_args.concat(libraries.map(|name| "-l${name}"))).stdout(Null).stderr(Capture).output_limit(MaxBytes(16_000_000)).run!() ? |error| "Could not run the Zig link probe: ${Str.inspect(error)}"
	trace = Str.from_utf8(probe.stderr_bytes) ? |_| "The Zig link probe emitted invalid UTF-8"
	if (probe.status != Exited(0)) return Err("Zig link probe failed: ${trace}")
	link = trace.split_on("\n").find_first(|line| line.starts_with("lld-link ")) ? |_| "No lld-link invocation in Zig output"
	paths = link_words(link)
	crt = paths.find_first(|path| path.ends_with("/crt2.obj")) ? |_| "Zig's startup object is missing from the link probe"
	copy!(crt, "${dir}/crt2.obj")?
	archives = paths.keep_if(|path| path.ends_with(".lib") and !path.starts_with("-"))
	for required in ["libmingw32.lib", "unwind.lib"] {
		if (!archives.any(|path| path.ends_with("/${required}"))) return Err("Zig's ${required} is missing from the link probe")
	}
	for library in archives {
		name = library.split_on("/").last() ? |_| "Invalid runtime library path"
		copy!(library, "${scratch}/${name}")?
	}
	# Add archives in reverse here because MRI's ADDLIB prepends members.
	commands = archives.fold([], |all, path| ["ADDLIB ${path.split_on("/").last() ?? ""}"].concat(all))
	mri = Str.join_with(["CREATE runtime.lib"].concat(commands).concat(["SAVE", "END", ""]), "\n")
	archive = command("zig", ["ar", "-M"]).cwd(Path.utf8(scratch)).stdin(Bytes(Str.to_utf8(mri)))
	outcome = archive.stdout(Inherit).stderr(Inherit).run!() ? |error| "Could not create runtime.lib: ${Str.inspect(error)}"
	if (outcome.status != Exited(0)) return Err("Creating runtime.lib failed: ${Str.inspect(outcome.status)}")
	copy!("${scratch}/runtime.lib", "${dir}/runtime.lib")
}

# Translate Zig's link trace. Only double quotes are used for path arguments.
link_words = |line| {
	parts = Str.split_on(line, "\"")
	parts.map_with_index(|part, index| if (index % 2 == 0) Str.split_on(part, " ").keep_if(|word| !word.is_empty()) else [part]).fold([], |all, words| all.concat(words))
}

expect link_words("lld-link -out:probe.exe \"/space here/x.lib\" /tmp/crt2.obj") == ["lld-link", "-out:probe.exe", "/space here/x.lib", "/tmp/crt2.obj"]

command = |program, args| Cmd.new(OsStr.from_str(program)).args(args.map(OsStr.from_str))

step! = |program, args| run!(command(program, args), "${program} ${Str.join_with(args, " ")}")

run! = |cmd, label| {
	_ = Stdout.line!("+ ${label}")
	result = cmd.inherit_stdio().run!() ? |error| "${label}: ${Str.inspect(error)}"
	if (result.status == Exited(0)) Ok({}) else Err("${label}: ${Str.inspect(result.status)}")
}

mkdir! = |dir| Path.create_all!(Path.utf8(dir)).map_err(|error| "Could not create ${dir}: ${Str.inspect(error)}")

copy! = |source, destination| Path.copy!(Path.utf8(source), Path.utf8(destination)).map_ok(|_| {}).map_err(|error| "Could not copy ${source} to ${destination}: ${Str.inspect(error)}")
