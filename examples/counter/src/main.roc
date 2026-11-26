app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, In, Root, State]
import pf.Ui

main : Engine.Program(_)
main = Engine.program(|| Engine.state(0.I64, view))

view : State(In(Root, I64), I64) -> Elem(In(Root, I64))
view = |count|
	Ui.vstack(
		{ padding: 16 },
		[
			Ui.text("Counter"),
			Ui.text(count.signal().map(|value| "Count: ${value.to_str()}")),
			Ui.hstack(
				{},
				[
					Ui.button("Decrease", { on_press: count.update(|value| value - 1) }),
					Ui.button("Reset", { on_press: count.update(|_| 0) }),
					Ui.button("Increase", { on_press: count.update(|value| value + 1) }),
				],
			),
		],
	)
