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
		detail: Engine.cell("preview"),
		grouped: Engine.cell(Bool.False),
		group_selection: Engine.cell(Ui.Selection.NoSelection),
		expanded: Engine.cell(Set.empty()),
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
	detail = cells.detail.signal()
	selection = { queue, selected }.Signal.map(|{ queue: entries, selected: key }| active(entries, key))
	group_selection = cells.group_selection.signal()
	tree_selected = { selected, group_selection }.Signal.map(|value|
		match value.group_selection {
			Selected(_) => value.group_selection
			NoSelection => value.selected
		},
	)
	group_summary = { queue, group_selection }.Signal.map(|value|
		match value.group_selection {
			Selected(group) => group_title(value.queue, group.drop_prefix("type:"))
			NoSelection => ""
		},
	)
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
			Ui.checkbox(
				"Group by file type",
				{ checked: cells.grouped.signal(), on_change: Engine.handle_by(selection, toggle_grouped) },
			),
			Engine.when(
				cells.grouped.signal(),
				|| Ui.vstack({}, [
					Ui.tree_view({
						label: "Picked files",
						nodes: queue.map(group_nodes),
						selected: tree_selected,
						on_select: Engine.handle_by(queue, select_tree),
						expanded: cells.expanded.signal(),
						on_expand: Engine.handle(expand_group),
						visible_rows: 5,
					}),
					Engine.when(group_summary.map(|title| !title.is_empty()), || Ui.text(group_summary), || Ui.vstack({}, [])),
				]),
				|| Ui.list_box({
					label: "Picked files",
					choices: queue.map(|entries| entries.map(|item| Ui.Choice.{ key: item.key, label: item.file.name() })),
					selected,
					on_select: Engine.handle_by(queue, select),
					visible_rows: 4,
				}),
			),
			Ui.text_with(display.map(|state| state.name), { overflow: EllipsizeEnd }),
			Ui.text(display.map(|state| state.size)),
			Ui.text(display.map(|state| state.status)),
			Ui.text(cells.pick_status.signal()),
			Ui.tabs(
				{
					label: "File details",
					tabs: [
						Ui.Choice.{ key: "preview", label: "Preview" },
						Ui.Choice.{ key: "fingerprint", label: "Fingerprint" },
					],
					selected: detail,
					on_select: Engine.handle(|key| [cells.detail.set(key)]),
				},
				Engine.switch(detail, ["preview", "fingerprint"], |key|
					if (key == "preview") {
						Ui.vstack(
							{},
							[
								Ui.radio_group({
									label: "Preview format",
									choices: [Ui.Choice.{ key: "text", label: "Text" }, Ui.Choice.{ key: "hex", label: "Hex" }],
									selected: format,
									on_select: Engine.handle(|format_key| [cells.format.set(format_key)]),
								}),
								Ui.text_area(preview, { label: "Contents" }),
							],
						)
					} else {
						Ui.vstack(
							{},
							[
								Ui.text(display.map(|state| state.digest)),
								Ui.button(
									"Copy SHA-256",
									{
										enabled: checksum.map(|text| !text.is_empty()),
										on_press: Engine.handle_by(checksum, |text, _| if (text.is_empty()) [] else [cells.copies.update(|outbox| outbox.post(text))]),
									},
								),
								Ui.text(cells.copy_status.signal()),
							],
						)
					},
				),
			),
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

file_type : File -> Str
file_type = |file| Inspector.extension(file.name())

group_key : Str -> Str
group_key = |extension| "type:${extension}"

group_title : List(Queued), Str -> Str
group_title = |entries, extension| {
	name = if (extension.is_empty()) "No extension" else ".${extension}"
	count = entries.keep_if(|item| file_type(item.file) == extension).len()
	"${name} (${count.to_str()})"
}

group_nodes : List(Queued) -> List(Ui.TreeNode)
group_nodes = |entries| {
	extensions = entries.fold([], |types, item| {
		extension = file_type(item.file)
		if (types.contains(extension)) types else types.append(extension)
	})
	roots = extensions.map(|extension| Ui.TreeNode.{ key: group_key(extension), label: group_title(entries, extension), parent: Root })
	leaves = entries.map(|item| Ui.TreeNode.{ key: item.key, label: item.file.name(), parent: ChildOf(group_key(file_type(item.file))) })
	roots.concat(leaves)
}

toggle_grouped : Selection, Bool -> List(Engine.Write(_))
toggle_grouped = |selection, grouped| {
	writes = [cells.grouped.set(grouped), cells.group_selection.set(NoSelection)]
	match (grouped, selection) {
		(Bool.True, Selected(file)) => writes.append(open_group(file))
		_ => writes
	}
}

open_group : File -> Engine.Write(_)
open_group = |file| {
	key = group_key(file_type(file))
	cells.expanded.update(|expanded| expanded.insert(key))
}

expand_group : Ui.TreeExpansion -> List(Engine.Write(_))
expand_group = |{ key, expansion }| [
	cells.expanded.update(|expanded| match expansion {
		Expanded => expanded.insert(key)
		Collapsed => expanded.remove(key)
	}),
]

select_tree : List(Queued), Ui.Selection -> List(Engine.Write(_))
select_tree = |entries, wanted|
	match wanted {
		Selected(key) if key.starts_with("type:") => [cells.group_selection.set(wanted)]
		_ => [cells.group_selection.set(NoSelection)].concat(select(entries, wanted))
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
			cells.group_selection.set(NoSelection),
			cells.expanded.set(Set.empty().insert(group_key(file_type(first.file)))),
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
empty_queue = |action| [
	cells.queue.set([]),
	cells.selected.set(NoSelection),
	cells.group_selection.set(NoSelection),
	cells.expanded.set(Set.empty()),
	cells.preview_bytes.set(NoPreview),
	cells.copy_status.set(""),
	change(action),
]
