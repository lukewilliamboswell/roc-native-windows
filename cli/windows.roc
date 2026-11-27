app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	http: "https://github.com/roc-lang/http/releases/download/1.0.0/6ZUwqYhCS8PU9Mo6MF7oV82ET2o7KYb57CLKDq4cq4sS.tar.zst",
	weaver: "https://github.com/lukewilliamboswell/weaver/releases/download/0.10.0/7CdYzjBAJmprmnK45oTpmH6BXt76ZASN2Ck5uZQEoWsK.tar.zst",
	zip: "https://github.com/lukewilliamboswell/roc-zip/releases/download/0.3.0-rc4/7KtXa6LGYfVGZK5kDMtkHeumUJywvSR15YdQoRvfzxg1.tar.zst",
}

import cli.Cmd
import cli.Duration
import cli.Env
import cli.Http
import cli.OsStr
import cli.Path
import cli.Stderr
import cli.Stdout
import cli.System
import http.Request
import weaver.Cli
import weaver.Opt
import weaver.Param
import weaver.SubCmd
import zip.Zip

PackageFormat : [Msi, Zip]

PackageOptions : { target : Str, format : PackageFormat, id : Str, version : Str, publisher : Str }

MsiMetadata : { id : Str, version : Str, publisher : Str }

app_manifest = "windows.json"

TestOptions : { check : Bool, failure_screenshot : Bool, target : Str }

RecordFormat : [Gif, Png]

Command : [Help, Init(Str, Str, Bool), Build(Str), Run(Str), Test(TestOptions), Record(Str, RecordFormat, Str), Fuzz(Bool), Package(PackageOptions)]

ScaffoldFile : { path : Str, bytes : List(U8) }

# scripts/release.roc replaces these defaults with the release's exact assets.
release_version = "development"

release_platform = ""

release_launcher = ""

release_launcher_sha256 = ""

release_packagers : List({ system : Str, url : Str, sha256 : Str })
release_packagers = []

release_scaffold : List(ScaffoldFile)
release_scaffold = []

main! : List(OsStr.OsStr) => Try({}, [Exit(I32)])
main! = |raw_args| {
	# Also accept invocation through Roc's argument separator.
	args = if (raw_args.first() == Ok(OsStr.from_str("--"))) raw_args.drop_first(1) else raw_args
	command = match Cli.parse_or_display_message(parser, args, OsStr.to_raw) {
		Ok(found) => found
		Err(Help(message)) | Err(Version(message)) => {
			Stdout.line!(message) ?? {}
			return Ok({})
		}
		Err(InvalidUsage(message)) => {
			Stderr.line!(message) ?? {}
			return Err(Exit(2))
		}
	}
	match execute!(command) {
		Ok({}) => Ok({})
		Err(problem) => {
			Stderr.line!("windows.roc: ${explain(problem)}") ?? {}
			Err(Exit(1))
		}
	}
}

parser : Cli.CliParser(Command)
parser =
	Cli.assert_valid(
		Cli.finish(
			Cli.map(
				SubCmd.optional([
					init_command,
					target_command("build", "Build a Windows EXE and copy assets/.", |target| Build(target)),
					target_command("run", "Build and launch the app on Windows.", |target| Run(target)),
					test_command,
					record_command,
					fuzz_command,
					package_command,
					SubCmd.empty({ name: "help", description: "Show this help.", value: Help }),
				]),
				|command| command ?? Help,
			),
			{
				name: "windows",
				description: description,
				version: release_version,
				authors: [],
				text_style: Plain,
			},
		),
	)

description =
	\\Create, build, run, test and package Roc Win32 apps with roc windows.roc.
	\\
	\\Run build, run, test and package from the app directory. The entry point
	\\is src/main.roc, or main.roc for an existing single-file application.
	\\Build, test and package work on Windows, macOS and Linux. You can
	\\cross-compile for Windows x64 or ARM64 from any of those systems.
	\\The EXE and installer run on Windows. The run command requires Windows.
	\\
	\\For MSI packaging, the release tool automatically downloads a packager
	\\for the OS and CPU running Roc. For example, roc-windows-msi-0.1.0-macos-arm64
	\\runs on Apple Silicon Macs and can create either Windows installer.
	\\Choose the Windows architecture with --target.
	\\
	\\  roc windows.roc -- package --target arm64mingw
	\\
	\\Build output goes in build/<target>/. Packages go in dist/.
	\\Set ROC_WINDOWS_ROC to use a particular Roc compiler instead of roc on PATH.

init_command : SubCmd.SubcommandParserConfig(Command)
init_command =
	SubCmd.finish(
		{
			simple: Opt.flag({ short: "", long: "simple", help: "Create a minimal counter with no separate logic or test directories." }),
			source: Opt.str({
				short: "",
				long: "platform",
				help: "Path to platform/main.roc from the working directory, or an HTTPS bundle URL. Defaults to ROC_WINDOWS_PLATFORM or platform/main.roc.",
				default: Value(""),
			}),
			dir: Param.str({
				name: "dir",
				help: "App directory, defaulting to the current directory.",
				default: Value("."),
			}),
		}.Cli,
		{ name: "init", description: "Create the reading-list starter with a logic package, behaviour specs and fuzz tests.", mapper: |{ dir, source, simple }| Init(dir, source, simple) },
	)

target_command : Str, Str, (Str -> Command) -> SubCmd.SubcommandParserConfig(Command)
target_command = |name, summary, mapper|
	SubCmd.finish(target_option, { name, description: summary, mapper })

test_command : SubCmd.SubcommandParserConfig(Command)
test_command =
	SubCmd.finish(
		{
			check: Opt.flag({ short: "", long: "check", help: "Check source, expects and spec syntax without launching the app." }),
			failure_screenshot: Opt.flag({ short: "", long: "screenshot-on-failure", help: "Capture the failed step's window under build/specs/ on Windows." }),
			target: target_option,
		}.Cli,
		{ name: "test", description: "Run Roc expects and the app's Win32 behaviour specs.", mapper: |options| Test(options) },
	)

record_command : SubCmd.SubcommandParserConfig(Command)
record_command =
	SubCmd.finish(
		{
			format: Opt.single({
				short: "",
				long: "format",
				type: "format",
				help: "gif (default) or png for a sequence of PNG frames.",
				parser: |arg| match arg.to_str() {
					Ok("gif") => Ok(Gif)
					Ok("png") => Ok(Png)
					Ok(_) => Err(InvalidValue("expected gif or png"))
					Err(_) => Err(InvalidUtf8)
				},
				default: Value(Gif),
			}),
			target: target_option,
			path: Param.str({ name: "file", help: "A .scm file containing one demo.", default: NoDefault }),
		}.Cli,
		{ name: "record", description: "Record a demo spec on Windows.", mapper: |{ path, format, target }| Record(path, format, target) },
	)

fuzz_command : SubCmd.SubcommandParserConfig(Command)
fuzz_command =
	SubCmd.finish(
		Opt.flag({ short: "", long: "check", help: "Run the declared examples without a generated-input campaign." }),
		{ name: "fuzz", description: "Build the roc-fuzz suite and fuzz each target for ten seconds.", mapper: |check| Fuzz(check) },
	)

target_option =
	Opt.single({
		short: "",
		long: "target",
		type: "target",
		help: "Windows architecture. Use x64mingw for x64 or arm64mingw for ARM64. Defaults to Roc's architecture on Windows and x64mingw on macOS and Linux.",
		parser: |arg|
			match arg.to_str() {
				Ok("x64mingw") => Ok("x64mingw")
				Ok("arm64mingw") => Ok("arm64mingw")
				Ok(_) => Err(InvalidValue("expected x64mingw or arm64mingw"))
				Err(_) => Err(InvalidUtf8)
			},
		default: Value(""),
	})

package_command : SubCmd.SubcommandParserConfig(Command)
package_command =
	SubCmd.finish(
		{
			target: target_option,
			format: Opt.single({
				short: "",
				long: "format",
				type: "format",
				help: "msi (default) uses a packager that runs on your build machine. zip creates a portable Windows app archive.",
				parser: |arg| match arg.to_str() {
					Ok("msi") => Ok(Msi)
					Ok("zip") => Ok(Zip)
					Ok(_) => Err(InvalidValue("expected msi or zip"))
					Err(_) => Err(InvalidUtf8)
				},
				default: Value(Msi),
			}),
			id: Opt.str({ short: "", long: "id", help: "Override the MSI upgrade identity in windows.json.", default: Value("") }),
			version: Opt.str({ short: "", long: "app-version", help: "Override the MSI product version in windows.json (MAJOR.MINOR.PATCH).", default: Value("") }),
			publisher: Opt.str({ short: "", long: "publisher", help: "Override the MSI publisher in windows.json.", default: Value("") }),
		}.Cli,
		{ name: "package", description: "Build a Windows app and create an MSI installer or portable ZIP in dist/.", mapper: |options| Package(options) },
	)

parse_text = |args| Cli.parse_or_display_message(parser, args, |arg| Utf8(arg))

help_text =
	match parse_text(["--help"]) {
		Err(Help(message)) => message
		_ => crash "Weaver did not produce help for windows.roc."
	}

execute! : Command => Try({}, _)
execute! = |command|
	match command {
		Help => Stdout.line!(help_text)
		Init(dir, source, simple) => init!(dir, source, simple)
		Build(target) => build!(target).map_ok(|_| {})
		Run(target) => run!(target)
		Test(options) => test!(options)
		Record(path, format, target) => record!(path, format, target)
		Fuzz(check) => fuzz!(check)
		Package(options) => package!(options)
	}

windows_target! : () => Str
windows_target! = || {
	target = System.target!()
	match (target.os(), target.arch()) {
		(Windows(_), X64) => "x64mingw"
		(Windows(_), Arm64) => "arm64mingw"
		_ => ""
	}
}

run! : Str => Try({}, _)
run! = |requested| {
	own = windows_target!()
	if own.is_empty() {
		return Err(RunRequiresWindows)
	}
	target = if (requested.is_empty()) own else requested
	{ dir, name, .. } = build!(target)?
	program = Path.absolute!(Path.utf8("${dir}/${name}.exe"))?
	ran = Cmd.new(program.to_os_str()).cwd(Path.utf8(dir)).inherit_stdio().run!()?
	check_exit(ran.status, "run")
}

entry! : () => Try(Str, _)
entry! = || {
	if Path.is_file!(Path.utf8("src/main.roc"))? {
		Ok("src/main.roc")
	} else if Path.is_file!(Path.utf8("main.roc"))? {
		Ok("main.roc")
	} else {
		Err(MissingEntry)
	}
}

roc! : List(Str), Str => Try({}, _)
roc! = |args, step| {
	override = env!("ROC_WINDOWS_ROC")
	compiler = if (override.is_empty()) "roc" else override
	if !Cmd.check_available!(compiler) {
		return Err(CompilerMissing(compiler))
	}
	outcome = Cmd.new(OsStr.from_str(compiler)).args(args.map(OsStr.from_str)).inherit_stdio().run!()?
	check_exit(outcome.status, step)
}

check_exit = |status, step|
	if (status == Exited(0)) Ok({}) else Err(ChildFailed(step, Str.inspect(status)))

test! : TestOptions => Try({}, _)
test! = |options| {
	entry = entry!()?
	roc!(["check", entry], "check")?
	roc!(["test", entry], "test")?
	spec_dir = Path.utf8("tests/specs")
	if (!Path.is_dir!(spec_dir)?) return Ok({})
	paths = Path.list!(spec_dir)?.map(Path.display).keep_if(|path| path.ends_with(".scm")).sort_with(compare_name_text)
	if (paths.is_empty()) return Ok({})

	var $scenarios = []
	for path in paths {
		$scenarios = $scenarios.append(read_scenario!(path)?)
	}
	Stdout.line!("Validated ${paths.len().to_str()} spec files.")?

	tests = $scenarios.keep_if(|scenario| scenario.mode == TestMode)
	own = windows_target!()
	if (tests.is_empty()) return Ok({})
	if options.check or own.is_empty() {
		return Stdout.line!("${tests.len().to_str()} Win32 scenarios require a native test run on Windows.")
	}

	target = if (options.target.is_empty()) own else options.target
	program = spec_executable!(target)?
	var $failed = 0.U64
	for scenario in tests {
		match run_scenario!(program, scenario, "", options.failure_screenshot) {
			Ok(output) => {
				Stdout.line!("PASS ${scenario.path}")?
				if !output.is_empty() {
					Stdout.line!("  Artifacts in ${output}")?
				}
			}
			Err(problem) => {
				$failed = $failed + 1
				Stderr.line!("FAIL ${scenario.path}\n  ${problem}")?
			}
		}
	}
	Stdout.line!("Executed ${tests.len().to_str()} Win32 scenarios. ${$failed.to_str()} failed.")?
	if ($failed == 0) Ok({}) else Err(SpecSuiteFailed($failed))
}

record! : Str, RecordFormat, Str => Try({}, _)
record! = |path, format, requested| {
	scenario = read_scenario!(path)?
	if (scenario.mode != DemoMode) return Err(SpecProblem(path, "Record requires a (demo ...) scenario"))
	own = windows_target!()
	if (own.is_empty()) return Err(RecordRequiresWindows)
	target = if (requested.is_empty()) own else requested
	program = spec_executable!(target)?
	output = run_scenario!(program, scenario, if (format == Gif) "gif" else "png", Bool.False) ? |detail| SpecProblem(path, detail)
	Stdout.line!("Recorded ${scenario.name} in ${output}.")
}

spec_executable! : Str => Try(Path.Path, _)
spec_executable! = |target| {
	built = build!(target)?
	Path.absolute!(Path.utf8("${built.dir}/${built.name}.exe"))
}

read_scenario! : Str => Try(Scenario, _)
read_scenario! = |path| {
	source = Path.read_utf8!(Path.utf8(path))?
	spec_parse(path, source).map_err(|issue| SpecProblem("${path}:${issue.pos.line.to_str()}:${issue.pos.column.to_str()}", issue.message))
}

run_scenario! : Path.Path, Scenario, Str, Bool => Try(Str, Str)
run_scenario! = |program, scenario, record_format, failure_screenshot| {
	ensure_dir!("build/specs") ? |issue| "Could not prepare build/specs: ${Str.inspect(issue)}"
	name = Path.filename(Path.utf8(scenario.path)).map_ok(Path.display) ?? "scenario"
	stem = name.drop_suffix(".scm")
	prefix = if (valid_asset_name(stem)) stem else "scenario"
	directory = Env.create_temp_dir_in!(Path.utf8("build/specs"), "${prefix}-") ? |issue| "Could not create an artifact directory: ${Str.inspect(issue)}"
	absolute = Path.absolute!(directory) ? |issue| "Could not resolve the artifact directory: ${Str.inspect(issue)}"
	plan = absolute.join("plan.bin")
	Path.write_bytes!(plan, spec_encode(scenario)) ? |issue| "Could not write the scenario plan: ${Str.inspect(issue)}"
	result = Cmd.new(program.to_os_str())
		.env("ROC_WINDOWS_SPEC", plan.to_os_str())
		.env("ROC_WINDOWS_SPEC_OUT", absolute.to_os_str())
		.env("ROC_WINDOWS_SPEC_RECORD", OsStr.from_str(record_format))
		.env("ROC_WINDOWS_SPEC_FAILURE_SCREENSHOT", OsStr.from_str(if (failure_screenshot) "1" else ""))
		.stdin(Null)
		.stdout(Capture)
		.stderr(Capture)
		.timeout(Duration.seconds(120))
		.output_limit(MaxBytes(1_000_000))
		.run!()
	Path.delete!(plan) ?? {}
	Path.delete_all!(absolute.join("storage")) ?? {}
	entries = (Path.list!(absolute) ?? []).keep_if(|asset| asset != plan)
	if entries.is_empty() {
		Path.delete_all!(absolute) ?? {}
	}
	artifact_note = if (entries.is_empty()) "" else "\n  Artifacts: ${absolute.display()}"
	outcome = result ? |issue| "Could not complete the native run: ${Str.inspect(issue)}${artifact_note}"
	stdout = Str.from_utf8_lossy(outcome.stdout_bytes)
	stderr = Str.from_utf8_lossy(outcome.stderr_bytes).trim()
	match (outcome.status, stdout.trim().split_on("\n").last().map_ok(Str.trim)) {
		(Exited(0), Ok("roc-windows-spec 1 done")) => {
			ready = recording_ready!(record_format, absolute) ? |issue| "Could not inspect the recording: ${Str.inspect(issue)}"
			if !ready {
				Err("Recording finished without producing the expected artifact")
			} else {
				Ok(if (entries.is_empty()) "" else absolute.display())
			}
		}
		_ => {
			details = [stderr, stdout.trim()].drop_if(Str.is_empty)
			explanation = if details.is_empty() {
				"the app exited without the completion marker for protocol 1"
			} else {
				Str.join_with(details, "\n  ")
			}
			Err("Native scenario failed (${Str.inspect(outcome.status)}): ${explanation}${artifact_note}")
		}
	}
}

recording_ready! : Str, Path.Path => Try(Bool, _)
recording_ready! = |format, directory| match format {
	"gif" => Path.is_file!(directory.join("demo.gif"))
	"png" => Path.is_file!(directory.join("frame-0000.png"))
	_ => Ok(Bool.True)
}

# S-expression reading follows roc-native-macos's UPL-licensed positional
# reader. The Win32 capabilities and wire plan are defined here.
SpecPos : { line : U64, column : U64 }

SpecIssue : { pos : SpecPos, message : Str }

SpecExpr := [Word(Str, SpecPos), Quoted(Str, SpecPos), Form(List(SpecExpr), SpecPos)]

SpecReader : { bytes : List(U8), at : U64, line : U64, column : U64 }

SpecMode : [TestMode, DemoMode]

SpecStep : { pos : SpecPos, op : Str, fields : List(Str) }

Scenario : { path : Str, mode : SpecMode, name : Str, steps : List(SpecStep) }

spec_parse : Str, Str -> Try(Scenario, SpecIssue)
spec_parse = |path, source| {
	reader = { bytes: source.to_utf8(), at: 0, line: 1, column: 1 }
	if (reader.bytes.len() > 1_000_000) return Err(spec_error(spec_here(reader), "The spec is larger than 1 MB"))
	(expr, rest) = spec_expr(reader, 0)?
	after = spec_trivia(rest)
	if (after.at != after.bytes.len()) return Err(spec_error(spec_here(after), "A spec file holds exactly one form"))
	match expr {
		Form([Word(kind, at), Quoted(name, name_pos), .. as body], _) => {
			mode = match kind {
				"test" => Ok(TestMode)
				"demo" => Ok(DemoMode)
				_ => Err(spec_error(at, "Expected a test or demo form"))
			}?
			if (name.is_empty()) return Err(spec_error(name_pos, "Expected a nonempty scenario name"))
			(setup, steps) = match body {
				[Form([Word("steps", _), .. as forms], _)] => Ok(([], forms))
				[Form([Word("setup", _), .. as settings], _), Form([Word("steps", _), .. as forms], _)] => Ok((settings, forms))
				_ => Err(spec_error(at, "Expected optional (setup ...) followed by (steps ...)"))
			}?
			if (steps.is_empty()) return Err(spec_error(at, "A scenario needs at least one step"))
			var $compiled = []
			for line in setup {
				$compiled = $compiled.append(spec_setup(line)?)
			}
			for line in steps {
				$compiled = $compiled.append(spec_step(line)?)
			}
			Ok({ path, mode, name, steps: $compiled })
		}
		_ => Err(spec_error(spec_position(expr), "Expected (test \"name\" (steps ...)) or (demo \"name\" (steps ...))"))
	}
}

spec_error : SpecPos, Str -> SpecIssue
spec_error = |pos, message| { pos, message }

spec_here : SpecReader -> SpecPos
spec_here = |reader| { line: reader.line, column: reader.column }

spec_position : SpecExpr -> SpecPos
spec_position = |expr| match expr {
	Word(_, pos) | Quoted(_, pos) | Form(_, pos) => pos
}

spec_peek : SpecReader -> Try(U8, [OutOfBounds])
spec_peek = |reader| reader.bytes.get(reader.at)

spec_advance : SpecReader -> SpecReader
spec_advance = |reader| match spec_peek(reader) {
	Ok('\n') => { ..reader, at: reader.at + 1, line: reader.line + 1, column: 1 }
	Ok(byte) if byte >= 0x80 and byte < 0xC0 => { ..reader, at: reader.at + 1 }
	_ => { ..reader, at: reader.at + 1, column: reader.column + 1 }
}

spec_space : U8 -> Bool
spec_space = |byte| [' ', '\t', '\r', '\n'].contains(byte)

spec_trivia : SpecReader -> SpecReader
spec_trivia = |start| {
	var $reader = start
	var $comment = Bool.False
	while $reader.at < $reader.bytes.len() {
		byte = spec_peek($reader) ?? 0
		if $comment {
			$comment = byte != '\n'
		} else if byte == ';' {
			$comment = Bool.True
		} else if !spec_space(byte) {
			return $reader
		}
		$reader = spec_advance($reader)
	}
	$reader
}

spec_expr : SpecReader, U64 -> Try((SpecExpr, SpecReader), SpecIssue)
spec_expr = |start, depth| {
	reader = spec_trivia(start)
	at = spec_here(reader)
	if (depth > 64) return Err(spec_error(at, "The spec contains too many nested forms"))
	match spec_peek(reader) {
		Err(_) => Err(spec_error(at, "Unexpected end of file"))
		Ok(')') => Err(spec_error(at, "Unexpected closing parenthesis"))
		Ok(0) => Err(spec_error(at, "NUL is not allowed in a spec"))
		Ok('"') => spec_quoted(reader)
		Ok('(') => {
			var $rest = spec_trivia(spec_advance(reader))
			var $items = []
			while spec_peek($rest) != Ok(')') {
				if ($rest.at >= $rest.bytes.len()) return Err(spec_error(at, "This form is never closed"))
				(item, next) = spec_expr($rest, depth + 1)?
				$items = $items.append(item)
				$rest = spec_trivia(next)
			}
			Ok((Form($items, at), spec_advance($rest)))
		}
		Ok(_) => {
			var $rest = reader
			while $rest.at < $rest.bytes.len() {
				byte = spec_peek($rest) ?? 0
				if spec_space(byte) or ['(', ')', '"', ';', 0].contains(byte) {
					break
				}
				$rest = spec_advance($rest)
			}
			word = Str.from_utf8_lossy(reader.bytes.sublist({ start: reader.at, len: $rest.at - reader.at }))
			Ok((Word(word, at), $rest))
		}
	}
}

spec_quoted : SpecReader -> Try((SpecExpr, SpecReader), SpecIssue)
spec_quoted = |start| {
	at = spec_here(start)
	var $reader = spec_advance(start)
	var $bytes = []
	while $reader.at < $reader.bytes.len() {
		match spec_peek($reader) ?? 0 {
			'"' => return Ok((Quoted(Str.from_utf8_lossy($bytes), at), spec_advance($reader)))
			'\r' | '\n' => return Err(spec_error(spec_here($reader), "Use an escape for a line break inside a string"))
			0 => return Err(spec_error(spec_here($reader), "NUL is not allowed in a spec"))
			'\\' => {
				escape_pos = spec_here($reader)
				$reader = spec_advance($reader)
				decoded = match spec_peek($reader) {
					Ok('n') => '\n'
					Ok('r') => '\r'
					Ok('t') => '\t'
					Ok('"') => '"'
					Ok('\\') => '\\'
					_ => return Err(spec_error(escape_pos, "Unknown string escape"))
				}
				$bytes = $bytes.append(decoded)
			}
			byte => {
				$bytes = $bytes.append(byte)
			}
		}
		$reader = spec_advance($reader)
	}
	Err(spec_error(at, "This string is never closed"))
}

spec_setup : SpecExpr -> Try(SpecStep, SpecIssue)
spec_setup = |expr| {
	at = spec_position(expr)
	(op, fields) = match expr {
		Form([Word("live-effects", _)], _) => Ok(("setup-live-effects", []))
		Form([Word("live-effects", _), Word(scope, _)], _) if ["http", "time"].contains(scope) => Ok(("setup-live-effects", [scope]))
		Form([Word("window-size", _), width, height], _) => {
			w = spec_number(width, "window width", 1, 3840)?
			h = spec_number(height, "window height", 1, 2160)?
			Ok(("setup-window-size", [w, h]))
		}
		Form([Word("initial-time", _), value], _) => {
			time = spec_number(value, "clock time", -9223372036854775808, 9223372036854775807)?
			Ok(("setup-initial-time", [time]))
		}
		Form([Word("worker-pool", _), value], _) => {
			count = spec_number(value, "worker limit", 1, 32)?
			Ok(("setup-worker-pool", [count]))
		}
		Form([Word(verb, _), Quoted(key, _), Quoted(value, _)], _) if ["local-storage", "session-storage"].contains(verb) =>
			Ok(("setup-${verb}", [key, value]))
		_ => Err(spec_error(at, "Unknown setup form or invalid arguments"))
	}?
	Ok({ pos: at, op, fields })
}

spec_step : SpecExpr -> Try(SpecStep, SpecIssue)
spec_step = |expr| {
	at = spec_position(expr)
	(op, args) = match expr {
		Form([Word(name, _), .. as arguments], _) => Ok((name, arguments))
		_ => Err(spec_error(at, "Expected a scenario step"))
	}?
	fields = match (op, args) {
		("click", [locator]) | ("toggle", [locator]) | ("press-enter", [locator]) | ("expect-visible", [locator]) | ("expect-absent", [locator]) =>
			spec_locator(locator)
		("fill", [locator, Quoted(value, _)]) | ("expect-text", [locator, Quoted(value, _)]) | ("select", [locator, Quoted(value, _)]) | ("expect-selection", [locator, Quoted(value, _)]) =>
			spec_locator(locator).map_ok(|parts| parts.append(value))
		("expect-checked", [locator, value]) | ("expect-disabled", [locator, value]) => {
			parts = spec_locator(locator)?
			word = match value {
				Word("true", _) | Word("#t", _) => Ok("true")
				Word("false", _) | Word("#f", _) => Ok("false")
				Word("mixed", _) if op == "expect-checked" => Ok("mixed")
				_ => Err(spec_error(spec_position(value), if (op == "expect-checked") "Expected true, false or mixed" else "Expected true or false"))
			}?
			Ok(parts.append(word))
		}
		("files", _) => spec_texts(args, "Expected a fixture path in quotes")
		("folder", []) | ("folder", [Quoted(_, _)]) => spec_texts(args, "Expected a fixture directory in quotes")
		("await-work", []) | ("clipboard-busy", []) => Ok([])
		("expect-clipboard", [count, Quoted(value, _)]) =>
			spec_number(count, "clipboard writes", 0, 4294967295).map_ok(|number| [number, value])
		("screenshot", [Quoted(name, pos)]) =>
			if (valid_asset_name(name)) Ok([name]) else Err(spec_error(pos, "Expected a Windows-safe screenshot name"))
		("wait", [value]) => spec_number(value, "wait in milliseconds", 0, 30000).map_ok(|number| [number])
		("run-effect", [value]) | ("expect-cancelled", [value]) =>
			spec_number(value, "effect number", 1, 4294967295).map_ok(|number| [number])
		("tick-interval", [value]) | ("fire-timer", [value]) =>
			spec_number(value, "period in milliseconds", 1, 4294967295).map_ok(|number| [number])
		("advance-time", [value]) =>
			spec_number(value, "milliseconds to advance", 0, 9223372036854775807).map_ok(|number| [number])
		("expect-pending-effects", [value]) =>
			spec_number(value, "expected count", 0, 4294967295).map_ok(|number| [number])
		("expect-interval", [period, count]) => {
			ms = spec_number(period, "interval in milliseconds", 1, 4294967295)?
			n = spec_number(count, "expected count", 0, 4294967295)?
			Ok([ms, n])
		}
		("stub-http", [Quoted(_, _), .. as options]) =>
			spec_stub(options, { url: "", status: "200", body: "", headers: [] })
		("fail-http", [Quoted(_, _), .. as options]) => spec_failure(options, { kind: "network", detail: "" })
		("expect-request", [number, _, ..]) => {
			index = spec_number(number, "effect number", 1, 4294967295)?
			pairs = spec_request_fields(args.drop_first(1), [])?
			Ok([index, (pairs.len() // 2).to_str()].concat(pairs))
		}
		_ => Err(spec_error(at, "Unknown or malformed step ${op}"))
	}?
	Ok({ pos: at, op, fields })
}

spec_number : SpecExpr, Str, I64, I64 -> Try(Str, SpecIssue)
spec_number = |expr, what, lo, hi| {
	bad = spec_error(spec_position(expr), "Expected ${what} from ${lo.to_str()} through ${hi.to_str()}")
	match expr {
		Word(word, _) => {
			number = I64.from_str(word) ? |_| bad
			if (number >= lo and number <= hi) Ok(number.to_str()) else Err(bad)
		}
		_ => Err(bad)
	}
}

spec_texts : List(SpecExpr), Str -> Try(List(Str), SpecIssue)
spec_texts = |nodes, problem| {
	var $fields = []
	for node in nodes {
		match node {
			Quoted(value, _) if !value.is_empty() => {
				$fields = $fields.append(value)
			}
			_ => return Err(spec_error(spec_position(node), problem))
		}
	}
	Ok($fields)
}

spec_locator : SpecExpr -> Try(List(Str), SpecIssue)
spec_locator = |expr| {
	parts = spec_locator_parts(expr)?
	Ok([(parts.len() // 2).to_str()].concat(parts))
}

spec_locator_parts : SpecExpr -> Try(List(Str), SpecIssue)
spec_locator_parts = |expr| {
	kinds = ["button", "checkbox", "text-field", "text", "file-button", "folder-button", "image", "test-id", "combo-box", "list-box", "radio-group"]
	match expr {
		Form([Word("within", _), outer, inner], _) => {
			left = spec_locator_parts(outer)?
			right = spec_locator_parts(inner)?
			Ok(left.concat(right))
		}
		Form([Word(kind, _), Quoted(label, _)], _) if kinds.contains(kind) and (kind != "test-id" or !label.is_empty()) => Ok([kind, label])
		Form([Word(kind, _)], _) if kinds.contains(kind) and kind != "test-id" => Ok([kind, ""])
		_ => Err(spec_error(spec_position(expr), "Expected a control locator, optionally scoped with within"))
	}
}

spec_stub : List(SpecExpr), { url : Str, status : Str, body : Str, headers : List(Str) } -> Try(List(Str), SpecIssue)
spec_stub = |options, state| match options {
	[] => Ok([state.url, state.status, state.body, (state.headers.len() // 2).to_str()].concat(state.headers))
	[Word(":url", _), Quoted(url, _), .. as rest] => spec_stub(rest, { ..state, url })
	[Word(":status", _), status, .. as rest] => {
		code = spec_number(status, "HTTP status", 100, 999)?
		spec_stub(rest, { ..state, status: code })
	}
	[Word(":body", _), Quoted(body, _), .. as rest] => spec_stub(rest, { ..state, body })
	[Word(":headers", _), Form(values, _), .. as rest] => {
		var $headers = []
		for value in values {
			match value {
				Form([Quoted(name, _), Quoted(text, _)], _) if !name.is_empty() => {
					$headers = $headers.concat([name, text])
				}
				_ => return Err(spec_error(spec_position(value), "Expected a (name value) header pair in quotes"))
			}
		}
		spec_stub(rest, { ..state, headers: $headers })
	}
	[first, ..] => Err(spec_error(spec_position(first), "Use :url, :status, :body or :headers with a value"))
}

spec_failure : List(SpecExpr), { kind : Str, detail : Str } -> Try(List(Str), SpecIssue)
spec_failure = |options, state| match options {
	[] => Ok([state.kind, state.detail])
	[Word(":kind", _), Word(kind, _), .. as rest] if ["network", "timeout"].contains(kind) =>
		spec_failure(rest, { ..state, kind })
	[Word(":detail", _), Quoted(detail, _), .. as rest] => spec_failure(rest, { ..state, detail })
	[first, ..] => Err(spec_error(spec_position(first), "Use :kind network or :kind timeout and optionally :detail"))
}

spec_request_fields : List(SpecExpr), List(Str) -> Try(List(Str), SpecIssue)
spec_request_fields = |options, fields| match options {
	[] => Ok(fields)
	[Word(":timeout", _), value, .. as rest] => {
		timeout = spec_number(value, "request timeout", 0, 4294967295)?
		spec_request_fields(rest, fields.concat(["timeout", timeout]))
	}
	[Word(key, _), Quoted(value, _), .. as rest] if [":method", ":url", ":body"].contains(key) =>
		spec_request_fields(rest, fields.concat([key.drop_prefix(":"), value]))
	[first, ..] => Err(spec_error(spec_position(first), "Use :method, :url, :body or :timeout with a value"))
}

# The CLI and native host exchange a versioned UTF-8 plan, in a file so
# neither the size nor the quoting of Windows environment variables limits
# scenarios. A step carries its source line, column, operation and arguments.
spec_encode : Scenario -> List(U8)
spec_encode = |scenario| {
	mode = if (scenario.mode == TestMode) "test" else "demo"
	fields = scenario.steps.fold(
		["roc-windows-spec 1", mode, scenario.name],
		|out, step| {
			out.concat([step.pos.line.to_str(), step.pos.column.to_str(), (step.fields.len() + 1).to_str(), step.op]).concat(step.fields)
		},
	)
	fields.fold([], |bytes, field| bytes.concat(field.to_utf8()).append(0))
}

compare_name_text = |left, right| compare_name(left.to_utf8(), right.to_utf8())

fuzz! : Bool => Try({}, _)
fuzz! = |check| {
	target = "tests/fuzz/main.roc"
	if !Path.is_file!(Path.utf8(target))? {
		return Err(MissingFuzz)
	}
	system = System.target!()
	match (system.os(), system.arch()) {
		(MacOS(_), Arm64) | (Linux(_), X64) => {}
		_ => return Err(FuzzHost)
	}
	ensure_dir!("build")?
	roc!(["build", "--fuzz", target, "--output=build/fuzz"], "build the roc-fuzz suite")?
	program = Path.absolute!(Path.utf8("build/fuzz"))?.to_os_str()
	listed = Cmd.new(program).args([OsStr.from_str("--list-targets")]).capture!()?
	for name in listed.split_on("\n").map(Str.trim).drop_if(Str.is_empty) {
		operation = if (check) ["check"] else ["run", "--time=10"]
		args = ["--target=${name}"].concat(operation).map(OsStr.from_str)
		outcome = Cmd.new(program).args(args).inherit_stdio().run!()?
		check_exit(outcome.status, "fuzz target ${name}")?
	}
	Ok({})
}

build! : Str => Try({ dir : Str, name : Str, target : Str }, _)
build! = |requested| {
	entry = entry!()?
	own = windows_target!()
	target = if (!requested.is_empty()) requested else if (own.is_empty()) "x64mingw" else own
	dir = "build/${target}"
	name = app_name(Env.cwd!()?.display())
	ensure_dir!("build")?
	stage = Env.create_temp_dir_in!(Path.utf8("build"), ".windows-")?.display()
	outcome = stage_build!(entry, name, target, stage, dir)
	match outcome {
		Err(BuildRecovery(_)) => {}
		_ => Path.delete_all!(Path.utf8(stage)) ?? {}
	}
	outcome?
	Stdout.line!("Built ${dir}/${name}.exe")?
	Ok({ dir, name, target })
}

stage_build! : Str, Str, Str, Str, Str => Try({}, _)
stage_build! = |entry, name, target, stage, dir| {
	fresh = "${stage}/candidate"
	backup = "${stage}/previous"
	ensure_dir!(fresh)?
	program = "${fresh}/${name}.exe"
	roc!(["build", entry, "--target=${target}", "--opt=size", "--output=${program}"], "build")?
	image = Path.utf8(program)
	if !Path.is_file!(image)? {
		return Err(MissingExecutable(program))
	}
	Path.write_bytes!(image, gui_executable(Path.read_bytes!(image)?, target)?)?
	if Path.exists!(Path.utf8("assets"))? {
		copy_assets!("assets", fresh)?
	}
	for notice in ["LICENSE", "THIRD_PARTY_LICENSES.md"] {
		path = Path.utf8(notice)
		if Path.exists!(path)? {
			if (!Path.is_file!(path)?) return Err(InvalidAsset(notice))
			Path.copy!(path, Path.utf8("${fresh}/${notice}"))?
		}
	}
	if Path.exists!(Path.utf8(dir))? {
		if !Path.is_dir!(Path.utf8(dir))? {
			return Err(ExpectedDirectory(dir))
		}
		Path.rename!(Path.utf8(dir), Path.utf8(backup))?
		match Path.rename!(Path.utf8(fresh), Path.utf8(dir)) {
			Ok({}) => Ok({})
			Err(problem) => {
				match Path.rename!(Path.utf8(backup), Path.utf8(dir)) {
					Ok({}) => Err(problem)
					Err(_) => Err(BuildRecovery(backup))
				}
			}
		}
	} else {
		Path.rename!(Path.utf8(fresh), Path.utf8(dir))
	}
}

## Roc currently links Windows executables with the console subsystem.
## Mark the completed PE32+ application as GUI before handing it to users.
## The loader starts the same entry point and no console is opened.
## https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
gui_executable : List(U8), Str -> Try(List(U8), [BadExecutable(Str)])
gui_executable = |bytes, target| {
	bad = |reason| Err(BadExecutable(reason))
	pe = little_endian(bytes, 0x3c, 4)
	if bytes.take_first(2) != "MZ".to_utf8() or pe < 64 or bytes.len() < pe + 24 or bytes.sublist({ start: pe, len: 4 }) != [80, 69, 0, 0] {
		return bad("missing PE header")
	}
	machine = if (target == "arm64mingw") 0xaa64 else 0x8664
	if little_endian(bytes, pe + 4, 2) != machine {
		return bad("unexpected architecture")
	}
	characteristics = little_endian(bytes, pe + 22, 2)
	if characteristics.bitwise_and(0x2002) != 2 {
		return bad("expected an executable image")
	}
	header = pe + 24
	size = little_endian(bytes, pe + 20, 2)
	if size < 112 or bytes.len() < header + size or little_endian(bytes, header, 2) != 0x20b {
		return bad("invalid PE32+ optional header")
	}
	subsystem = little_endian(bytes, header + 68, 2)
	if subsystem == 2 {
		return Ok(bytes)
	}
	if subsystem != 3 {
		return bad("unsupported Windows subsystem")
	}
	if little_endian(bytes, header + 108, 4) >= 5 {
		if size < 152 {
			return bad("truncated data directories")
		}
		if little_endian(bytes, header + 144, 8) != 0 {
			return bad("refusing to modify a signed image")
		}
	}
	# The checksum is optional for ordinary applications. Clear it when
	# changing the subsystem so it doesn't retain a stale value.
	Ok(bytes.take_first(header + 64).concat([0, 0, 0, 0, 2, 0]).concat(bytes.drop_first(header + 70)))
}

little_endian : List(U8), U64, U64 -> U64
little_endian = |bytes, start, len|
	bytes.sublist({ start, len }).fold({ number: 0, shift: 0 }, |acc, byte| { number: acc.number + byte.to_u64().shl_wrap(acc.shift), shift: acc.shift + 8 }).number

ensure_dir! : Str => Try({}, _)
ensure_dir! = |dir| {
	path = Path.utf8(dir)
	if Path.exists!(path)? {
		if (Path.is_dir!(path)?) Ok({}) else Err(ExpectedDirectory(dir))
	} else {
		Path.create_all!(path)
	}
}

## Enumerate regular files and directories once for both staging and packaging.
Asset : { path : Str, source : Path.Path, directory : Bool }

assets! : Path.Path, Str => Try(List(Asset), _)
assets! = |source, relative| {
	if !Path.is_dir!(source)? {
		return Err(ExpectedDirectory(source.display()))
	}
	var $entries = [{ path: relative, source, directory: Bool.True }]
	for entry in Path.list!(source)? {
		part = Path.filename(entry)?.to_str().map_err(|_| InvalidAsset(entry.display()))?
		if !valid_asset_name(part) {
			return Err(InvalidAsset(entry.display()))
		}
		path = "${relative}/${part}"
		match Path.type!(entry)? {
			IsDir => {
				$entries = $entries.concat(assets!(entry, path)?)
			}
			IsFile => {
				$entries = $entries.append({ path, source: entry, directory: Bool.False })
			}
			_ => return Err(InvalidAsset(entry.display()))
		}
	}
	Ok($entries)
}

copy_assets! : Str, Str => Try({}, _)
copy_assets! = |source, destination| {
	for asset in assets!(Path.utf8(source), "assets")? {
		next = "${destination}/${asset.path}"
		if asset.directory {
			ensure_dir!(next)?
		} else {
			Path.copy!(asset.source, Path.utf8(next))?
		}
	}
	Ok({})
}

valid_asset_name : Str -> Bool
valid_asset_name = |part|
	!part.is_empty()
		and !part.ends_with(".")
		and !part.ends_with(" ")
		and !reserved_name(part)
		and !part.to_utf8().any(|byte| byte < 32 or ['<', '>', ':', '"', '/', '\\', '|', '?', '*'].contains(byte))

reserved_name : Str -> Bool
reserved_name = |name| {
	base = name.split_on(".").first() ?? ""
	lower = Str.from_utf8_lossy(base.to_utf8().map(|byte| if (byte >= 'A' and byte <= 'Z') byte + 32 else byte))
	["aux", "con", "nul", "prn", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³"].contains(lower)
}

package! : PackageOptions => Try({}, _)
package! = |options| {
	metadata = if (options.format == Msi) msi_metadata!(options)? else empty_metadata
	built = build!(options.target)?
	extension = if (options.format == Msi) "msi" else "zip"
	ensure_dir!("dist")?
	output = "dist/${built.name}-${built.target}.${extension}"
	path = Path.utf8(output)
	if Path.exists!(path)? and !Path.is_file!(path)? {
		return Err(InvalidOutput(output))
	}
	stage = Env.create_temp_dir_in!(Path.utf8("dist"), ".windows-")?
	file = stage.join("candidate.${extension}")
	saved = match write_package!(built, options.format, metadata, stage, file) {
		Ok({}) => Path.rename!(file, path)
		Err(problem) => Err(problem)
	}
	Path.delete_all!(stage) ?? {}
	saved?
	Stdout.line!("Packaged ${output}")
}

empty_metadata : MsiMetadata
empty_metadata = { id: "", version: "", publisher: "" }

parse_metadata : Str -> Try(MsiMetadata, Str)
parse_metadata = |source| {
	fields : Dict(Str, Str)
	fields = Json.parse(source) ? |_| "expected a JSON object with string values"
	if fields.keys().any(|key| !["id", "version", "publisher"].contains(key)) {
		return Err("the supported fields are id, version and publisher")
	}
	Ok({ id: fields.get("id") ?? "", version: fields.get("version") ?? "", publisher: fields.get("publisher") ?? "" })
}

metadata_override = |flag, setting, fallback|
	[flag, setting].find_first(|value| !value.is_empty()) ?? fallback

merge_metadata : MsiMetadata, MsiMetadata -> MsiMetadata
merge_metadata = |flags, settings| {
	id: metadata_override(flags.id, settings.id, ""),
	version: metadata_override(flags.version, settings.version, "0.1.0"),
	publisher: metadata_override(flags.publisher, settings.publisher, "Local development"),
}

msi_metadata! = |options| {
	path = Path.utf8(app_manifest)
	settings = if Path.exists!(path)? {
		source = Path.read_utf8!(path)?
		parse_metadata(source) ? |problem| InvalidManifest(problem)
	} else empty_metadata
	Ok(merge_metadata({ id: options.id, version: options.version, publisher: options.publisher }, settings))
}

msi_helper! = |stage| {
	override = env!("ROC_WINDOWS_MSI")
	if !override.is_empty() {
		return if (Cmd.check_available!(override)) Ok(override) else Err(PackagerMissing(override))
	}
	system = System.target!()
	id = match (system.os(), system.arch()) {
		(MacOS(_), Arm64) => "macos-arm64"
		(MacOS(_), X64) => "macos-x64"
		(Windows(_), Arm64) => "windows-arm64"
		(Windows(_), X64) => "windows-x64"
		(Linux(_), Arm64) => "linux-arm64"
		(Linux(_), X64) => "linux-x64"
	}
	match release_packagers.find_first(|asset| asset.system == id) {
		Ok(asset) => {
			bytes = download!(asset.url, 32 * 1024 * 1024)?
			if Crypto.SHA256.hash(bytes).to_hex() != asset.sha256 {
				return Err(DownloadDigest(asset.url))
			}
			windows = id.starts_with("windows-")
			destination = stage.join(if (windows) "roc-windows-msi.exe" else "roc-windows-msi")
			Path.write_bytes!(destination, bytes)?
			program = Path.absolute!(destination)?.display()
			if !windows {
				made = Cmd.new("/bin/chmod").args(["700", OsStr.from_str(program)]).inherit_stdio().run!()?
				check_exit(made.status, "prepare the MSI packager")?
			}
			Ok(program)
		}
		Err(_) => if (Cmd.check_available!("roc-windows-msi")) Ok("roc-windows-msi") else Err(PackagerMissing("roc-windows-msi"))
	}
}

download! = |url, limit| {
	request = Request.from_method(GET).with_uri(url).with_timeout(TimeoutMilliseconds(60_000))
	response = Http.send_streaming!(request, { max_redirects: 5 }) ? |problem| DownloadFailed(url, Str.inspect(problem))
	if response.status() != 200 {
		return Err(DownloadFailed(url, "HTTP ${response.status().to_str()}"))
	}
	response.read_to_end!(MaxBytes(limit)).map_err(|problem| DownloadFailed(url, Str.inspect(problem)))
}

write_package! = |built, format, metadata, stage, file| {
	if format == Msi {
		helper = msi_helper!(stage)?
		id = if (metadata.id.is_empty()) "org.roc-lang.local.${built.name}" else metadata.id
		args = [built.target, built.name, id, metadata.version, metadata.publisher, built.dir, file.display()]
		outcome = Cmd.new(OsStr.from_str(helper)).args(args.map(OsStr.from_str)).inherit_stdio().run!()?
		check_exit(outcome.status, "MSI packaging")
	} else {
		Path.write_bytes!(file, zip_bytes!(built)?)
	}
}

zip_bytes! = |built| {
	name = "${built.name}.exe"
	binary = { path: name, source: Path.utf8("${built.dir}/${name}"), directory: Bool.False }
	asset_dir = Path.utf8("${built.dir}/assets")
	files = if (Path.is_dir!(asset_dir)?) assets!(asset_dir, "assets")? else []
	stamp = Zip.dos_time(315532800)
	var $payload = files.append(binary)
	for notice in ["LICENSE", "THIRD_PARTY_LICENSES.md"] {
		source = Path.utf8("${built.dir}/${notice}")
		if Path.is_file!(source)? {
			$payload = $payload.append({ path: notice, source, directory: Bool.False })
		}
	}
	sorted = $payload.sort_with(|a, b| compare_name(a.path.to_utf8(), b.path.to_utf8()))
	var $items = []
	for item in sorted {
		entry = if item.directory {
			Directory({ path: "${item.path}/", modified: stamp, mode: Unix(0o755) })
		} else {
			File({ path: item.path, content: Path.read_bytes!(item.source)?, modified: stamp, mode: Unix(0o644) })
		}
		$items = $items.append(entry)
	}
	Zip.create_items($items, Balanced).map_err(|err| ArchiveFailed(Str.inspect(err)))
}

compare_name : List(U8), List(U8) -> [Before, Same, After]
compare_name = |a, b|
	match (a, b) {
		([], []) => Same
		([], _) => Before
		(_, []) => After
		([x, .. as xs], [y, .. as ys]) => if (x < y) Before else if (x > y) After else compare_name(xs, ys)
	}

init! : Str, Str, Bool => Try({}, _)
init! = |dir, requested, simple| {
	configured = env!("ROC_WINDOWS_PLATFORM")
	source = [requested, configured, release_platform].find_first(|candidate| !candidate.is_empty()) ?? "platform/main.roc"
	main_path = "${dir}/src/main.roc"
	tool_path = "${dir}/windows.roc"
	var $local_name = ""
	for candidate in ["windows-${release_version}.roc", "windows.roc", "cli/windows.roc"] {
		if Path.is_file!(Path.utf8(candidate))? {
			$local_name = candidate
			break
		}
	}
	local_path = Path.utf8($local_name)
	launcher_source = match (release_launcher_sha256, $local_name, dir) {
		("", "", _) | ("", "windows.roc", ".") => Skip
		("", _, _) => Local
		(_, _, _) => Release
	}
	source_ref = platform_source!(source, "${dir}/src")?
	files = if simple {
		[{ path: "src/main.roc", bytes: simple_counter(source_ref).to_utf8() }]
	} else {
		template = scaffold!(release_scaffold)?
		project = app_name(Path.absolute!(Path.utf8(dir))?.display())
		template.map(
			|file|
				if file.path == "src/main.roc" {
					{ ..file, bytes: starter_entry(file.bytes, source_ref) }
				} else if file.path == app_manifest {
					text = Str.from_utf8_lossy(file.bytes).replace_each("\"com.example.reading-list\"", "\"com.example.${project}\"")
					{ ..file, bytes: text.to_utf8() }
				} else file,
		)
	}
	all = files.append({ path: ".gitignore", bytes: "/build/\n/dist/\n/.roc-fuzz/\n".to_utf8() })
	for destination in all.map(|file| "${dir}/${file.path}").concat(if (launcher_source == Skip) [] else [tool_path]) {
		if Path.exists!(Path.utf8(destination))? {
			return Err(InitConflict(destination))
		}
	}
	launcher = match launcher_source {
		Release => {
			bytes = download!(release_launcher, 1024 * 1024)?
			if Crypto.SHA256.hash(bytes).to_hex() != release_launcher_sha256 {
				return Err(DownloadDigest(release_launcher))
			}
			bytes
		}
		Local | Skip => []
	}
	for file in all {
		destination = Path.utf8("${dir}/${file.path}")
		parent = Path.parent(destination) ? |_| InitConflict(destination.display())
		Path.create_all!(parent)?
		Path.write_bytes!(destination, file.bytes)?
	}
	match launcher_source {
		Release => Path.write_bytes!(Path.utf8(tool_path), launcher)?
		Local => {
			_ = Path.copy!(local_path, Path.utf8(tool_path))?
		}
		Skip => {}
	}
	Stdout.line!("Created ${main_path}. Run roc windows.roc build from ${dir}.")
}

scaffold! : List(ScaffoldFile) => Try(List(ScaffoldFile), _)
scaffold! = |embedded| match embedded {
	[] => {
		root = "examples/starter"
		manifest = Path.read_utf8!(Path.utf8("${root}/scaffold.txt")) ? |_| MissingScaffold
		var $files = []
		for path in manifest.split_on("\n").map(Str.trim).drop_if(Str.is_empty) {
			bytes = Path.read_bytes!(Path.utf8("${root}/${path}"))?
			$files = $files.append({ path, bytes })
		}
		Ok($files)
	}
	files => Ok(files)
}

starter_entry : List(U8), Str -> List(U8)
starter_entry = |entry, source|
	Str.from_utf8_lossy(entry).replace_each("../../../platform/main.roc", source).to_utf8()

platform_source! : Str, Str => Try(Str, _)
platform_source! = |source, from| {
	qualified = if source.starts_with("https://") or source.starts_with("http://127.0.0.1:") or source.starts_with("http://localhost:") {
		source
	} else {
		path = Path.utf8(source)
		if !Path.is_file!(path)? {
			return Err(MissingPlatform(source))
		}
		base = Path.absolute!(Path.utf8(from))?.to_str()?
		file = Path.absolute!(path)?.to_str()?
		relative_to(base, file)?
	}
	if qualified.to_utf8().any(|byte| byte < 32 or ['"', '\\'].contains(byte)) {
		Err(InvalidPlatform(qualified))
	} else {
		Ok(qualified)
	}
}

relative_to : Str, Str -> Try(Str, [DifferentDrive])
relative_to = |base, file| {
	first = components(base)
	second = components(file)
	if first.first() != second.first() and (base.starts_with("\\") or file.starts_with("\\") or base.to_utf8().get(1) == Ok(':') or file.to_utf8().get(1) == Ok(':')) {
		return Err(DifferentDrive)
	}
	Ok(relative_parts(first, second))
}

components : Str -> List(Str)
components = |path| {
	var $parts = []
	for part in path.replace_each("\\", "/").split_on("/") {
		if part == ".." {
			$parts = $parts.drop_last(1)
		} else if part != "." and part != "" {
			$parts = $parts.append(part)
		}
	}
	$parts
}

relative_parts : List(Str), List(Str) -> Str
relative_parts = |base, file|
	match (base, file) {
		([a, .. as rest], [b, .. as other]) if a == b => relative_parts(rest, other)
		_ => Str.join_with(base.map(|_| "..").concat(file), "/")
	}

env! : Str => Str
env! = |name| Env.var!(OsStr.from_str(name)).map_ok(OsStr.display) ?? ""

app_name : Str -> Str
app_name = |root| {
	part = root.replace_each("\\", "/").split_on("/").last() ?? ""
	bytes = part.to_utf8().map(
		|byte|
			if byte >= 'A' and byte <= 'Z' {
				byte + 32
			} else if (byte >= 'a' and byte <= 'z') or (byte >= '0' and byte <= '9') {
				byte
			} else {
				'-'
			},
	)
	name = Str.from_utf8_lossy(bytes)
	if bytes.all(|byte| byte == '-') {
		"app"
	} else if reserved_name(name) {
		"${name}-app"
	} else {
		name
	}
}

simple_counter : Str -> Str
simple_counter = |source_ref|
	\\app [main] { pf: platform "${source_ref}" }
	\\
	\\import pf.Engine exposing [Elem, In, Root, State]
	\\import pf.Ui
	\\
	\\main : Engine.Program(_)
	\\main = Engine.program(|| Engine.state(0.I64, view))
	\\
	\\view : State(In(Root, I64), I64) -> Elem(In(Root, I64))
	\\view = |count|
	\\	Ui.vstack(
	\\		{ padding: 16 },
	\\		[
	\\			Ui.text(count.signal().map(|n| n.to_str())),
	\\			Ui.button("+", { on_press: count.update(increment) }),
	\\		],
	\\	)
	\\
	\\increment : I64 -> I64
	\\increment = |count| count + 1
	\\
	\\expect increment(41) == 42
	\\

explain = |problem|
	match problem {
		BadExecutable(reason) => "Roc did not produce a usable Windows executable (${reason})."
		MissingExecutable(path) => "Roc did not produce the executable at ${path}."
		MissingEntry => "Expected src/main.roc or main.roc in the current directory."
		SpecProblem(location, detail) => "${location}: ${detail}"
		SpecSuiteFailed(count) => "${count.to_str()} native scenarios failed."
		RecordRequiresWindows => "Recording requires a Windows x64 or ARM64 machine."
		MissingFuzz => "Expected a roc-fuzz app at tests/fuzz/main.roc."
		FuzzHost => "The bundled roc-fuzz suite runs on Apple Silicon macOS or x64 Linux."
		MissingScaffold => "Use the released tool or run init from the platform checkout. Pass --simple for a minimal counter."
		RunRequiresWindows => "Run requires a Windows x64 or ARM64 machine. You can use build here."
		CompilerMissing(compiler) => "Could not find ${compiler}. Put Roc on PATH or set ROC_WINDOWS_ROC."
		PackagerMissing(helper) => "Could not find ${helper}. Put roc-windows-msi on PATH or set ROC_WINDOWS_MSI to its path. From a platform checkout, build it with blueprint run packager."
		DownloadFailed(url, reason) => "Could not download ${url} (${reason})."
		DownloadDigest(url) => "The download from ${url} did not match the release checksum."
		ChildFailed(step, status) => "The ${step} command failed (${status})."
		ExpectedDirectory(path) => "Expected a directory at ${path}."
		InvalidAsset(path) => "Expected a regular file or directory with a Windows-safe name at ${path}."
		InvalidOutput(path) => "Expected a regular file at ${path}."
		InvalidManifest(detail) => "${app_manifest}: ${detail}."
		BuildRecovery(path) => "Could not restore the previous build. It is saved at ${path}."
		ArchiveFailed(why) => "Could not create a ZIP archive (${why})."
		InitConflict(path) => "Refusing to overwrite ${path}."
		MissingPlatform(path) => "Could not find the platform at ${path}. Pass --platform or set ROC_WINDOWS_PLATFORM."
		InvalidPlatform(path) => "Cannot put ${path} in a Roc app header."
		DifferentDrive => "Place the app on the same drive as the local platform or use a platform URL."
		PathErr(error, path) => "Could not access ${path.display()} (${Str.inspect(error)})."
		other => Str.inspect(other)
	}

expect {
	source = "; utf-8 labels and Windows line endings\r\n(test \"Café\"\r\n  (steps (fill (text-field \"Title\") \"quoted \\\"text\\\"\\n\\tab\")))"
	scenario = spec_parse("tests/specs/reading.scm", source)
	fields = ["roc-windows-spec 1", "test", "Café", "3", "10", "5", "fill", "1", "text-field", "Title", "quoted \"text\"\n\tab"]
	expected = fields.fold([], |bytes, field| bytes.concat(field.to_utf8()).append(0))
	scenario.map_ok(spec_encode) == Ok(expected)
}

expect spec_parse("broken.scm", "(test \"a\"\n  (steps\n    (clik (button \"Add\"))))") ==
	Err({ pos: { line: 3, column: 5 }, message: "Unknown or malformed step clik" })

expect {
	source =
		\\(demo "run"
		\\  (setup (window-size 640 480) (live-effects http))
		\\  (steps
		\\    (expect-checked (within (test-id "row") (checkbox "Read")) #t)
		\\    (stub-http "OK" :url "http://localhost/" :status 201 :headers (("content-type" "text/plain")))
		\\    (expect-request 1 :method "GET" :timeout 25)
		\\    (fail-http "refused" :kind network :detail "Connection refused")))
		\\
	spec_parse("demo.scm", source).map_ok(
		|scenario| {
			mode: scenario.mode,
			steps: scenario.steps.map(|step| (step.op, step.fields)),
		},
	) == Ok({
		mode: DemoMode,
		steps: [
			("setup-window-size", ["640", "480"]),
			("setup-live-effects", ["http"]),
			("expect-checked", ["2", "test-id", "row", "checkbox", "Read", "true"]),
			("stub-http", ["http://localhost/", "201", "", "1", "content-type", "text/plain"]),
			("expect-request", ["1", "2", "method", "GET", "timeout", "25"]),
			("fail-http", ["network", "Connection refused"]),
		],
	})
}

expect {
	invalid = [
		"(test \"a\" (steps))",
		"(test \"a\" (steps (await-work))) trailing",
		"(test \"a\" (steps (screenshot \"../outside\")))",
		"(test \"a\" (steps (expect-checked (checkbox \"X\") yes)))",
		"(test \"a\" (steps (advance-time -1)))",
		"(test \"a\" (steps (expect-visible (test-id \"\"))))",
		"(test \"a\" (steps (files \"\")))",
		"(test \"a\" (steps (files \"bad\\z\")))",
		"(test \"a\" (steps (await-work))",
	]
	invalid.all(|source| spec_parse("invalid.scm", source).is_err())
}

expect parse_text(["build", "--target", "arm64mingw"]) == Ok(Build("arm64mingw"))
expect parse_text(["run", "--target=x64mingw"]) == Ok(Run("x64mingw"))
expect parse_text(["test"]) == Ok(Test({ check: Bool.False, failure_screenshot: Bool.False, target: "" }))
expect parse_text(["test", "--check"]) == Ok(Test({ check: Bool.True, failure_screenshot: Bool.False, target: "" }))
expect parse_text(["record", "tests/specs/demo.scm", "--format", "png"]) == Ok(Record("tests/specs/demo.scm", Png, ""))
expect parse_text(["test", "--target", "arm64mingw", "--screenshot-on-failure"]) == Ok(Test({ check: Bool.False, failure_screenshot: Bool.True, target: "arm64mingw" }))
expect parse_text(["record", "--target", "arm64mingw", "tests/specs/demo.scm"]) == Ok(Record("tests/specs/demo.scm", Gif, "arm64mingw"))
expect parse_text(["help"]) == Ok(Help)
expect parse_text([]) == Ok(Help)
expect {
	match parse_text(["package", "--target", "x64mac"]) {
		Err(InvalidUsage(message)) => message.contains("--target") and message.contains("x64mingw") and message.contains("arm64mingw")
		_ => Bool.False
	}
}
expect {
	match parse_text(["package", "--help"]) {
		Err(Help(message)) => message.contains("MSI installer") and message.contains("--target") and message.contains("--format")
		_ => Bool.False
	}
}
expect parse_text(["package"]) == Ok(Package({ target: "", format: Msi, id: "", version: "", publisher: "" }))
expect parse_text(["package", "--format", "zip", "--target=arm64mingw"]) == Ok(Package({ target: "arm64mingw", format: Zip, id: "", version: "", publisher: "" }))
expect parse_metadata("{\"id\":\"com.example.app\",\"version\":\"1.2.3\"}") == Ok({ id: "com.example.app", version: "1.2.3", publisher: "" })
expect parse_metadata("{\"id\": 1}").is_err()
expect parse_metadata("{\"product\":\"App\"}").is_err()
expect merge_metadata({ ..empty_metadata, version: "2.0.0" }, { id: "com.example.app", version: "1.2.3", publisher: "" }) == { id: "com.example.app", version: "2.0.0", publisher: "Local development" }
expect {
	match parse_text(["package", "--format", "exe"]) {
		Err(InvalidUsage(message)) => message.contains("--format") and message.contains("msi or zip")
		_ => Bool.False
	}
}
expect parse_text(["init", "--platform", "platform/main.roc", "starter"]) == Ok(Init("starter", "platform/main.roc", Bool.False))
expect parse_text(["init", "starter", "--simple", "--platform", "platform/main.roc"]) == Ok(Init("starter", "platform/main.roc", Bool.True))
expect relative_to("/dev/demo/src", "/dev/platform/main.roc") == Ok("../../platform/main.roc")
expect relative_to("C:\\dev\\demo\\src", "D:\\platform\\main.roc") == Err(DifferentDrive)
expect relative_to("C:\\dev\\demo\\src", "C:\\dev\\platform\\main.roc") == Ok("../../platform/main.roc")
expect app_name("/Users/luke/My App") == "my-app"
expect app_name("C:\\apps\\CON") == "con-app"

expect valid_asset_name("photo.jpg")
expect !valid_asset_name("AUX.jpg") and !valid_asset_name("some:file") and !valid_asset_name("dot.")

# A minimal PE32+ header with a console subsystem and an optional certificate table.
pe_fixture : List(U8), Bool -> List(U8)
pe_fixture = |machine, signed| {
	dos = "MZ".to_utf8().concat(List.repeat(0, 58)).concat([64, 0, 0, 0])
	coff = [80, 69, 0, 0].concat(machine).concat(List.repeat(0, 14)).concat([152, 0, 2, 0])
	optional = [11, 2].concat(List.repeat(0, 62)).concat([1, 0, 0, 0, 3, 0]).concat(List.repeat(0, 38)).concat([5, 0, 0, 0])
	directories = List.repeat(0.U8, 32).concat(if (signed) [0, 0, 0, 0, 1, 0, 0, 0] else List.repeat(0, 8))
	dos.concat(coff).concat(optional).concat(directories)
}

expect {
	x64 = pe_fixture([0x64, 0x86], Bool.False)
	arm64 = pe_fixture([0x64, 0xaa], Bool.False)
	gui_x64 = gui_executable(x64, "x64mingw") ?? []
	gui_arm64 = gui_executable(arm64, "arm64mingw") ?? []
	little_endian(gui_x64, 156, 2) == 2 and little_endian(gui_arm64, 156, 2) == 2
		and little_endian(gui_x64, 152, 4) == 0
		and gui_executable(gui_x64, "x64mingw") == Ok(gui_x64)
		and x64.drop_first(158) == gui_x64.drop_first(158)
}

expect gui_executable(pe_fixture([0x64, 0xaa], Bool.False), "x64mingw") == Err(BadExecutable("unexpected architecture"))
expect gui_executable(pe_fixture([0x64, 0x86], Bool.True), "x64mingw") == Err(BadExecutable("refusing to modify a signed image"))
expect gui_executable([], "x64mingw") == Err(BadExecutable("missing PE header"))
