app [main] { logic: "../logic/main.roc", pf: platform "../../../platform/main.roc" }

import logic.Inspector
import pf.Clipboard
import pf.Engine exposing [Elem, File, Signal]
import pf.Files
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Queued : { key : Str, file : File }

Selection : [None, Selected(File)]

PreviewBytes : [NoPreview, Loaded(List(U8))]

FolderRequest := { sequence : U64, selected : [NoFolder, ReadFolder(Files.Folder)] }.{
	is_eq : FolderRequest, FolderRequest -> Bool
	is_eq = |first, second| first.sequence == second.sequence
}

no_folder = FolderRequest.{ sequence: 0, selected: NoFolder }

store = Engine.store(
	FilesDemo,
	{
		queue: Engine.cell([]),
		next_key: Engine.cell(1.U64),
		selected: Engine.cell(Ui.Selection.NoSelection),
		display: Engine.cell(Inspector.idle),
		copies: Engine.cell(Engine.outbox({})),
		copy_status: Engine.cell(""),
		preview_bytes: Engine.cell(NoPreview),
		format: Engine.cell("text"),
		folder: Engine.cell(no_folder),
		pick_status: Engine.cell(""),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	queue = cells.queue.signal()
	selected = cells.selected.signal()
	format = cells.format.signal()
	selection = { queue, selected }.Signal.map(|{ queue: entries, selected: key }| active(entries, key))
	display = cells.display.signal()
	checksum = display.map(|state| if (state.digest.starts_with("SHA-256: ")) state.digest.drop_prefix("SHA-256: ") else "")
	preview = { display, bytes: cells.preview_bytes.signal(), format }.Signal.map(
		|{ display: state, bytes, format: mode }| match (mode, bytes) {
			("hex", Loaded(raw)) => Inspector.hex(raw)
			_ => state.preview
		},
	)
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
					Ui.file_button("Choose Files…", { selection: MultipleFiles, on_files: Engine.handle_by(cells.next_key.signal(), receive) }),
					Ui.folder_button("Choose Folder…", { on_folder: Engine.handle(receive_folder) }),
					Ui.button("Clear", { on_press: Engine.handle(|_| reset(Clear)) }),
				],
			),
			Ui.list_box({
				label: "Picked files",
				choices: queue.map(|entries| entries.map(|item| Ui.Choice.{ key: item.key, label: item.file.name() })),
				selected,
				on_select: Engine.handle_by(queue, select),
				visible_rows: 4,
			}),
			Ui.text(display.map(|state| state.status)),
			Ui.text(cells.pick_status.signal()),
			Ui.radio_group({
				label: "Preview format",
				choices: [Ui.Choice.{ key: "text", label: "Text" }, Ui.Choice.{ key: "hex", label: "Hex" }],
				selected: format,
				on_select: Engine.handle(|key| [cells.format.set(key)]),
			}),
			Ui.text(preview),
			Ui.text(display.map(|state| state.digest)),
			Ui.button("Copy SHA-256", { enabled: checksum.map(|text| !text.is_empty()), on_press: Engine.handle_by(checksum, |text, _| if (text.is_empty()) [] else [cells.copies.update(|outbox| outbox.post(text))]) }),
			Ui.text(cells.copy_status.signal()),
			Engine.watch_latest(
				cells.folder.signal(),
				|request| match request.selected {
					NoFolder => []
					ReadFolder(folder) => [folder.files({})]
				},
				Engine.react_by(cells.next_key.signal(), folder_listed),
			),
			Engine.on_post(
				cells.copies.signal(),
				|text| [Clipboard.text(text)],
				Engine.react(|_, answer| [cells.copy_status.set(if (answer == Ok({})) "SHA-256 copied." else "Could not copy SHA-256.")]),
			),
			Engine.watch_latest(
				selection,
				|current| match current {
					Selected(file) =>
						match file.slice({ start: 0, len: file.size().min(Inspector.preview_limit) }) {
							Ok(head) => [head.read_bytes({})]
							Err(_) => []
						}
					None => []
				},
				Engine.react(
					|_, result| [
						cells.preview_bytes.set(
							match result {
								Ok(bytes) => Loaded(bytes)
								Err(_) => NoPreview
							},
						),
						change(Preview(result.map_err(Str.inspect))),
					],
				),
			),
			Engine.watch_latest(
				selection,
				|current| match current {
					Selected(file) => [file.sha256()]
					None => []
				},
				Engine.react(|_, result| [change(Digest(result.map_err(Str.inspect)))]),
			),
		],
	)
}

active : List(Queued), Ui.Selection -> Selection
active = |entries, selection|
	match selection {
		NoSelection => None
		Selected(key) => entries.find_first(|item| item.key == key).map_ok(|item| Selected(item.file)) ?? None
	}

select : List(Queued), Ui.Selection -> List(Engine.Write(_))
select = |entries, selection| {
	action = match active(entries, selection) {
		Selected(file) => Choose(file.name(), file.size())
		None => Clear
	}
	[cells.selected.set(selection), cells.preview_bytes.set(NoPreview), cells.copy_status.set(""), change(action)]
}

receive : U64, Engine.Received -> List(Engine.Write(_))
receive = |base, { files, refused }| {
	if files.is_empty() and refused == 0 {
		[]
	} else {
		status = if (refused > 0 and !files.is_empty()) "${refused.to_str()} selected files could not be opened." else ""
		action = if (refused > 0) Refused else Clear
		[clear_folder, cells.pick_status.set(status)].concat(set_queue(base, files, action))
	}
}

receive_folder : Files.FolderSelection -> List(Engine.Write(_))
receive_folder = |choice| match choice {
	FolderChosen(folder) => [
		cells.folder.update(|previous| FolderRequest.{ sequence: previous.sequence + 1, selected: ReadFolder(folder) }),
		cells.pick_status.set("Reading ${folder.name()}…"),
	]
	FolderRefused(_) => [clear_folder, cells.pick_status.set("Could not open the selected folder.")]
}

folder_listed : U64, FolderRequest, Try(Files.FolderListing, Files.FolderFailure) -> List(Engine.Write(_))
folder_listed = |base, request, answer| match (request.selected, answer) {
	(ReadFolder(folder), Ok({ files, completeness })) => {
		name = folder.name()
		status = match completeness {
			Complete if files.is_empty() => "${name} contains no files."
			Complete => "Opened ${files.len().to_str()} files from ${name}."
			Partial(_) => "Opened ${files.len().to_str()} files from ${name}. Some entries were omitted."
		}
		[cells.pick_status.set(status)].concat(set_queue(base, files, Clear))
	}
	(ReadFolder(_), Err(_)) => [cells.pick_status.set("Could not read the selected folder.")]
	(NoFolder, _) => []
}

set_queue : U64, List(File), Inspector.Action -> List(Engine.Write(_))
set_queue = |base, files, empty_action| {
	queued = files.map_with_index(|file, index| { key: (base + index).to_str(), file })
	match queued.first() {
		Ok(first) => [
			cells.queue.set(queued),
			cells.next_key.set(base + queued.len()),
		].concat(select(queued, Selected(first.key)))
		Err(_) => empty_queue(empty_action)
	}
}

change : Inspector.Action -> Engine.Write(_)
change = |action| cells.display.update(|display| Inspector.update(display, action))

reset : Inspector.Action -> List(Engine.Write(_))
reset = |action| [clear_folder, cells.pick_status.set("")].concat(empty_queue(action))

clear_folder : Engine.Write(_)
clear_folder = cells.folder.update(|previous| FolderRequest.{ sequence: previous.sequence + 1, selected: NoFolder })

empty_queue : Inspector.Action -> List(Engine.Write(_))
empty_queue = |action| [cells.queue.set([]), cells.selected.set(NoSelection), cells.preview_bytes.set(NoPreview), cells.copy_status.set(""), change(action)]
