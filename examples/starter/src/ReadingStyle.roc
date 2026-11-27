import pf.Win

## Visual defaults shared by the reading-list views.
ReadingStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 27, green: 101, blue: 91 }) },
	page : Win.Ui.StackProps ?? { spacing: 12, padding: 16 },
	heading : Win.Ui.StackProps ?? { spacing: 12 },
	entry : Win.Ui.StackProps ?? { spacing: 10, stack_below: 300 },
	page_title : Win.Ui.TextProps ?? { style: Title, tone: AccentText },
	entry_panel : Win.Ui.PanelProps ?? { surface: AccentTint, border: Outline, width: Fill, padding: 12 },
	browse_panel : Win.Ui.PanelProps ?? { surface: Inset, border: Outline, width: Fill, padding: 10 },
	browse : Win.Ui.StackProps ?? { spacing: 6 },
}
