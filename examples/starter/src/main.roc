app [main] { logic: "../logic/main.roc", pf: platform "../../../platform/main.roc" }

import logic.ReadingList
import pf.Engine exposing [Elem, Signal]
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_| store, |_| view())

store = Engine.store(
	ReadingRoom,
	{
		draft: Engine.cell(""),
		items: Engine.cell(ReadingList.empty),
		filter: Engine.cell(Ui.Selection.Selected("all")),
		selected: Engine.cell(Ui.Selection.NoSelection),
		sorted: Engine.cell(Bool.False),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	draft = cells.draft.signal()
	items = cells.items.signal()
	filter = cells.filter.signal()
	visible_items = { items, filter }.Signal.map(
		|{ items: all, filter: choice }|
			match choice {
				Selected("unread") => all.keep_if(|item| !item.read)
				Selected("read") => all.keep_if(|item| item.read)
				_ => all
			},
	)
	selected = cells.selected.signal()
	rows = { items: visible_items, sorted: cells.sorted.signal() }.Signal.map(|state|
		if (state.sorted) state.items.sort_with(|a, b| compare_title(a.title.to_utf8(), b.title.to_utf8())) else state.items)
	active = { items: visible_items, selected }.Signal.map(
		|state| match state.selected {
			NoSelection => Err(NoTitle)
			Selected(key) => state.items.find_first(|item| item.title == key).map_err(|_| NoTitle)
		},
	)
	content = Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{},
				[
					Ui.asset_image("reading-list.bmp", { label: "A checklist", width: 48, height: 48 }),
					Ui.text("Reading list"),
				],
			),
			Ui.hstack(
				{},
				[
					Ui.text_field(draft, { label: "Title", placeholder: "Add something to read", on_input: Engine.handle(|text| [cells.draft.set(text)]) }),
					Ui.button("Add", { enabled: draft.map(|text| !text.trim().is_empty()), on_press: Engine.handle_by(draft, |title, _| [change(Add(title)), cells.selected.set(Selected(title.trim())), cells.draft.set("")]) }),
				],
			),
			Ui.combo_box({
				label: "Status filter",
				choices: Engine.const([
					Ui.Choice.{ key: "all", label: "All statuses" },
					Ui.Choice.{ key: "unread", label: "Unread" },
					Ui.Choice.{ key: "read", label: "Read" },
				]),
				selected: filter,
				on_select: Engine.handle(|value| [cells.filter.set(value)]),
			}),
			Ui.toolbar(
				{ label: "Reading list actions" },
				[
					Ui.toolbar_button(
						active.map(
							|result| match result {
								Ok(item) if item.read => "Mark unread"
								_ => "Mark read"
							},
						),
						{
							key: "mark",
							enabled: active.map(Try.is_ok),
							on_press: Engine.handle_by(
								active,
								|result, _| match result {
									Ok(item) => [change(Toggle(item.title))]
									Err(_) => []
								},
							),
						},
					),
					Ui.toolbar_button(
						"Remove",
						{
							key: "remove",
							enabled: active.map(Try.is_ok),
							on_press: Engine.handle_by(
								active,
								|result, _| match result {
									Ok(item) => [change(Remove(item.title)), cells.selected.set(NoSelection)]
									Err(_) => []
								},
							),
						},
					),
				],
			),
			Ui.list_view(
				{
					label: "Titles",
					columns: [{ key: "title", heading: "Title" }, { key: "status", heading: "Status" }],
					selected,
					on_select: Engine.handle(|selection| [cells.selected.set(selection)]),
					on_activate: Handle(Engine.handle(|key| [change(Toggle(key))])),
					on_column_press: Handle(Engine.handle(|key| if (key == "title") [cells.sorted.update(|sorted| !sorted)] else [])),
				},
				[
					Engine.each(
						rows,
						|item| item.title,
						|row| {
							item = row.item()
							title = item.map(|entry| entry.title)
							Ui.list_row(
								{ key: title },
								[
									Ui.list_cell(title, { column: "title" }),
									Ui.list_cell(item.map(|entry| if (entry.read) "Read" else "Unread"), { column: "status" }),
								],
							)
						},
					),
				],
			),
		],
	)
	visible_count = visible_items.map(|list| "${list.len().to_str()} visible")
	unread_count = items.map(|list| "${ReadingList.remaining(list).to_str()} unread")
	Ui.window({ status_bar: StatusBar({ text: visible_count, trailing: [{ text: unread_count }] }) }, content)
}

change : ReadingList.Action -> Engine.Write(_)
change = |action| cells.items.update(|items| ReadingList.update(items, action))

compare_title : List(U8), List(U8) -> [Before, Same, After]
compare_title = |left, right| match (left, right) {
	([], []) => Same
	([], [_, ..]) => Before
	([_, ..], []) => After
	([a, .. as tail_left], [b, .. as tail_right]) =>
		if (a < b) Before else if (a > b) After else compare_title(tail_left, tail_right)
}
