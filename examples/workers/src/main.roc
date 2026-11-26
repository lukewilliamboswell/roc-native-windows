app [main, native] { pf: platform "../../../platform/main.roc" }

import Stock
import StockStyle
import pf.Engine exposing [Elem, Signal]
import pf.Win
import pf.Worker

main : Engine.Program(_)
main = Engine.program_store({}, |_| store, |_| view()).with_workers([reconcile.registration()])

native = Win.Features.use([Win.Features.direct3d!, Win.Features.workers!, Win.Features.grid!])

reconcile : Worker.Task(List(Stock.Count), Stock.Summary, Worker.Never)
reconcile = Worker.task({
	name: "roc-native-windows.example/workers/reconcile",
	run!: |counts| Ok(Stock.reconcile(counts)),
})

inventory =
	match Engine.Table.from_list([
		{ key: 1.U64, row: Stock.new("Paper", 4) },
		{ key: 2.U64, row: Stock.new("Labels", 8) },
		{ key: 3.U64, row: Stock.new("Folders", 3) },
		{ key: 4.U64, row: Stock.new("Notebooks", 12) },
	]) {
		Ok(table) => table
		Err(DuplicateKey(key)) => crash "Duplicate inventory key: ${key.to_str()}"
	}

Query : { reverse : Bool, mismatches : Bool }

Job : { serial : U64, running : Bool, counts : List(Stock.Count) }

initial_job : Job
initial_job = { serial: 0, running: Bool.False, counts: [] }

Variance : [NoSnapshot, Measured({ total : U64, different : U64 })]

store = Engine.store(
	Inventory,
	{
		counts: Engine.table(inventory),
		reverse: Engine.cell(Bool.False),
		mismatches: Engine.cell(Bool.False),
		second_page: Engine.cell(Bool.False),
		job: Engine.cell(initial_job),
		variance: Engine.cell(NoSnapshot),
		status: Engine.cell("Edit a count, then reconcile a snapshot."),
	}.Engine.Cells,
)

cells = store.cells()

plan : Query -> Engine.QueryPlan(U64, Stock.Item, (U64, U64))
plan = |query| Engine.sort_by({
	keep: |_, item| keep_item(query, item),
	sort: |key, _| rank(query, key),
})

keep_item : Query, Stock.Item -> Bool
keep_item = |query, item| !query.mismatches or item.counted != item.expected

rank : Query, U64 -> U64
rank = |query, key| if (query.reverse) 4 - key else key

paste_error : Win.Ui.GridPasteError -> Str
paste_error = |problem| match problem {
	Unavailable => "The clipboard does not contain readable text."
	Malformed => "Paste a rectangular set of tab-separated values."
	TooLarge => "The paste is too large."
	OutsideGrid => "The paste does not fit the mounted grid."
	SelectionMismatch => "The pasted cells must match the selected rectangle."
	ReadOnlyCell => "The paste includes a read-only cell."
}

paste_updates = |table, query, page, batch| {
	ordered = table.to_list().keep_if(|entry| keep_item(query, entry.row)).sort_with(
		|left, right| {
			a = rank(query, left.key)
			b = rank(query, right.key)
			if (a < b) Before else if (a > b) After else Same
		},
	)
	visible = ordered.drop_first(if (page) 3 else 0).take_first(3)
	replacements = Stock.paste(visible, batch)?
	Ok(replacements.fold(table, |next, change| next.update(change.key, |_| change.row)))
}

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	design = StockStyle.{}
	query = { reverse: cells.reverse.signal(), mismatches: cells.mismatches.signal() }.Signal.map(|value| value)
	page = cells.second_page.signal()
	window = page.map(|second| { start: if (second) 3 else 0, count: 3 })
	job = cells.job.signal()
	paste_context = { table: cells.counts.signal(), query, page }.Signal.map(|value| value)
	content = Win.Ui.vstack(
		design.page,
		[
			Win.Ui.hstack(
				design.commands,
				[
					Win.Ui.asset_image(Engine.const(Win.asset("app-art")), { description: Decorative, width: 28, height: 28 }),
					Win.Ui.text_with("Stock count reconciliation", design.title),
				],
			),
			Win.Ui.text_with("Edit a Counted cell with Enter or F2. Hold Shift and use the arrows to select a range for Ctrl+V.", design.metadata),
			Win.Ui.hstack(
				design.commands,
				[
					Win.Ui.button("Reverse order", { on_press: Engine.handle(|_| [cells.reverse.update(|reverse| !reverse)]) }),
					Win.Ui.button(
						cells.mismatches.signal().map(|only| if (only) "All items" else "Mismatches only"),
						{ on_press: Engine.handle(|_| [cells.mismatches.update(|only| !only), cells.second_page.set(Bool.False)]) },
					),
					Win.Ui.button(
						page.map(|second| if (second) "First page" else "Next page"),
						{
							enabled: cells.mismatches.signal().map(|only| !only),
							on_press: Engine.handle(|_| [cells.second_page.update(|second| !second)]),
						},
					),
				],
			),
			Win.Ui.data_grid(
				{
					label: "Stock counts",
					columns: [
						{ key: "item", heading: "Product", width: Fixed(140) },
						{ key: "expected", heading: "Expected", width: Fixed(90) },
						{ key: "counted", heading: "Counted", width: Fixed(90) },
					],
					visible_rows: 3,
					on_paste: Handle(
						Engine.handle_by(
							paste_context,
							|state, result| {
								updates = match result {
									Ok(batch) => paste_updates(state.table, state.query, state.page, batch)
									Err(problem) => Err(paste_error(problem))
								}
								match updates {
									Ok(table) => [cells.counts.set(table), cells.status.set("Pasted counts into the mounted grid.")]
									Err(message) => [cells.status.set(message)]
								}
							},
						),
					),
				},
				Engine.table_view(
					cells.counts,
					{ query, plan, window },
					|_, row| {
						value = row.signal()
						Win.Ui.grid_row([
							Win.Ui.grid_cell(value.map(|item| item.name), { column: "item" }),
							Win.Ui.grid_cell(value.map(|item| item.expected.to_str()), { column: "expected" }),
							Win.Ui.grid_cell(
								value.map(|item| item.counted.to_str()),
								{
									column: "counted",
									editor: TextEditor(row.set_value(Stock.recount)),
									feedback: value.map(|item| item.feedback),
								},
							),
						])
					},
				),
			),
			Win.Ui.hstack(
				design.commands,
				[
					Win.Ui.button(
						"Reconcile",
						{
							on_press: Engine.handle_by(
								cells.counts.signal(),
								|table, _| {
									snapshot = table.fold([], |counts, _, item| counts.append(Stock.count(item)))
									[
										cells.job.update(|before| { serial: before.serial + 1, running: Bool.True, counts: snapshot }),
										cells.status.set("Reconciling submitted counts…"),
									]
								},
							),
						},
					),
					Win.Ui.button(
						"Cancel",
						{
							enabled: job.map(|run| run.running),
							on_press: Engine.handle(|_| [cells.job.update(|run| { ..run, running: Bool.False }), cells.status.set("Reconciliation cancelled.")]),
						},
					),
				],
			),
			Win.Ui.panel(
				design.results_panel,
				Win.Ui.vstack(
					design.results,
					[
						Win.Ui.status_text(cells.status.signal(), { text: design.summary }),
						variance_meter(design, cells.variance.signal()),
					],
				),
			),
			Engine.watch_latest(
				job,
				|run| if (run.running) [reconcile.ask(run.counts)] else [],
				Engine.react(
					|submitted, result| {
						finish = cells.job.update(|run| { ..run, running: Bool.False })
						match result {
							Ok(totals) => [
								finish,
								cells.status.set("Submitted snapshot: ${totals.matched.to_str()} matched, ${totals.short.to_str()} short, ${totals.surplus.to_str()} surplus."),
								cells.variance.set(Measured({ total: submitted.counts.len(), different: submitted.counts.len() - totals.matched })),
							]
							Err(error) => [finish, cells.status.set("Worker failed: ${Str.inspect(error)}")]
						}
					},
				),
			),
		],
	)
	Win.Ui.window({ appearance: design.appearance }, content)
}

variance_meter : StockStyle, Signal(ctx, Variance) -> Elem(ctx)
variance_meter = |design, variance| Engine.when(
	variance.map(|value| value != NoSnapshot),
	|| Win.Ui.vstack(
		design.results,
		[
			Win.Ui.text_with(
				variance.map(
					|value| match value {
						NoSnapshot => ""
						Measured({ total, different }) => "${different.to_str()} of ${total.to_str()} submitted lines differ"
					},
				),
				design.metric,
			),
			Win.Ui.shader_view({
				description: Described({ label: "Share of submitted lines with a count variance" }),
				shader: Win.asset("stock-variance"),
				width: 264,
				height: 12,
				parameters: variance.map(
					|value| [
						{
							name: "mismatch",
							value: match value {
								NoSnapshot => 0.F32
								Measured({ total, different }) => different.to_f32() / total.max(1).to_f32()
							},
						},
					],
				),
			}),
		],
	),
	|| Win.Ui.vstack({}, []),
)

expect Engine.mounts(main) > 0
