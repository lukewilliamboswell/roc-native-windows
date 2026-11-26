app [main, native] {
	pf: platform "../../../platform/main.roc",
	time: "https://github.com/lukewilliamboswell/roc-time/releases/download/0.4.0/roc-time-5uLL89o7nJvrtgHt1NamZMgjp76Tb5AaoMzzE8F2xfqD.tar.zst",
}

import pf.Engine exposing [Elem, Signal]
import pf.Win
import TaskList exposing [Task]

main : Engine.Program(_)
main = Engine.program_store({ zone_days: 366 }, |boot| store.with_initial(cells.clock, { now: boot.now_ms, zone: boot.zone }), |_boot| view())

native = Win.Features.use([Win.Features.image_codecs!])

Clock : { now : I64, zone : Engine.Zone }

default_clock : Clock
default_clock = { now: 0, zone: Engine.utc }

clock_interval = 60_000.U32

NoteEditor : [Closed, Editing({ id : U64, title : Str, draft : Str })]

closed_editor : NoteEditor
closed_editor = Closed

store = Engine.store(
	Todos,
	{
		draft: Engine.cell(""),
		clock: Engine.cell(default_clock),
		due_date: Engine.cell(NoDate),
		due_time: Engine.cell(NoTime),
		next_id: Engine.cell(1.U64),
		todos: Engine.cell([]),
		note_editor: Engine.cell(closed_editor),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	draft = cells.draft.signal()
	due_date = cells.due_date.signal()
	due_time = cells.due_time.signal()
	todos = cells.todos.signal()
	clock = cells.clock.signal()
	outside = clock.map(|sample| if (sample.zone.offset_at(sample.now).is_err()) [sample.now] else [])
	new_todo = { draft, due_date, due_time, id: cells.next_id.signal() }.Signal.map(
		|{ draft: text, due_date: date, due_time: time, id }| { id, title: text.trim(), done: Bool.False, notes: "", due_date: date, due_time: time },
	)
	content = Win.Ui.vstack(
		{ padding: 20, spacing: 12 },
		[
			Win.Ui.hstack(
				{ spacing: 12 },
				[
					Win.Ui.asset_image(Engine.const(Win.asset("app-art")), { description: Decorative, width: 32, height: 32 }),
					Win.Ui.text_with("To-do list", { style: Title }),
				],
			),
			Win.Ui.hstack(
				{ spacing: 16, stack_below: 304 },
				[
					Win.Ui.text_with(todos.map(TaskList.remaining), { style: Heading, tone: AccentText }),
					Win.Ui.aggregate_checkbox(
						"Complete all visible tasks",
						{
							state: todos.map(TaskList.completion),
							enabled: todos.map(|all| !all.is_empty()),
							on_change: Engine.handle(|done| [cells.todos.update(|all| all.map(|task| { ..task, done }))]),
						},
					),
				],
			),
			Win.Ui.panel(
				{ surface: AccentTint, border: Outline, width: Fill },
				Win.Ui.vstack(
					{ spacing: 8 },
					[
						Win.Ui.hstack(
							{ stack_below: 280 },
							[
								Win.Ui.text_field(draft, { label: "New to-do", placeholder: "What needs doing?", on_input: Engine.handle(|text| [cells.draft.set(text)]) }),
								Win.Ui.button("Add", { enabled: new_todo.map(|t| !t.title.is_empty()), on_press: Engine.handle_by(new_todo, |todo, _| add(todo)) }),
							],
						),
						Win.Ui.hstack(
							{ stack_below: 420 },
							[
								Win.Ui.date_field(due_date, { label: "New due date", on_change: Engine.handle(draft_date) }),
								Win.Ui.time_field(due_time, { label: "New due time", enabled: due_date.map(TaskList.has_due_date), on_change: Engine.handle(|time| [cells.due_time.set(time)]) }),
							],
						),
					],
				),
			),
			notes_panel(),
			Engine.each(
				todos.map(TaskList.ordered),
				|todo| todo.id.to_str(),
				|row| todo_row(row.item()),
			),
			Engine.watch(
				Engine.const({}),
				|_| [Engine.interval(clock_interval)],
				Engine.react(
					|_, sample| match sample {
						Ok(now) => [cells.clock.update(|prior| { ..prior, now })]
						Err(_) => []
					},
				),
			),
			Engine.watch_latest(
				outside,
				|samples| samples.map(|_| Engine.environment({})),
				Engine.react(
					|_, outcome| match outcome {
						Ok(facts) => [cells.clock.set({ now: facts.now_ms, zone: facts.zone })]
						Err(_) => []
					},
				),
			),
		],
	)
	Win.Ui.window({ appearance: { accent: CustomAccent({ red: 35, green: 106, blue: 84 }) } }, content)
}

todo_row = |item| {
	today = cells.clock.signal().map(|sample| TaskList.project_date(sample.zone, sample.now))
	date = item.map(|todo| todo.due_date)
	detail = { item, today }.Signal.map(|reading| TaskList.due_summary(reading.item, reading.today))
	content = Win.Ui.vstack(
		{ spacing: 6 },
		[
			Win.Ui.hstack(
				{ stack_below: 400 },
				[
					Win.Ui.checkbox(
						item.map(|todo| todo.title),
						{ checked: item.map(|todo| todo.done), on_change: Engine.handle_by(item, |todo, done| update(todo.id, |task| { ..task, done })) },
					),
					Win.Ui.button(
						item.map(|todo| "Notes for ${todo.title}"),
						{ on_press: Engine.handle_by(item, |todo, _| [cells.note_editor.set(Editing({ id: todo.id, title: todo.title, draft: todo.notes }))]) },
					),
					Win.Ui.button("Remove", { on_press: Engine.handle_by(item, |todo, _| remove(todo.id)) }),
				],
			),
			Win.Ui.hstack(
				{ stack_below: 420 },
				[
					Win.Ui.date_field(
						date,
						{
							label: item.map(|todo| "Due date for ${todo.title}"),
							on_change: Engine.handle_by(item, |todo, value| update(todo.id, |task| TaskList.with_date(task, value))),
						},
					),
					Win.Ui.time_field(
						item.map(|todo| todo.due_time),
						{
							label: item.map(|todo| "Due time for ${todo.title}"),
							enabled: date.map(TaskList.has_due_date),
							on_change: Engine.handle_by(item, |todo, value| update(todo.id, |task| TaskList.with_time(task, value))),
						},
					),
				],
			),
			Win.Ui.text_with(detail, { style: Caption, tone: SecondaryText }),
		],
	)
	Win.Ui.panel({ border: Outline, width: Fill, padding: 10 }, content)
}

notes_panel : () -> Elem(Engine.StoreRoot(_))
notes_panel = || {
	editor = cells.note_editor.signal()
	is_open = editor.map(
		|current| match current {
			Closed => Bool.False
			Editing(_) => Bool.True
		},
	)
	title = editor.map(
		|current| match current {
			Closed => ""
			Editing(fields) => "Notes for ${fields.title}"
		},
	)
	draft = editor.map(
		|current| match current {
			Closed => ""
			Editing(fields) => fields.draft
		},
	)
	Win.Ui.dialog(
		{ title, open: is_open, on_close_request: Engine.handle(|_| [cells.note_editor.set(Closed)]) },
		Win.Ui.panel(
			{ surface: AccentTint, width: Fill },
			Win.Ui.vstack(
				{},
				[
					Win.Ui.text_area(draft, { label: "Notes", rows: 4, mode: Editable(Engine.handle(write_note)) }),
					Win.Ui.hstack(
						{},
						[
							Win.Ui.button("Save notes", { enter_action: WindowDefault, on_press: Engine.handle_by(editor, |current, _| save_note(current)) }),
							Win.Ui.button("Cancel", { on_press: Engine.handle(|_| [cells.note_editor.set(Closed)]) }),
						],
					),
				],
			),
		),
	)
}

write_note : Str -> List(Engine.Write(_))
write_note = |draft| [
	cells.note_editor.update(
		|current| match current {
			Closed => Closed
			Editing(fields) => Editing({ ..fields, draft })
		},
	),
]

save_note : NoteEditor -> List(Engine.Write(_))
save_note = |current|
	match current {
		Closed => []
		Editing(fields) => update(fields.id, |task| { ..task, notes: fields.draft }).append(cells.note_editor.set(Closed))
	}

remove : U64 -> List(Engine.Write(_))
remove = |id| [
	cells.todos.update(|all| all.keep_if(|task| task.id != id)),
	cells.note_editor.update(
		|current| match current {
			Editing(fields) if fields.id == id => Closed
			_ => current
		},
	),
]

draft_date : Win.Ui.DateSelection -> List(Engine.Write(_))
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
