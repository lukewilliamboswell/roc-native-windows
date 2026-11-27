import pf.Win

## Compact stock-count views share the same native appearance.
StockStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 80, green: 56, blue: 135 }) },
	page : Win.Ui.StackProps ?? { padding: 14, spacing: 8 },
	header : Win.Ui.StackProps ?? { spacing: 10, stack_below: 230 },
	commands : Win.Ui.StackProps ?? { spacing: 8, stack_below: 300 },
	title : Win.Ui.TextProps ?? { style: Title, tone: AccentText },
	metadata : Win.Ui.TextProps ?? { tone: SecondaryText },
	grid_panel : Win.Ui.PanelProps ?? { surface: Inset, border: Outline, width: Fill, padding: 8 },
	results_panel : Win.Ui.PanelProps ?? { surface: AccentTint, border: Outline, width: Fill, padding: 10 },
	results : Win.Ui.StackProps ?? { spacing: 6 },
	summary : Win.Ui.TextProps ?? { style: Heading, tone: AccentText },
	metric : Win.Ui.TextProps ?? { style: Monospace, tone: SecondaryText },
}
