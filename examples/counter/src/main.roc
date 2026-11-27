app [main, native] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, In, Root, State]
import pf.Win

main : Engine.Program(_)
main = Engine.program(|| Engine.state(0.I64, view))

native = Win.Features.use([Win.Features.image_codecs!])

CounterStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 33, green: 100, blue: 146 }) },
	page : Win.Ui.StackProps ?? { padding: 16 },
	row : Win.Ui.StackProps ?? { spacing: 12 },
	title : Win.Ui.TextProps ?? { style: Heading },
	readout_panel : Win.Ui.PanelProps ?? { surface: AccentTint, width: Fill, padding: 24 },
	readout : Win.Ui.TextProps ?? {
		style: Display,
		tone: AccentText,
		align: Center,
		font: PackagedFont({
			asset: Win.asset("display-font"),
			family: "IBM Plex Sans",
			face: Bold,
		}),
	},
}

view : State(In(Root, I64), I64) -> Elem(In(Root, I64))
view = |count| {
	design = CounterStyle.{}
	Win.Ui.window(
		{ appearance: design.appearance },
		Win.Ui.vstack(
			design.page,
			[
				Win.Ui.hstack(
					design.row,
					[
						Win.Ui.asset_image(Engine.const(Win.asset("app-art")), { description: Decorative, width: 32, height: 32 }),
						Win.Ui.text_with("Counter", design.title),
					],
				),
				Win.Ui.panel(
					design.readout_panel,
					Win.Ui.text_with(count.signal().map(|value| "Count: ${value.to_str()}"), design.readout),
				),
				Win.Ui.hstack(
					design.row,
					[
						Win.Ui.button("Decrease", { on_press: count.update(|value| value - 1) }),
						Win.Ui.button("Reset", { on_press: count.update(|_| 0) }),
						Win.Ui.button("Increase", { on_press: count.update(|value| value + 1) }),
					],
				),
			],
		),
	)
}
