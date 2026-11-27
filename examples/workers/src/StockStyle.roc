import pf.Win

## Compact stock-count views share the same native appearance.
StockStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 80, green: 56, blue: 135 }) },
	page : Win.Ui.StackProps ?? { padding: 14, spacing: 9 },
	commands : Win.Ui.StackProps ?? { spacing: 8, stack_below: 300 },
	title : Win.Ui.TextProps ?? { style: Heading },
	metadata : Win.Ui.TextProps ?? { style: Caption, tone: SecondaryText },
	results_panel : Win.Ui.PanelProps ?? { surface: AccentTint, border: Outline, width: Fill },
	results : Win.Ui.StackProps ?? { spacing: 5 },
	summary : Win.Ui.TextProps ?? { style: Heading },
	metric : Win.Ui.TextProps ?? { style: Monospace },
}
