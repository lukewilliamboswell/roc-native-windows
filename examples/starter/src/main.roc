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
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	draft = cells.draft.signal()
	items = cells.items.signal()
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{},
				[
					Ui.asset_image("reading-list.bmp", { label: "A checklist", width: 48, height: 48 }),
					Ui.text("Reading list"),
				],
			),
			Ui.text(items.map(|list| "${ReadingList.remaining(list).to_str()} unread")),
			Ui.hstack(
				{},
				[
					Ui.text_field(draft, { label: "Title", placeholder: "Add something to read", on_input: Engine.handle(|text| [cells.draft.set(text)]) }),
					Ui.button("Add", { enabled: draft.map(|text| !text.trim().is_empty()), on_press: Engine.handle_by(draft, |title, _| [change(Add(title)), cells.draft.set("")]) }),
				],
			),
			Engine.each(
				items,
				|item| item.title,
				|row| {
					item = row.item()
					Ui.hstack(
						{},
						[
							Ui.checkbox(item.map(|entry| entry.title), { checked: item.map(|entry| entry.read), on_change: Engine.handle_by(item, |entry, _| [change(Toggle(entry.title))]) }),
							Ui.button("Remove", { on_press: Engine.handle_by(item, |entry, _| [change(Remove(entry.title))]) }),
						],
					)
				},
			),
		],
	)
}

change : ReadingList.Action -> Engine.Write(_)
change = |action| cells.items.update(|items| ReadingList.update(items, action))
