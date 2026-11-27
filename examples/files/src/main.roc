app [main] { logic: "../logic/main.roc", pf: platform "../../../platform/main.roc" }

import logic.Inspector
import pf.Clipboard
import pf.Engine exposing [Elem, File, HandlerOf, Signal]
import pf.Files
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Queued : { key : Str, file : File }

Selection : [None, Selected(File)]

PreviewBytes : [NoPreview, Loaded(List(U8))]

Photo : [Waiting, PreviewReady(Ui.Raster), PreviewRefused(Str)]

WindowEntry := { key : Str, file : Selection }.{
	# A session keeps the same file for the lifetime of its key.
	is_eq : WindowEntry, WindowEntry -> Bool
	is_eq = |first, second| first.key == second.key
}

main_window = WindowEntry.{ key: "main", file: None }

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
		photo: Engine.cell(Waiting),
		photo_mode: Engine.cell("original"),
		contrast: Engine.cell(50.I32),
		divider: Engine.cell(50.I32),
		preview_focus: Engine.cell(Engine.outbox({})),
		format: Engine.cell("text"),
		detail: Engine.cell("preview"),
		grouped: Engine.cell(Bool.False),
		group_selection: Engine.cell(Ui.Selection.NoSelection),
		expanded: Engine.cell(Set.empty()),
		folder: Engine.cell(no_folder),
		pick_status: Engine.cell(""),
		windows: Engine.cell([main_window]),
		next_window: Engine.cell(1.U64),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || Ui.windows(
	Engine.each(
		cells.windows.signal(),
		|entry| entry.key,
		|row| {
			entry = row.item()
			Ui.window(
				{
					title: entry.map(
						|window| match window.file {
							None => Title("Files")
							Selected(file) => Title("Compare ${file.name()}")
						},
					),
					close: Handle(Engine.handle_by(row.key(), |key, _| close_window(key))),
				},
				Engine.when(row.key().map(|key| key == "main"), || main_inspector(), || comparison(entry)),
			)
		},
	),
)

close_window : Str -> List(Engine.Write(_))
close_window = |key| {
	remove = cells.windows.update(|windows| windows.keep_if(|window| window.key != key))
	if key == "main" {
		[remove].concat(reset(Clear))
	} else {
		[remove]
	}
}

main_inspector = || {
	queue = cells.queue.signal()
	selected = cells.selected.signal()
	format = cells.format.signal()
	detail = cells.detail.signal()
	on_files = Engine.handle_by(cells.next_key.signal(), receive)
	selection = { queue, selected }.Signal.map(|{ queue: entries, selected: key }| active(entries, key))
	comparison_input = { selection, windows: cells.windows.signal(), serial: cells.next_window.signal() }.Signal.map(|value| value)
	group_selection = cells.group_selection.signal()
	tree_selected = { selected, group_selection }.Signal.map(
		|value|
			match value.group_selection {
				Selected(_) => value.group_selection
				NoSelection => value.selected
			},
	)
	group_summary = { queue, group_selection }.Signal.map(
		|value|
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
	queue_pane = Ui.file_drop_target(
		{ label: "Drop files here", on_files },
		Ui.vstack(
			{},
			[
				Ui.checkbox(
					"Group by file type",
					{ checked: cells.grouped.signal(), on_change: Engine.handle_by(selection, toggle_grouped) },
				),
				Engine.when(
					cells.grouped.signal(),
					|| Ui.vstack(
						{},
						[
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
						],
					),
					|| Ui.list_box({
						label: "Picked files",
						choices: queue.map(|entries| entries.map(|item| Ui.Choice.{ key: item.key, label: item.file.name() })),
						selected,
						on_select: Engine.handle_by(queue, select),
						visible_rows: 4,
					}),
				),
			],
		),
	)
	details = Ui.vstack(
		{},
		[
			Ui.text_with(display.map(|state| state.name), { style: Heading, overflow: EllipsizeEnd }),
			Ui.text_with(display.map(|state| state.size), { style: Caption, tone: SecondaryText }),
			Engine.when(
				display.map(|state| !state.status.is_empty()),
				|| Ui.text(display.map(|state| state.status)),
				|| Ui.vstack({}, []),
			),
			inspection_tabs(
				detail,
				Engine.handle(|key| [cells.detail.set(key)]),
				|| Engine.when(
					selection.map(is_photo),
					|| photo_preview(),
					|| Ui.vstack(
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
					),
				),
				|| Ui.vstack(
					{},
					[
						Ui.text_with(display.map(|state| state.digest), { style: Monospace }),
						Ui.button(
							"Copy SHA-256",
							{
								enabled: checksum.map(|text| !text.is_empty()),
								on_press: Engine.handle_by(checksum, |text, _| if (text.is_empty()) [] else [cells.copies.update(|outbox| outbox.post(text))]),
							},
						),
						Ui.text(cells.copy_status.signal()),
					],
				),
			),
		],
	)
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{ spacing: 12 },
				[
					Ui.asset_image("files.png", { label: "A document in a folder", width: 36, height: 36, fit: Contain }),
					Ui.text_with("Files", { style: Title }),
				],
			),
			Ui.hstack(
				{ stack_below: 480 },
				[
					Ui.file_button("Choose Files…", { selection: MultipleFiles, on_files }),
					Ui.folder_button("Choose Folder…", { on_folder: Engine.handle(receive_folder) }),
					Ui.button("Clear", { on_press: Engine.handle(|_| reset(Clear)) }),
					Ui.button(
						"Open in New Window",
						{
							enabled: comparison_input.map(
								|current| match current.selection {
									Selected(_) => current.windows.len() == 1
									None => Bool.False
								},
							),
							on_press: Engine.handle_by(
								comparison_input,
								|current, _| match current.selection {
									Selected(file) if current.windows.len() == 1 => [
										cells.windows.update(|windows| windows.append(WindowEntry.{ key: current.serial.to_str(), file: Selected(file) })),
										cells.next_window.set(current.serial + 1),
									]
									_ => []
								},
							),
						},
					),
				],
			),
			Engine.when(
				cells.pick_status.signal().map(|status| !status.is_empty()),
				|| Ui.text_with(cells.pick_status.signal(), { style: Caption, tone: SecondaryText }),
				|| Ui.vstack({}, []),
			),
			Ui.hstack(
				{ stack_below: 552, spacing: 12 },
				[
					Ui.panel({ width: Fill, padding: 0 }, queue_pane),
					Ui.panel({ width: Fill, padding: 10 }, details),
				],
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
				preview_requests,
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
				photo_requests,
				Engine.react(|_, answer| [cells.photo.set(photo_result(answer))]),
			),
			Engine.on_post(
				cells.preview_focus.signal(),
				|_| [Engine.command(Focus("comparison-divider"))],
				Engine.react(|_, _| []),
			),
			Engine.watch_latest(
				selection,
				digest_requests,
				Engine.react(|_, result| [change(Digest(result.map_err(Str.inspect)))]),
			),
		],
	)
}

comparison = |entry| Engine.state(
	{ detail: "preview", display: Inspector.idle, photo: Waiting },
	|local| {
		window = entry.lift()
		current = local.signal()
		detail = current.map(|state| state.detail)
		Ui.vstack(
			{ padding: 16, spacing: 12 },
			[
				Ui.text_with(
					window.map(
						|value| match value.file {
							None => ""
							Selected(file) => file.name()
						},
					),
					{ style: Title, overflow: EllipsizeEnd },
				),
				inspection_tabs(
					detail,
					local.set_value(|state, key| { ..state, detail: key }),
					|| Engine.when(
						window.map(|value| is_photo(value.file)),
						|| original_photo(current.map(|state| state.photo)),
						|| Ui.text_area(current.map(|state| state.display.preview), { label: "Contents" }),
					),
					|| Ui.text_with(current.map(|state| state.display.digest), { style: Monospace }),
				),
				Engine.watch_latest(
					window,
					|value| preview_requests(value.file),
					local.on_result(
						|state, _, result| {
							..state,
							display: Inspector.update(state.display, Preview(result.map_err(Str.inspect))),
						},
					),
				),
				Engine.watch_latest(
					window,
					|value| digest_requests(value.file),
					local.on_result(
						|state, _, result| {
							..state,
							display: Inspector.update(state.display, Digest(result.map_err(Str.inspect))),
						},
					),
				),
				Engine.watch_latest(
					window,
					|value| photo_requests(value.file),
					local.on_result(
						|state, _, result| {
							..state,
							photo: photo_result(result),
						},
					),
				),
			],
		)
	},
)

is_photo : Selection -> Bool
is_photo = |selection| match selection {
	None => Bool.False
	Selected(file) => ["png", "jpeg", "jpg", "bmp"].contains(Inspector.extension(file.name()))
}

photo_requests : Selection -> List(Engine.Ask(Ui.Raster, Ui.RasterError))
photo_requests = |selection| match selection {
	Selected(file) if is_photo(selection) => [Ui.decode_image(file)]
	_ => []
}

photo_result : Try(Ui.Raster, Ui.RasterError) -> Photo
photo_result = |answer| match answer {
	Ok(raster) => PreviewReady(raster)
	Err(problem) => PreviewRefused("Could not preview image: ${Str.inspect(problem)}")
}

photo_preview : () -> Elem(ctx) where [ctx.store : () -> Engine.Reach(ctx, _)]
photo_preview = || {
	photo = cells.photo.signal()
	mode = cells.photo_mode.signal()
	Ui.vstack(
		{},
		[
			Ui.combo_box({
				label: "Image view",
				choices: Engine.const([
					Ui.Choice.{ key: "original", label: "Original" },
					Ui.Choice.{ key: "contrast", label: "Adjust contrast" },
					Ui.Choice.{ key: "compare", label: "Compare to calibration" },
					Ui.Choice.{ key: "calibration", label: "Calibration reference" },
				]),
				selected: mode.map(|key| Selected(key)),
				on_select: Engine.handle(
					|choice| match choice {
						Selected(key) => [cells.photo_mode.set(key)]
						NoSelection => []
					},
				),
			}),
			Engine.switch(
				mode,
				["original", "contrast", "compare", "calibration"],
				|key| match key {
					"contrast" => contrast_photo(photo)
					"compare" => compare_photo(photo)
					"calibration" => Ui.image(
						Engine.const(calibration.map_ok(|raster| RasterImage(raster)) ?? NoImage),
						{ label: "Calibration reference", width: 300, height: 150, fit: Contain },
					)
					_ => original_photo(photo)
				},
			),
		],
	)
}

original_photo : Signal(ctx, Photo) -> Elem(ctx)
original_photo = |photo| Ui.vstack(
	{},
	[
		Ui.image(
			photo.map(
				|current| match current {
					PreviewReady(raster) => RasterImage(raster)
					_ => NoImage
				},
			),
			{ label: "Original image", width: 300, height: 150, fit: Contain },
		),
		Ui.text_with(photo.map(photo_status), { style: Caption, tone: SecondaryText }),
	],
)

photo_status : Photo -> Str
photo_status = |current| match current {
	Waiting => "Decoding image…"
	PreviewReady(_) => "Image ready"
	PreviewRefused(message) => message
}

photo_texture : Photo -> Ui.ShaderTexture
photo_texture = |current| match current {
	PreviewReady(raster) => RasterTexture(raster)
	_ => NoTexture
}

contrast_photo : Signal(ctx, Photo) -> Elem(ctx) where [ctx.store : () -> Engine.Reach(ctx, _)]
contrast_photo = |photo| {
	value = cells.contrast.signal()
	Ui.vstack(
		{},
		[
			Ui.shader_view({
				label: "Contrast-adjusted image",
				shader: "scan-contrast.cso",
				width: 300,
				height: 150,
				texture: photo.map(photo_texture),
				parameters: value.map(|percent| [{ name: "contrast", value: percent.to_f32() / 50.F32 }]),
			}),
			Ui.slider(value, { label: "Contrast", on_change: Engine.handle(|percent| [cells.contrast.set(percent)]) }),
			Ui.text_with(photo.map(photo_status), { style: Caption, tone: SecondaryText }),
		],
	)
}

compare_photo : Signal(ctx, Photo) -> Elem(ctx) where [ctx.store : () -> Engine.Reach(ctx, _)]
compare_photo = |photo| Engine.state(
	Bool.False,
	|drag| {
		photo_at_surface = photo.lift()
		divider = cells.divider.signal()
		on_press = Engine.handle(
			|pointer| {
				if pointer.button == Main {
					[cells.preview_focus.update(|outbox| outbox.post({}))].concat(divider_position(pointer))
				} else {
					[]
				}
			},
		).also(drag.set_value(|_, pointer| pointer.button == Main))
		on_move = Engine.handle_by(
			drag.signal(),
			|dragging, pointer| {
				if (dragging) divider_position(pointer) else []
			},
		)
		stop = drag.update(|_| Bool.False)
		Ui.vstack(
			{},
			[
				Ui.shader_view({
					label: "Image comparison",
					shader: "image-compare.cso",
					width: 300,
					height: 150,
					texture: photo_at_surface.map(photo_texture),
					second_texture: Engine.const(calibration.map_ok(|raster| RasterTexture(raster)) ?? NoTexture),
					parameters: divider.map(|percent| [{ name: "divider", value: percent.to_f32() / 100.F32 }]),
					input: Interactive({
						pointer: Pointer({
							zone: "calibration-comparison",
							down: Handle(on_press),
							move: Handle(on_move),
							up: Handle(stop),
							cancel: Handle(stop),
							capture: CaptureOnPress,
							move_policy: LatestMove,
						}),
					}),
				}),
				Ui.slider(
					divider,
					{
						label: "Comparison divider",
						reference: "comparison-divider",
						on_change: Engine.handle(|percent| [cells.divider.set(percent)]),
					},
				),
				Ui.text_with(divider.map(|percent| "Comparison divider: ${percent.to_str()}%"), { style: Caption, tone: SecondaryText }),
			],
		)
	},
)

divider_position : Engine.Pointer -> List(Engine.Write(_))
divider_position = |pointer| match pointer.at {
	In({ zone: "calibration-comparison", x, .. }) =>
		[cells.divider.set((x.to_i32() + 50) // 100)]
	_ => []
}

calibration : Try(Ui.Raster, Ui.RasterError)
calibration = {
	width = 80
	height = 40
	colors = [[16, 32, 56, 255], [34, 86, 111, 255], [40, 126, 140, 255], [222, 129, 82, 255], [246, 222, 170, 255]]
	row = List.join(colors.map(|color| List.join(List.repeat(color, 16))))
	Ui.raster({ width, height, rgba: List.join(List.repeat(row, height.to_u64())) })
}

inspection_tabs : Signal(ctx, Str), HandlerOf(ctx, Str), (() -> Elem(ctx)), (() -> Elem(ctx)) -> Elem(ctx)
inspection_tabs = |detail, on_select, preview, fingerprint| Ui.tabs(
	{
		label: "File details",
		tabs: [
			Ui.Choice.{ key: "preview", label: "Preview" },
			Ui.Choice.{ key: "fingerprint", label: "Fingerprint" },
		],
		selected: detail,
		on_select,
	},
	Engine.switch(detail, ["preview", "fingerprint"], |key| if (key == "preview") preview() else fingerprint()),
)

preview_requests = |current| match current {
	Selected(file) if !is_photo(current) =>
		match file.slice({ start: 0, len: file.size().min(Inspector.preview_limit) }) {
			Ok(head) => [head.read_bytes({ max_bytes: Inspector.preview_limit })]
			Err(_) => []
		}
	_ => []
}

digest_requests = |current| match current {
	Selected(file) => [file.sha256()]
	None => []
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
	extensions = entries.fold(
		[],
		|types, item| {
			extension = file_type(item.file)
			if (types.contains(extension)) types else types.append(extension)
		},
	)
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
	cells.expanded.update(
		|expanded| match expansion {
			Expanded => expanded.insert(key)
			Collapsed => expanded.remove(key)
		},
	),
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
	[cells.selected.set(selection), cells.preview_bytes.set(NoPreview), cells.photo.set(Waiting), cells.copy_status.set(""), change(action)]
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
	cells.photo.set(Waiting),
	cells.copy_status.set(""),
	change(action),
]
