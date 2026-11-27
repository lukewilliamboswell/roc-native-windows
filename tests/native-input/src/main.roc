app [main, native] { pf: platform "../../../platform/main.roc" }

import pf.Engine
import pf.Ui
import pf.Win

main = Engine.program_store({}, |_| store, |_| view())

native = Win.Features.use([])

store = Engine.store(
	InputJig,
	{
		trace: Engine.cell(""),
		value: Engine.cell(35.I32),
		open: Engine.cell(Bool.False),
		row: Engine.cell(1.U64),
		uncaptured: Engine.cell(Bool.True),
	}.Engine.Cells,
)

cells = store.cells()

view = || {
	Ui.window(
		{ title: Engine.const(Title("Input fixture")) },
		Ui.vstack(
			{ padding: 16 },
			[
				Ui.text(cells.trace.signal()),
				Ui.slider(
					cells.value.signal(),
					{
						label: "Divider",
						step: 7,
						range: { min: -10, max: 100 },
						on_change: Engine.handle(|value| if (value == 99) [] else [cells.value.set(value)]),
					},
				),
				Ui.button("Remount", { on_press: Engine.handle(|_| [cells.row.update(|value| value + 1)]) }),
				Ui.button("Open modal", { on_press: Engine.handle(|_| [cells.open.set(Bool.True)]) }),
				Engine.each(cells.row.signal().map(|row| [row]), U64.to_str, |_| pointer_view()),
				Engine.when(cells.uncaptured.signal(), || uncaptured_view(), || Ui.vstack({}, [])),
				Ui.dialog(
					{
						title: "Modal",
						open: cells.open.signal(),
						on_close_request: Engine.handle(|_| [cells.open.set(Bool.False)]),
					},
					Ui.text("A modal owner guard"),
				),
			],
		),
	)
}

pointer_view = || {
	input : Ui.SurfaceInput(_)
	input = Interactive({
		pointer: Pointer({
			zone: "compare",
			capture: CaptureOnPress,
			move_policy: LatestMove,
			down: Handle(Engine.handle(|event| pointer("D", event))),
			move: Handle(Engine.handle(|event| pointer("M", event))),
			up: Handle(Engine.handle(|event| pointer("U", event))),
			cancel: Handle(Engine.handle(|event| pointer("C", event))),
		}),
		keys: FocusedKeys({
			key: { keymap: Engine.const(Engine.Keymap.prevent([Engine.Chord.plain("ArrowRight")])) },
			handle: Engine.handle(|event| [record(event.key)]),
		}),
	})
	bitmap("Pointer", input)
}

uncaptured_view = || bitmap(
	"Uncaptured",
	Interactive({
		pointer: Pointer({
			zone: "free",
			move: Handle(Engine.handle(uncaptured_move)),
		}),
	}),
)

bitmap = |label, input| {
	raster = Ui.raster({ width: 1, height: 1, rgba: [48, 98, 144, 255] })
	Ui.image(Engine.const(raster.map_ok(|value| RasterImage(value)) ?? NoImage), { description: Described({ label: Engine.const(label) }), width: 101, height: 103, input })
}

uncaptured_move : Engine.Pointer -> List(Engine.Write(_))
uncaptured_move = |event| match event.at {
	In({ x: 1000, .. }) =>
		[cells.trace.set("first buttons: ${event.buttons.to_str()}"), cells.uncaptured.set(Bool.False)]
	_ => [cells.trace.set("stale move delivered")]
}

pointer : Str, Engine.Pointer -> List(Engine.Write(_))
pointer = |kind, event|
	match event.at {
		In({ zone: "compare", x, .. }) =>
			[record("${kind}${x.to_str()}")].concat(if (kind == "M") [cells.value.set(x.to_i32() // 100)] else [])
		_ => [record("${kind}O")]
	}

record : Str -> Engine.Write(_)
record = |entry| cells.trace.update(|trace| if (trace.is_empty()) entry else "${trace} ${entry}")
