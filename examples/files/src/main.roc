app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, File]
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Selection : [None, Selected(File)]

store = Engine.store(
	FilesDemo,
	{
		selection: Engine.cell(None),
		status: Engine.cell("Choose a file to preview its contents."),
		preview: Engine.cell(""),
		digest: Engine.cell(""),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	selection = cells.selection.signal()
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.text("Files"),
			Ui.hstack(
				{},
				[
					Ui.file_button("Choose File…", { on_files: Engine.handle(receive) }),
					Ui.button("Clear", { on_press: Engine.handle(|_| reset("Choose a file to preview its contents.")) }),
				],
			),
			Ui.text(cells.status.signal()),
			Ui.text(cells.preview.signal()),
			Ui.text(cells.digest.signal()),
			Engine.watch_latest(
				selection,
				|selected| match selected {
					Selected(file) =>
						match file.slice({ start: 0, len: file.size().min(256) }) {
							Ok(head) => [head.read_bytes({})]
							Err(_) => []
						}
					None => []
				},
				Engine.react(
					|_, result| [
						cells.preview.set(
							match result {
								Ok(bytes) => preview(bytes)
								Err(error) => "Read failed: ${Str.inspect(error)}"
							},
						),
					],
				),
			),
			Engine.watch_latest(
				selection,
				|selected| match selected {
					Selected(file) => [file.sha256()]
					None => []
				},
				Engine.react(
					|_, result| [
						cells.digest.set(
							match result {
								Ok(bytes) => "SHA-256: ${hex(bytes)}"
								Err(error) => "Digest failed: ${Str.inspect(error)}"
							},
						),
					],
				),
			),
		],
	)
}

receive : Engine.Received -> List(Engine.Write(_))
receive = |{ files, refused }|
	match files.first() {
		Ok(file) => [
			cells.selection.set(Selected(file)),
			cells.status.set("${file.name()} · ${file.size().to_str()} bytes"),
			cells.preview.set("Reading…"),
			cells.digest.set("Hashing…"),
		]
		Err(_) if refused > 0 => reset("Could not open the selected file.")
		Err(_) => []
	}

reset : Str -> List(Engine.Write(_))
reset = |status| [cells.selection.set(None), cells.status.set(status), cells.preview.set(""), cells.digest.set("")]

preview : List(U8) -> Str
preview = |bytes|
	if bytes.is_empty() {
		"The file is empty."
	} else {
		Str.join_with(Str.from_utf8_lossy(bytes).split_on("\n").map(|line| line.trim()), " ")
	}

hex : List(U8) -> Str
hex = |bytes| {
	digits = "0123456789abcdef".to_utf8()
	Str.from_utf8_lossy(
		bytes.fold([], |out, byte| out.append(digits.get((byte // 16).to_u64()) ?? '0').append(digits.get((byte % 16).to_u64()) ?? '0')),
	)
}

expect hex([0, 15, 255]) == "000fff"
expect preview("hello\n world".to_utf8()) == "hello world"
