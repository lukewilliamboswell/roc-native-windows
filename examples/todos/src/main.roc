app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, Signal]
import pf.Ui

Todo : { id : U64, title : Str, done : Bool }

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

store = Engine.store(
	Todos,
	{
		draft: Engine.cell(""),
		next_id: Engine.cell(1.U64),
		todos: Engine.cell([]),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	draft = cells.draft.signal()
	todos = cells.todos.signal()
	new_todo = { draft, id: cells.next_id.signal() }.Signal.map(|{ draft: text, id }| { id, title: text.trim(), done: Bool.False })
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.text(todos.map(remaining)),
			Ui.hstack(
				{},
				[
					Ui.text_field(draft, { label: "New to-do", placeholder: "What needs doing?", on_input: Engine.handle(|text| [cells.draft.set(text)]) }),
					Ui.button("Add", { enabled: new_todo.map(|t| !t.title.is_empty()), on_press: Engine.handle_by(new_todo, |todo, _| add(todo)) }),
				],
			),
			Engine.each(
				todos,
				|todo| todo.id.to_str(),
				|row| {
					item = row.item()
					Ui.hstack(
						{},
						[
							Ui.checkbox(
								item.map(|todo| todo.title),
								{ checked: item.map(|todo| todo.done), on_change: Engine.handle_by(item, |todo, done| [cells.todos.update(|all| set_done(all, todo.id, done))]) },
							),
							Ui.button("Remove", { on_press: Engine.handle_by(item, |todo, _| [cells.todos.update(|all| all.keep_if(|t| t.id != todo.id))]) }),
						],
					)
				},
			),
		],
	)
}

add : Todo -> List(Engine.Write(_))
add = |todo|
	if todo.title.is_empty() {
		[]
	} else {
		[cells.todos.update(|all| all.append(todo)), cells.next_id.set(todo.id + 1), cells.draft.set("")]
	}

set_done : List(Todo), U64, Bool -> List(Todo)
set_done = |todos, id, done|
	todos.map(|todo| if (todo.id == id) { ..todo, done } else todo)

remaining : List(Todo) -> Str
remaining = |todos| {
	count = todos.keep_if(|todo| !todo.done).len()
	"${count.to_str()} remaining"
}

expect remaining([]) == "0 remaining"
expect set_done([{ id: 1, title: "Write a list", done: Bool.False }], 1, Bool.True) == [{ id: 1, title: "Write a list", done: Bool.True }]
