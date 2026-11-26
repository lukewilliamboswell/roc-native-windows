app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, Signal]
import pf.Ui
import pf.Worker

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view()).with_workers([squares.registration()])

squares : Worker.Task(U64, U64, Worker.Never)
squares = Worker.task({ name: "roc-native-windows.example/workers/squares", run!: |count| Ok(sum_squares(count)) })

sum_squares : U64 -> U64
sum_squares = |count|
	(0.U64).until(count).fold(0, |sum, value| (sum + value * value) % 1_000_000_007)

store = Engine.store(
	Workers,
	{
		serial: Engine.cell(0.U64),
		running: Engine.cell(Bool.False),
		status: Engine.cell("Press Compute to start a worker."),
		clicks: Engine.cell(0.U64),
	}.Engine.Cells,
)

cells = store.cells()

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	running = cells.running.signal()
	job = { active: running, serial: cells.serial.signal() }.Signal.map(|value| value)
	Ui.vstack(
		{ padding: 16, spacing: 12 },
		[
			Ui.hstack(
				{ spacing: 12 },
				[
					Ui.asset_image("workers.bmp", { label: "Work splitting into two threads", width: 48, height: 48 }),
					Ui.text("Workers"),
				],
			),
			Ui.text(cells.status.signal()),
			Ui.hstack(
				{},
				[
					Ui.button("Compute", { on_press: Engine.handle(|_| [cells.serial.update(|n| n + 1), cells.running.set(Bool.True), cells.status.set("Computing…")]) }),
					Ui.button("Cancel", { enabled: running, on_press: Engine.handle(|_| [cells.running.set(Bool.False), cells.status.set("Cancelled.")]) }),
				],
			),
			Ui.button("Tap while computing", { on_press: Engine.handle(|_| [cells.clicks.update(|n| n + 1)]) }),
			Ui.text(cells.clicks.signal().map(|n| "Taps: ${n.to_str()}")),
			Engine.watch_latest(
				job,
				|{ active, .. }| if (active) [squares.ask(2_000_000)] else [],
				Engine.react(
					|_, result| [
						cells.running.set(Bool.False),
						cells.status.set(
							match result {
								Ok(n) => "Sum of squares modulo 1,000,000,007: ${n.to_str()}"
								Err(error) => "Worker failed: ${Str.inspect(error)}"
							},
						),
					],
				),
			),
		],
	)
}

expect sum_squares(4) == 14
