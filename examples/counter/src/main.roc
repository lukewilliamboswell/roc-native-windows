app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, In, Root, State]
import pf.Ui

main : Engine.Program(_)
main = Engine.program(|| Engine.state(0.I64, view))

CounterStyle := {
	appearance : Ui.AppearanceProps ?? { accent: CustomAccent({ red: 33, green: 100, blue: 146 }) },
	page : Ui.StackProps ?? { padding: 16 },
	row : Ui.StackProps ?? { spacing: 12 },
	title : Ui.TextProps ?? { style: Heading },
	readout_panel : Ui.PanelProps ?? { surface: AccentTint, width: Fill, padding: 24 },
	readout : Ui.TextProps ?? {
		style: Display,
		tone: AccentText,
		align: Center,
		font: PackagedFont({
			asset: "fonts/IBMPlexSans-Bold.ttf",
			family: "IBM Plex Sans",
			face: Bold,
		}),
	},
}

view : State(In(Root, I64), I64) -> Elem(In(Root, I64))
view = |count| {
	design = CounterStyle.{}
	Ui.window(
		{ appearance: design.appearance },
		Ui.vstack(
			design.page,
			[
				Ui.hstack(
					design.row,
					[
						Ui.asset_image("counter.bmp", { description: Decorative, width: 32, height: 32 }),
						Ui.text_with("Counter", design.title),
					],
				),
				Ui.panel(
					design.readout_panel,
					Ui.text_with(count.signal().map(|value| "Count: ${value.to_str()}"), design.readout),
				),
				Ui.hstack(
					design.row,
					[
						Ui.button("Decrease", { on_press: count.update(|value| value - 1) }),
						Ui.button("Reset", { on_press: count.update(|_| 0) }),
						Ui.button("Increase", { on_press: count.update(|value| value + 1) }),
					],
				),
			],
		),
	)
}
