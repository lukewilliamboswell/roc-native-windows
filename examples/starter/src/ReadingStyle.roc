import pf.Win

## Visual defaults shared by the reading-list views.
ReadingStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 123, green: 75, blue: 42 }) },
	page : Win.Ui.StackProps ?? { spacing: 12, padding: 16 },
	heading : Win.Ui.StackProps ?? { spacing: 12 },
	entry : Win.Ui.StackProps ?? { spacing: 8, stack_below: 300 },
	page_title : Win.Ui.TextProps ?? { style: Title, tone: AccentText },
	entry_panel : Win.Ui.PanelProps ?? { surface: AccentTint, width: Fill, padding: 10 },
}
