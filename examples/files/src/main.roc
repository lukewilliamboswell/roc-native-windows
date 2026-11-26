app [main] { logic: "../logic/main.roc", pf: platform "../../../platform/main.roc" }

import logic.Inspector
import pf.Engine exposing [Elem, File]
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Selection : [None, Selected(File)]

store = Engine.store(
	FilesDemo,
	{
		selection: Engine.cell(None),
		display: Engine.cell(Inspector.idle),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	selection = cells.selection.signal()
	display = cells.display.signal()
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{ spacing: 12 },
				[
					Ui.asset_image("files.bmp", { label: "A document in a folder", width: 48, height: 48 }),
					Ui.text("Files"),
				],
			),
			Ui.hstack(
				{},
				[
					Ui.file_button("Choose File…", { on_files: Engine.handle(receive) }),
					Ui.button("Clear", { on_press: Engine.handle(|_| reset(Clear)) }),
				],
			),
			Ui.text(display.map(|state| state.status)),
			Ui.text(display.map(|state| state.preview)),
			Ui.text(display.map(|state| state.digest)),
			Engine.watch_latest(
				selection,
				|selected| match selected {
					Selected(file) =>
						match file.slice({ start: 0, len: file.size().min(Inspector.preview_limit) }) {
							Ok(head) => [head.read_bytes({})]
							Err(_) => []
						}
					None => []
				},
				Engine.react(
					|_, result| [
						change(Preview(result.map_err(Str.inspect))),
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
						change(Digest(result.map_err(Str.inspect))),
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
			change(Choose(file.name(), file.size())),
		]
		Err(_) if refused > 0 => reset(Refused)
		Err(_) => []
	}

change : Inspector.Action -> Engine.Write(_)
change = |action| cells.display.update(|display| Inspector.update(display, action))

reset : Inspector.Action -> List(Engine.Write(_))
reset = |action| [cells.selection.set(None), change(action)]
