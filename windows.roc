app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	weaver: "https://github.com/lukewilliamboswell/weaver/releases/download/0.10.0/7CdYzjBAJmprmnK45oTpmH6BXt76ZASN2Ck5uZQEoWsK.tar.zst",
	zip: "https://github.com/lukewilliamboswell/roc-zip/releases/download/0.3.0-rc4/7KtXa6LGYfVGZK5kDMtkHeumUJywvSR15YdQoRvfzxg1.tar.zst",
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
import weaver.SubCmd
import zip.Zip

Command : [Help, Init(Str, Str), Build(Str), Run(Str), Test, Package(Str)]

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
					target_command("build", "Build an EXE and copy assets/.", |target| Build(target)),
					target_command("run", "Build and launch the app on Windows.", |target| Run(target)),
					SubCmd.empty({ name: "test", description: "Check source and run Roc expects.", value: Test }),
					target_command("package", "Build and create a ZIP in dist/.", |target| Package(target)),
					SubCmd.empty({ name: "help", description: "Show this help.", value: Help }),
				]),
				|command| command ?? Help,
			),
			{
				name: "windows",
				description: description,
				version: "development",
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
	\\Build output goes in build/<target>/. Build, test and package work on
	\\macOS as well as on Windows. Set ROC_WINDOWS_ROC to use a particular
	\\Roc compiler instead of roc on PATH.

init_command : SubCmd.SubcommandParserConfig(Command)
init_command =
	SubCmd.finish(
		{
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
		{ name: "init", description: "Create a starter app.", mapper: |{ dir, source }| Init(dir, source) },
	)

target_command : Str, Str, (Str -> Command) -> SubCmd.SubcommandParserConfig(Command)
target_command = |name, summary, mapper|
	SubCmd.finish(
		Opt.single({
			short: "",
			long: "target",
			type: "target",
			help: "x64mingw or arm64mingw. Defaults to the Windows Roc process architecture, or x64mingw on other systems.",
			parser: |arg|
				match arg.to_str() {
					Ok("x64mingw") => Ok("x64mingw")
					Ok("arm64mingw") => Ok("arm64mingw")
					Ok(_) => Err(InvalidValue("expected x64mingw or arm64mingw"))
					Err(_) => Err(InvalidUtf8)
				},
			default: Value(""),
		}),
		{ name, description: summary, mapper },
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
		Init(dir, source) => init!(dir, source)
		Build(target) => build!(target).map_ok(|_| {})
		Run(target) => run!(target)
		Test => test!()
		Package(target) => package!(target)
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

test! : () => Try({}, _)
test! = || {
	entry = entry!()?
	roc!(["check", entry], "check")?
	roc!(["test", entry], "test")
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

package! : Str => Try({}, _)
package! = |target| {
	built = build!(target)?
	name = "${built.name}.exe"
	binary = { path: name, source: Path.utf8("${built.dir}/${name}"), directory: Bool.False }
	asset_dir = Path.utf8("${built.dir}/assets")
	files = if (Path.is_dir!(asset_dir)?) assets!(asset_dir, "assets")? else []
	stamp = Zip.dos_time(315532800)
	sorted = files.append(binary).sort_with(|a, b| compare_name(a.path.to_utf8(), b.path.to_utf8()))
	var $items = []
	for item in sorted {
		entry = if item.directory {
			Directory({ path: "${item.path}/", modified: stamp, mode: Unix(0o755) })
		} else {
			File({ path: item.path, content: Path.read_bytes!(item.source)?, modified: stamp, mode: Unix(0o644) })
		}
		$items = $items.append(entry)
	}
	data = Zip.create_items($items, Balanced).map_err(|err| ArchiveFailed(Str.inspect(err)))?
	ensure_dir!("dist")?
	output = "dist/${built.name}-${built.target}.zip"
	path = Path.utf8(output)
	if Path.exists!(path)? and !Path.is_file!(path)? {
		return Err(InvalidOutput(output))
	}
	stage = Env.create_temp_dir_in!(Path.utf8("dist"), ".windows-")?
	file = stage.join("candidate.zip")
	saved = match Path.write_bytes!(file, data) {
		Ok({}) => Path.rename!(file, path)
		Err(problem) => Err(problem)
	}
	Path.delete_all!(stage) ?? {}
	saved?
	Stdout.line!("Packaged ${output}")
}

compare_name : List(U8), List(U8) -> [Before, Same, After]
compare_name = |a, b|
	match (a, b) {
		([], []) => Same
		([], _) => Before
		(_, []) => After
		([x, .. as xs], [y, .. as ys]) => if (x < y) Before else if (x > y) After else compare_name(xs, ys)
	}

init! : Str, Str => Try({}, _)
init! = |dir, requested| {
	configured = env!("ROC_WINDOWS_PLATFORM")
	source = if (!requested.is_empty()) requested else if (!configured.is_empty()) configured else "platform/main.roc"
	main_path = "${dir}/src/main.roc"
	ignore_path = "${dir}/.gitignore"
	tool_path = "${dir}/windows.roc"
	copy_tool = dir != "." and Path.is_file!(Path.utf8("windows.roc"))?
	for destination in [main_path, ignore_path].concat(if (copy_tool) [tool_path] else []) {
		if Path.exists!(Path.utf8(destination))? {
			return Err(InitConflict(destination))
		}
	}
	source_ref = platform_source!(source, "${dir}/src")?
	ensure_dir!(dir)?
	ensure_dir!("${dir}/src")?
	Path.write_utf8!(Path.utf8(main_path), starter(source_ref))?
	Path.write_utf8!(Path.utf8(ignore_path), "/build/\n/dist/\n")?
	if copy_tool {
		Path.copy!(Path.utf8("windows.roc"), Path.utf8(tool_path))?
	}
	Stdout.line!("Created ${main_path}. Run roc windows.roc build from ${dir}.")
}

platform_source! : Str, Str => Try(Str, _)
platform_source! = |source, from| {
	qualified = if source.starts_with("https://") {
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

starter : Str -> Str
starter = |source_ref|
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
		RunRequiresWindows => "Run requires a Windows x64 or ARM64 machine. You can use build here."
		CompilerMissing(compiler) => "Could not find ${compiler}. Put Roc on PATH or set ROC_WINDOWS_ROC."
		ChildFailed(step, status) => "The ${step} command failed (${status})."
		ExpectedDirectory(path) => "Expected a directory at ${path}."
		InvalidAsset(path) => "Expected a regular file or directory with a Windows-safe name at ${path}."
		InvalidOutput(path) => "Expected a regular file at ${path}."
		BuildRecovery(path) => "Could not restore the previous build. It is saved at ${path}."
		ArchiveFailed(why) => "Could not create a ZIP archive (${why})."
		InitConflict(path) => "Refusing to overwrite ${path}."
		MissingPlatform(path) => "Could not find the platform at ${path}. Pass --platform or set ROC_WINDOWS_PLATFORM."
		InvalidPlatform(path) => "Cannot put ${path} in a Roc app header."
		DifferentDrive => "Place the app on the same drive as the local platform or use a platform URL."
		PathErr(error, path) => "Could not access ${path.display()} (${Str.inspect(error)})."
		other => Str.inspect(other)
	}

expect parse_text(["build", "--target", "arm64mingw"]) == Ok(Build("arm64mingw"))
expect parse_text(["run", "--target=x64mingw"]) == Ok(Run("x64mingw"))
expect parse_text(["test"]) == Ok(Test)
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
		Err(Help(message)) => message.contains("Build and create a ZIP") and message.contains("--target")
		_ => Bool.False
	}
}
expect parse_text(["init", "--platform", "platform/main.roc", "starter"]) == Ok(Init("starter", "platform/main.roc"))
expect parse_text(["init", "starter", "--platform", "platform/main.roc"]) == Ok(Init("starter", "platform/main.roc"))
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
