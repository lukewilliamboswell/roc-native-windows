app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, Signal]
import pf.Ui
import TaskList exposing [Task]

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

store = Engine.store(
	Todos,
	{
		draft: Engine.cell(""),
		due_date: Engine.cell(NoDate),
		due_time: Engine.cell(NoTime),
		next_id: Engine.cell(1.U64),
		todos: Engine.cell([]),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	draft = cells.draft.signal()
	due_date = cells.due_date.signal()
	due_time = cells.due_time.signal()
	todos = cells.todos.signal()
	new_todo = { draft, due_date, due_time, id: cells.next_id.signal() }.Signal.map(
		|{ draft: text, due_date: date, due_time: time, id }| { id, title: text.trim(), done: Bool.False, due_date: date, due_time: time },
	)
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{ spacing: 12 },
				[
					Ui.asset_image("todos.bmp", { label: "A three-item checklist", width: 48, height: 48 }),
					Ui.text("To-do list"),
				],
			),
			Ui.text(todos.map(TaskList.remaining)),
			Ui.aggregate_checkbox(
				"Complete all visible tasks",
				{
					state: todos.map(TaskList.completion),
					enabled: todos.map(|all| !all.is_empty()),
					on_change: Engine.handle(|done| [cells.todos.update(|all| all.map(|task| { ..task, done }))]),
				},
			),
			Ui.hstack(
				{},
				[
					Ui.text_field(draft, { label: "New to-do", placeholder: "What needs doing?", on_input: Engine.handle(|text| [cells.draft.set(text)]) }),
					Ui.button("Add", { enabled: new_todo.map(|t| !t.title.is_empty()), on_press: Engine.handle_by(new_todo, |todo, _| add(todo)) }),
				],
			),
			Ui.hstack(
				{},
				[
					Ui.date_picker(due_date, { label: "New due date", on_change: Engine.handle(draft_date) }),
					Ui.time_picker(due_time, { label: "New due time", enabled: due_date.map(TaskList.has_due_date), on_change: Engine.handle(|time| [cells.due_time.set(time)]) }),
				],
			),
			Engine.each(
				todos.map(TaskList.ordered),
				|todo| todo.id.to_str(),
				|row| todo_row(row.item()),
			),
		],
	)
}

todo_row = |item| {
	date = item.map(|todo| todo.due_date)
	Ui.vstack(
		{ spacing: 4, padding: 4 },
		[
			Ui.hstack(
				{},
				[
					Ui.checkbox(
						item.map(|todo| todo.title),
						{ checked: item.map(|todo| todo.done), on_change: Engine.handle_by(item, |todo, done| update(todo.id, |task| { ..task, done })) },
					),
					Ui.button("Remove", { on_press: Engine.handle_by(item, |todo, _| [cells.todos.update(|all| all.keep_if(|t| t.id != todo.id))]) }),
				],
			),
			Ui.hstack(
				{},
				[
					Ui.date_picker(
						date,
						{
							label: item.map(|todo| "Due date for ${todo.title}"),
							on_change: Engine.handle_by(item, |todo, value| update(todo.id, |task| TaskList.with_date(task, value))),
						},
					),
					Ui.time_picker(
						item.map(|todo| todo.due_time),
						{
							label: item.map(|todo| "Due time for ${todo.title}"),
							enabled: date.map(TaskList.has_due_date),
							on_change: Engine.handle_by(item, |todo, value| update(todo.id, |task| TaskList.with_time(task, value))),
						},
					),
				],
			),
			Ui.text(item.map(TaskList.summary)),
		],
	)
}

draft_date : Ui.DateSelection -> List(Engine.Write(_))
draft_date = |date|
	if TaskList.has_due_date(date) {
		[cells.due_date.set(date)]
	} else {
		[cells.due_date.set(NoDate), cells.due_time.set(NoTime)]
	}

add : Task -> List(Engine.Write(_))
add = |todo|
	if todo.title.is_empty() {
		[]
	} else {
		[
			cells.todos.update(|all| all.append(todo)),
			cells.next_id.set(todo.id + 1),
			cells.draft.set(""),
			cells.due_date.set(NoDate),
			cells.due_time.set(NoTime),
		]
	}

update : U64, (Task -> Task) -> List(Engine.Write(_))
update = |id, change| [cells.todos.update(|all| TaskList.update(all, id, change))]
