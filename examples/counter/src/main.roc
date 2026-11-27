app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, In, Root, State]
import pf.Ui

main : Engine.Program(_)
main = Engine.program(|| Engine.state(0.I64, view))

view : State(In(Root, I64), I64) -> Elem(In(Root, I64))
view = |count|
	Ui.window(
		{ appearance: { accent: CustomAccent({ red: 33, green: 100, blue: 146 }) } },
		Ui.vstack(
			{ padding: 16 },
			[
				Ui.hstack(
					{ spacing: 12 },
					[
						Ui.asset_image("counter.bmp", { label: "Five tally marks", width: 32, height: 32 }),
						Ui.text_with("Counter", { style: Heading }),
					],
				),
				Ui.panel(
					{ surface: AccentTint, width: Fill, padding: 24 },
					Ui.text_with(count.signal().map(|value| "Count: ${value.to_str()}"), { style: Display, tone: AccentText, align: Center }),
				),
				Ui.hstack(
					{ spacing: 12 },
					[
						Ui.button("Decrease", { on_press: count.update(|value| value - 1) }),
						Ui.button("Reset", { on_press: count.update(|_| 0) }),
						Ui.button("Increase", { on_press: count.update(|value| value + 1) }),
					],
				),
			],
		),
	)
