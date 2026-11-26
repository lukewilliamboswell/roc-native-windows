app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine
import pf.Ui

main : Engine.Program(_)
main =
	Engine.program(
		|| Ui.vstack(
			{ padding: 16, spacing: 12 },
			[
				Ui.text("Application assets"),
				Ui.asset_image("landscape.bmp", { label: "Sun over a mountain landscape", width: 192, height: 144 }),
				Ui.text("Loaded from this app's assets directory."),
			],
		),
	)
