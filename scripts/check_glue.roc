#!/usr/bin/env roc-stable
app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	roc: "nightly-2026-11-26-890d6c8",
}

import cli.Cmd
import cli.OsStr
import cli.Path
import cli.Random
import cli.Stderr
import cli.Stdout

## Run from the repository root with the compiler to check and the Rust
## glue specification from that compiler's source revision.
usage : Str
usage = "usage: check_glue [--update] --roc PATH --spec PATH"

generated_name : Str
generated_name = "roc_platform_abi.rs"

committed : Str
committed = "host/src/${generated_name}"

main! : List(OsStr) => Try({}, _)
main! = |args| {
	options = parse(args.map(OsStr.display).drop_if(|arg| arg == "--"))
	match options {
		Ok({ roc, spec, update }) =>
			match check!(roc, spec, update) {
				Ok({}) => Ok({})
				Err(problem) => {
					Stderr.line!("check_glue: ${problem}") ?? {}
					Err(Exit(1))
				}
			}
		Err(Usage) => {
			Stderr.line!(usage) ?? {}
			Err(Exit(2))
		}
	}
}

Options : { roc : Str, spec : Str, update : Bool }

parse : List(Str) -> Try(Options, [Usage])
parse = |args|
	match args {
		["--roc", roc, "--spec", spec] => Ok({ roc, spec, update: Bool.False })
		["--update", "--roc", roc, "--spec", spec] => Ok({ roc, spec, update: Bool.True })
		_ => Err(Usage)
	}

check! : Str, Str, Bool => Try({}, Str)
check! = |roc, spec, update| {
	nonce = Random.seed_u64!() ? |error| "could not make a scratch directory name: ${Str.inspect(error)}"
	dir = "build/glue-${nonce.to_str()}"
	scratch = Path.utf8(dir)
	Path.create_all!(Path.utf8("build")) ? |error| "could not create build: ${Str.inspect(error)}"
	Path.create_dir!(scratch) ? |error| "could not create ${dir}: ${Str.inspect(error)}"
	result = generate!(roc, spec, dir, update)
	cleared = Path.delete_all!(scratch).map_err(|error| "could not remove ${dir}: ${Str.inspect(error)}")
	result?
	cleared?
	message = if (update) "Updated ${committed}." else "Checked ${committed} against the selected Roc compiler."
	Stdout.line!(message) ?? {}
	Ok({})
}

generate! : Str, Str, Str, Bool => Try({}, Str)
generate! = |roc, spec, dir, update| {
	command = Cmd.new(OsStr.from_str(roc)).args(["glue", spec, dir, "platform/main.roc"].map(OsStr.from_str))
	result = command.inherit_stdio().run!() ? |error| "could not invoke ${roc} glue: ${Str.inspect(error)}"
	if result.status != Exited(0) {
		return Err("${roc} glue failed: ${Str.inspect(result.status)}")
	}
	fresh = Path.utf8("${dir}/${generated_name}")
	destination = Path.utf8(committed)
	expected = Path.read_bytes!(fresh) ? |error| "could not read generated ${generated_name}: ${Str.inspect(error)}"
	if update {
		Path.rename!(fresh, destination).map_err(|error| "could not replace ${committed}: ${Str.inspect(error)}")
	} else {
		actual = Path.read_bytes!(destination) ? |error| "could not read ${committed}: ${Str.inspect(error)}"
		if actual == expected {
			Ok({})
		} else {
			Err("${committed} differs from the generated bindings. Regenerate with --update and review the ABI changes.")
		}
	}
}

expect parse(["--roc", "roc", "--spec", "RustGlue.roc"]) == Ok({ roc: "roc", spec: "RustGlue.roc", update: Bool.False })
expect parse(["--update", "--roc", "roc", "--spec", "RustGlue.roc"]) == Ok({ roc: "roc", spec: "RustGlue.roc", update: Bool.True })
expect parse(["--update", "--roc", "roc"]) == Err(Usage)
