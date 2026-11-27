import pf.Ui

## Visual defaults shared by the reading-list views.
ReadingStyle := {
	appearance : Ui.AppearanceProps ?? { accent: CustomAccent({ red: 123, green: 75, blue: 42 }) },
	page : Ui.StackProps ?? { spacing: 12, padding: 16 },
	heading : Ui.StackProps ?? { spacing: 12 },
	entry : Ui.StackProps ?? { spacing: 8, stack_below: 300 },
	page_title : Ui.TextProps ?? { style: Title, tone: AccentText },
	entry_panel : Ui.PanelProps ?? { surface: AccentTint, width: Fill, padding: 10 },
}
