import pf.Ui

## Compact stock-count views share the same native appearance.
StockStyle := {
	appearance : Ui.AppearanceProps ?? { accent: CustomAccent({ red: 80, green: 56, blue: 135 }) },
	page : Ui.StackProps ?? { padding: 14, spacing: 9 },
	commands : Ui.StackProps ?? { spacing: 8, stack_below: 300 },
	title : Ui.TextProps ?? { style: Heading },
	metadata : Ui.TextProps ?? { style: Caption, tone: SecondaryText },
	results_panel : Ui.PanelProps ?? { surface: AccentTint, border: Outline, width: Fill },
	results : Ui.StackProps ?? { spacing: 5 },
	summary : Ui.TextProps ?? { style: Heading },
	metric : Ui.TextProps ?? { style: Monospace },
}
