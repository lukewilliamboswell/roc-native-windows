import Engine exposing [Elem, Handler, HandlerOf, Signal]
import Files

## Native Windows controls backed by the roc-signals engine.
Ui := [].{

	## A trailing native status pane with a width in device-independent pixels.
	StatusPart(ctx) := { text : Signal(ctx, Str), width : U32 ?? 128 }

	StatusBarProps(ctx) := {
		text : Signal(ctx, Str),
		trailing : List(StatusPart(ctx)) ?? [],
	}

	WindowProps(ctx) := {
		status_bar : [NoStatusBar, StatusBar(StatusBarProps(ctx))] ?? NoStatusBar,
		taskbar_progress : Signal(ctx, Progress) ?? Engine.const(Idle),
	}

	## Configure the app's one outermost native window.
	window : WindowProps(ctx), Elem(ctx) -> Elem(ctx)
	window = |props, content| {
		body = Engine.element("ui-window-content", [], [content])
		chrome = match props.status_bar {
			NoStatusBar => []
			StatusBar(bar) => [
				Engine.element(
					"ui-status-bar",
					[Engine.attr_s("text", bar.text)],
					bar.trailing.map(
						|item| Engine.element(
							"ui-status-part",
							[Engine.attr_s("text", item.text), Engine.attr("width", item.width.to_str())],
							[],
						),
					),
				),
			]
		}
		Engine.element("ui-window", [Engine.attr_s("taskbar-progress", props.taskbar_progress.map(progress_word))], [body].concat(chrome))
	}

	## Stack spacing and padding in device-independent pixels.
	StackProps := { spacing : U32 ?? 8, padding : U32 ?? 0 }

	## Lay out children vertically.
	vstack : StackProps, List(Elem(ctx)) -> Elem(ctx)
	vstack = |props, children| stack("ui-vstack", props, children)

	## Lay out children horizontally.
	hstack : StackProps, List(Elem(ctx)) -> Elem(ctx)
	hstack = |props, children| stack("ui-hstack", props, children)

	## A label whose text follows a signal.
	text : Signal(ctx, Str) -> Elem(ctx)
	text = |value| Ui.text_with(value, {})

	## Wrapped text or a single line with an ellipsis when it does not fit.
	TextProps := { overflow : [Wrap, EllipsizeEnd] ?? Wrap }

	## The full caption remains available when the display truncates it.
	text_with : Signal(ctx, Str), TextProps -> Elem(ctx)
	text_with = |value, props|
		Engine.element(
			"ui-text",
			[Engine.attr("ellipsize-end", bool_word(props.overflow == EllipsizeEnd))],
			[Engine.text_s(value)],
		)

	## An optional ICO asset beside a command's text.
	CommandIcon := [NoIcon, IconAsset(Str)]

	## A button's action and enabled state.
	ButtonProps(ctx) := {
		on_press : HandlerOf(ctx, Engine.Click),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		enter_action : [Standard, WindowDefault] ?? Standard,
		icon : Signal(ctx, CommandIcon) ?? Engine.const(NoIcon),
	}

	## A push button.
	button : Signal(ctx, Str), ButtonProps(ctx) -> Elem(ctx)
	button = |title, props|
		Engine.element(
			"ui-button",
			[
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr("window-default", bool_word(props.enter_action == WindowDefault)),
				Engine.attr_s("command-icon", props.icon.map(command_icon_path)),
				Engine.on_click(props.on_press),
			],
			[Engine.text_s(title)],
		)

	## The accessible name of a toolbar.
	ToolbarProps := { label : Str }

	## A toolbar command's stable key, click handler and controlled availability.
	ToolbarButtonProps(ctx) := {
		key : Str,
		on_press : HandlerOf(ctx, Engine.Click),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		icon : Signal(ctx, CommandIcon) ?? Engine.const(NoIcon),
	}

	ToolbarItem(ctx) := [ToolbarCommand(Elem(ctx)), ToolbarSeparator]

	## A text-bearing command inside a toolbar.
	toolbar_button : Signal(ctx, Str), ToolbarButtonProps(ctx) -> ToolbarItem(ctx)
	toolbar_button = |title, props| ToolbarCommand(
		Engine.element(
			"ui-toolbar-button",
			[
				Engine.attr("command-key", props.key),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr_s("command-icon", props.icon.map(command_icon_path)),
				Engine.on_click(props.on_press),
			],
			[Engine.text_s(title)],
		),
	)

	## A divider between command groups.
	toolbar_separator : () -> ToolbarItem(ctx)
	toolbar_separator = || ToolbarSeparator

	## A native command group that wraps as its width changes.
	toolbar : ToolbarProps, List(ToolbarItem(ctx)) -> Elem(ctx)
	toolbar = |props, items| {
		children = items.map(
			|item| match item {
				ToolbarCommand(command) => command
				ToolbarSeparator => Engine.element("ui-toolbar-separator", [], [])
			},
		)
		Engine.element("ui-toolbar", [Engine.attr("label", props.label)], children)
	}

	## A text field sends the current value after each edit.
	TextFieldProps(ctx) := {
		on_input : Handler(ctx),
		placeholder : Str ?? "",
		label : Str ?? "",
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## A single-line editable text field.
	text_field : Signal(ctx, Str), TextFieldProps(ctx) -> Elem(ctx)
	text_field = |value, props|
		Engine.element(
			"ui-text-field",
			[
				Engine.attr_s("value", value),
				Engine.attr("placeholder", props.placeholder),
				Engine.attr("label", props.label),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.on("input", props.on_input),
			],
			[],
		)

	## A read-only preview or an editor that sends its entire LF-delimited draft.
	TextAreaProps(ctx) := {
		label : Str,
		rows : U32 ?? 5,
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		mode : [ReadOnly, Editable(HandlerOf(ctx, Str))] ?? ReadOnly,
		reference : Str ?? "",
	}

	## A wrapped, selectable multiline edit control. Set a reference to request ApplyValue.
	text_area : Signal(ctx, Str), TextAreaProps(ctx) -> Elem(ctx)
	text_area = |value, props| {
		base = [
			Engine.prop_s(Engine.Prop.value, value),
			Engine.attr("label", props.label),
			Engine.attr("rows", props.rows.to_str()),
			Engine.attr_s("enabled", props.enabled.map(bool_word)),
			Engine.ref(props.reference),
		]
		attrs = match props.mode {
			ReadOnly => base
			Editable(on_input) => base.append(Engine.attr("editable", "true")).append(Engine.on("input", on_input))
		}
		Engine.element("textarea", attrs, [])
	}

	## A checkbox sends its new checked state as a Bool.
	CheckboxProps(ctx) := {
		checked : Signal(ctx, Bool),
		on_change : HandlerOf(ctx, Bool),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## A check box with a text label.
	checkbox : Signal(ctx, Str), CheckboxProps(ctx) -> Elem(ctx)
	checkbox = |title, props|
		Engine.element(
			"ui-checkbox",
			[
				Engine.attr_s("title", title),
				Engine.attr_s("checked", props.checked.map(bool_word)),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.on_checked(props.on_change),
			],
			[],
		)

	## The rendered state of an aggregate check box.
	CheckState : [Unchecked, Checked, Mixed]

	AggregateCheckboxProps(ctx) := {
		state : Signal(ctx, CheckState),
		on_change : HandlerOf(ctx, Bool),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## An activation requests true from Unchecked or Mixed and false from Checked.
	aggregate_checkbox : Signal(ctx, Str), AggregateCheckboxProps(ctx) -> Elem(ctx)
	aggregate_checkbox = |title, props|
		Engine.element(
			"ui-aggregate-checkbox",
			[
				Engine.attr_s("title", title),
				Engine.attr_s("check-state", props.state.map(check_word)),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.on_checked(props.on_change),
			],
			[],
		)

	## A Gregorian calendar date in the inclusive range 1753-01-01 through 9999-12-31.
	CivilDate := { year : U16, month : U8, day : U8 }.{

		## Construct a date without applying a locale, offset or timezone.
		from_parts : { year : U16, month : U8, day : U8 } -> Try(CivilDate, [YearOutOfRange, InvalidCalendarDate])
		from_parts = |fields| {
			if (fields.year < 1753 or fields.year > 9999) return Err(YearOutOfRange)
			leap = fields.year % 4 == 0 and (fields.year % 100 != 0 or fields.year % 400 == 0)
			limit = match fields.month {
				1 | 3 | 5 | 7 | 8 | 10 | 12 => 31
				4 | 6 | 9 | 11 => 30
				2 => if (leap) 29 else 28
				_ => 0
			}
			if fields.day == 0 or fields.day > limit {
				Err(InvalidCalendarDate)
			} else {
				Ok(CivilDate.{ year: fields.year, month: fields.month, day: fields.day })
			}
		}

		## The original Gregorian components.
		parts : CivilDate -> { year : U16, month : U8, day : U8 }
		parts = |date| { year: date.year, month: date.month, day: date.day }

		## ISO date text for interchange and app-defined summaries.
		to_iso : CivilDate -> Str
		to_iso = |date| "${date.year.to_str()}-${two_digits(date.month)}-${two_digits(date.day)}"

		is_eq : CivilDate, CivilDate -> Bool
		is_eq = |a, b| a.parts() == b.parts()
	}

	## An optional date from a calendar picker.
	DateSelection : [NoDate, Date(CivilDate)]

	DatePickerProps(ctx) := {
		label : Signal(ctx, Str),
		on_change : HandlerOf(ctx, DateSelection),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## Edit a controlled date with a native, localized calendar.
	date_picker : Signal(ctx, DateSelection), DatePickerProps(ctx) -> Elem(ctx)
	date_picker = |value, props| {
		encoded = value.map(
			|selection| match selection {
				NoDate => ""
				Date(date) => date.to_iso()
			},
		)
		handler = props.on_change.from_payload_by(value, |current, payload| decode_date(payload) ?? current)
		civil_picker("ui-date-picker", encoded, props.label, props.enabled, handler)
	}

	## A local clock reading with minute precision.
	CivilTime := { hour : U8, minute : U8 }.{
		from_parts : { hour : U8, minute : U8 } -> Try(CivilTime, [InvalidClockTime])
		from_parts = |fields|
			if fields.hour < 24 and fields.minute < 60 {
				Ok(CivilTime.{ hour: fields.hour, minute: fields.minute })
			} else {
				Err(InvalidClockTime)
			}

		parts : CivilTime -> { hour : U8, minute : U8 }
		parts = |time| { hour: time.hour, minute: time.minute }

		## 24-hour time text with minutes for interchange and app-defined summaries.
		to_iso : CivilTime -> Str
		to_iso = |time| "${two_digits(time.hour)}:${two_digits(time.minute)}"

		is_eq : CivilTime, CivilTime -> Bool
		is_eq = |a, b| a.parts() == b.parts()
	}

	## An optional time of day without a timezone or offset.
	TimeSelection : [NoTime, Time(CivilTime)]

	TimePickerProps(ctx) := {
		label : Signal(ctx, Str),
		on_change : HandlerOf(ctx, TimeSelection),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## Edit a controlled hour and minute with the user's short-time format.
	time_picker : Signal(ctx, TimeSelection), TimePickerProps(ctx) -> Elem(ctx)
	time_picker = |value, props| {
		encoded = value.map(
			|selection| match selection {
				NoTime => ""
				Time(time) => time.to_iso()
			},
		)
		handler = props.on_change.from_payload_by(value, |current, payload| decode_time(payload) ?? current)
		civil_picker("ui-time-picker", encoded, props.label, props.enabled, handler)
	}

	## Presentation progress for work with optional measurable counts.
	Progress := [Idle, Indeterminate, Determinate({ completed : U64, total : U64 })]

	ProgressBarProps := {
		label : Str,
		height : U32 ?? 16,
	}

	## Show a native progress indicator. Idle keeps its space and hides the indicator.
	progress_bar : Signal(ctx, Progress), ProgressBarProps -> Elem(ctx)
	progress_bar = |progress, props|
		Engine.element(
			"ui-progress-bar",
			[
				Engine.attr("label", props.label),
				Engine.attr("height", props.height.to_str()),
				Engine.attr_s("progress", progress.map(progress_word)),
			],
			[],
		)

	## A stable native option key and its display label.
	Choice := { key : Str, label : Str }

	## The selected option, if any.
	Selection := [NoSelection, Selected(Str)]

	ComboBoxProps(ctx) := {
		label : Str,
		choices : Signal(ctx, List(Choice)),
		selected : Signal(ctx, Selection),
		on_select : HandlerOf(ctx, Selection),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## Select one of a short list of options with a native dropdown.
	combo_box : ComboBoxProps(ctx) -> Elem(ctx)
	combo_box = |props|
		choice_control("ui-combo-box", props.label, props.choices, props.selected, props.on_select, props.enabled, 1)

	ListBoxProps(ctx) := {
		label : Str,
		choices : Signal(ctx, List(Choice)),
		selected : Signal(ctx, Selection),
		on_select : HandlerOf(ctx, Selection),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		visible_rows : U32 ?? 6,
	}

	## Select one option from a visible scrollable list.
	list_box : ListBoxProps(ctx) -> Elem(ctx)
	list_box = |props|
		choice_control("ui-list-box", props.label, props.choices, props.selected, props.on_select, props.enabled, props.visible_rows)

	RadioGroupProps(ctx) := {
		label : Str,
		choices : List(Choice),
		selected : Signal(ctx, Str),
		on_select : HandlerOf(ctx, Str),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## Select a required key from a static group of native radio buttons.
	radio_group : RadioGroupProps(ctx) -> Elem(ctx)
	radio_group = |props|
		Engine.element(
			"ui-radio-group",
			[
				Engine.attr("label", props.label),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr_s("selected", props.selected),
				Engine.on("change", props.on_select),
			],
			props.choices.map(static_choice),
		)

	## A keyed tree item. The list order defines the order of siblings.
	TreeNode := { key : Str, label : Str, parent : [Root, ChildOf(Str)] }

	Expansion : [Expanded, Collapsed]
	TreeExpansion : { key : Str, expansion : Expansion }

	TreeViewProps(ctx) := {
		label : Str,
		nodes : Signal(ctx, List(TreeNode)),
		selected : Signal(ctx, Selection),
		on_select : HandlerOf(ctx, Selection),
		expanded : Signal(ctx, Set(Str)),
		on_expand : HandlerOf(ctx, TreeExpansion),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		visible_rows : U32 ?? 8,
	}

	## A tree with controlled selection and expansion.
	tree_view : TreeViewProps(ctx) -> Elem(ctx)
	tree_view = |props| {
		nodes = { nodes: props.nodes, expanded: props.expanded }.Signal.map(|{ nodes: items, expanded }|
			items.map(|item| {
				key: item.key,
				label: item.label,
				parent: match item.parent {
					Root => ""
					ChildOf(key) => "1${key}"
				},
				expanded: expanded.contains(item.key),
			}),
		)
		Engine.element(
			"ui-tree-view",
			[
				Engine.attr("label", props.label),
				Engine.attr("visible-rows", props.visible_rows.to_str()),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr_s("selected", props.selected.map(selection_key)),
				Engine.on("change", props.on_select.from_payload_by(Engine.const({}), |_, key| selection_from_key(key))),
				Engine.on("expand", props.on_expand.from_payload_by(Engine.const({}), |_, payload| {
					key: payload.drop_prefix(if (payload.starts_with("1")) "1" else "0"),
					expansion: if (payload.starts_with("1")) Expanded else Collapsed,
				})),
			],
			[
				Engine.each(nodes, |item| item.key, |row| {
					node = row.item()
					Engine.element(
						"ui-tree-node",
						[
							Engine.attr_s("key", node.map(|item| item.key)),
							Engine.attr_s("label", node.map(|item| item.label)),
							Engine.attr_s("parent", node.map(|item| item.parent)),
							Engine.attr_s("expanded", node.map(|item| bool_word(item.expanded))),
						],
						[],
					)
				}),
			],
		)
	}

	## A native tab strip with an app-owned selection and active page.
	TabsProps(ctx) := {
		label : Str,
		tabs : List(Choice),
		selected : Signal(ctx, Str),
		on_select : HandlerOf(ctx, Str),
	}

	## Use the selected key to mount the active page, for example with Engine.switch.
	tabs : TabsProps(ctx), Elem(ctx) -> Elem(ctx)
	tabs = |props, page|
		Engine.element(
			"ui-tabs",
			[
				Engine.attr("label", props.label),
				Engine.attr_s("selected", props.selected),
				Engine.on("change", props.on_select),
			],
			props.tabs.map(static_choice).append(Engine.element("ui-tab-panel", [], [page])),
		)

	## A labeled area that receives Explorer file drops.
	FileDropTargetProps(ctx) := { label : Str, on_files : Engine.FilesHandler(ctx) }

	## Mount content inside a native file drop target.
	file_drop_target : FileDropTargetProps(ctx), Elem(ctx) -> Elem(ctx)
	file_drop_target = |props, content|
		Engine.element("ui-file-drop-target", [
			Engine.attr("label", props.label),
			Engine.on_files(Drop, props.on_files),
		], [content])

	## Receive handles for the files selected by the user.
	FileButtonProps(ctx) := {
		on_files : Engine.FilesHandler(ctx),
		selection : [SingleFile, MultipleFiles] ?? SingleFile,
	}

	## Open the system file picker.
	file_button : Signal(ctx, Str), FileButtonProps(ctx) -> Elem(ctx)
	file_button = |title, props|
		Engine.element(
			"ui-file-button",
			[Engine.attr("multiple", bool_word(props.selection == MultipleFiles)), Engine.on_files(Change, props.on_files)],
			[Engine.text_s(title)],
		)

	FolderButtonProps(ctx) : Files.FolderButtonProps(ctx)

	## Open the system folder picker. Cancelling preserves the current selection.
	folder_button : Signal(ctx, Str), FolderButtonProps(ctx) -> Elem(ctx)
	folder_button = Files.folder_button

	## A report column. Fixed widths are in device-independent pixels.
	Column := { key : Str, heading : Str, width : [Auto, Fixed(U32)] ?? Auto }

	## A single-select report with optional row activation and header actions.
	ListViewProps(ctx) := {
		label : Str,
		columns : List(Column),
		selected : Signal(ctx, Selection),
		on_select : HandlerOf(ctx, Selection),
		on_activate : [Ignore, Handle(HandlerOf(ctx, Str))] ?? Ignore,
		on_column_press : [Ignore, Handle(HandlerOf(ctx, Str))] ?? Ignore,
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		visible_rows : U32 ?? 8,
	}

	## Display rows in their engine order. Row and column keys are unique and nonempty.
	list_view : ListViewProps(ctx), List(Elem(ctx)) -> Elem(ctx)
	list_view = |props, rows| {
		activate = match props.on_activate {
			Ignore => []
			Handle(handler) => [Engine.on("activate", handler)]
		}
		press = match props.on_column_press {
			Ignore => []
			Handle(handler) => [Engine.on("column-press", handler)]
		}
		Engine.element(
			"ui-list-view",
			[
				Engine.attr("label", props.label),
				Engine.attr_s("selected", props.selected.map(selection_key)),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr("visible-rows", props.visible_rows.to_str()),
				Engine.on("change", props.on_select.from_payload_by(Engine.const({}), |_, key| selection_from_key(key))),
			].concat(activate).concat(press),
			props.columns.map(list_column).concat(rows),
		)
	}

	ListRowProps(ctx) := { key : Signal(ctx, Str) }

	## A report row whose key remains stable when its cells change.
	list_row : ListRowProps(ctx), List(Elem(ctx)) -> Elem(ctx)
	list_row = |props, cells| Engine.element("ui-list-row", [Engine.attr_s("list-key", props.key)], cells)

	ListCellProps := { column : Str }

	## A report cell. Its column names its position regardless of declaration order.
	list_cell : Signal(ctx, Str), ListCellProps -> Elem(ctx)
	list_cell = |value, props| Engine.element("ui-list-cell", [Engine.attr("list-key", props.column)], [Engine.text_s(value)])

	## A rectangular paste, in mounted view order with a one-based starting row.
	GridPaste := { first_row : U64, columns : List(Str), rows : List(List(Str)) }

	## Paste validation happens before an app can apply the batch.
	GridPasteError := [Unavailable, Malformed, TooLarge, OutsideGrid, SelectionMismatch, ReadOnlyCell]

	## A report grid whose cells can receive typed text edits.
	DataGridProps(ctx) := {
		label : Str,
		columns : List(Column),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		visible_rows : U32 ?? 8,
		on_paste : [Ignore, Handle(HandlerOf(ctx, Try(GridPaste, GridPasteError)))] ?? Ignore,
	}

	## Display rows from an Engine.table_view using its mounted identities.
	data_grid : DataGridProps(ctx), Elem(ctx) -> Elem(ctx)
	data_grid = |props, rows| {
		attrs = [
			Engine.attr("label", props.label),
			Engine.attr_s("enabled", props.enabled.map(bool_word)),
			Engine.attr("visible-rows", props.visible_rows.to_str()),
		]
		with_paste = match props.on_paste {
			Ignore => attrs
			Handle(handler) => attrs.append(Engine.on("grid-paste", handler.from_payload_by(Engine.const({}), |_, payload| grid_paste_from_payload(payload))))
		}
		Engine.element("ui-data-grid", with_paste, props.columns.map(list_column).append(rows))
	}

	## An edit intent carries the entire draft from the native text editor.
	CellEdit := { draft : Str }

	## Empty feedback clears a previous rejection.
	Feedback := [Clear, Invalid(Str)]

	GridCellProps(ctx) := {
		column : Str,
		editor : [ReadOnly, TextEditor(HandlerOf(ctx, CellEdit))] ?? ReadOnly,
		feedback : Signal(ctx, Feedback) ?? Engine.const(Clear),
	}

	## Place one cell per declared column in each rendered row.
	grid_row : List(Elem(ctx)) -> Elem(ctx)
	grid_row = |cells| Engine.element("ui-grid-row", [], cells)

	## Project a cell's committed text, edit handler and rejection feedback.
	grid_cell : Signal(ctx, Str), GridCellProps(ctx) -> Elem(ctx)
	grid_cell = |value, props| {
		attrs = [
			Engine.attr("list-key", props.column),
			Engine.attr_s("feedback", props.feedback.map(feedback_text)),
		]
		edit = match props.editor {
			ReadOnly => []
			TextEditor(handler) => [
				Engine.on(
					"grid-commit",
					handler.from_payload_by(
						Engine.const({}),
						|_, draft| { draft: draft },
					),
				),
			]
		}
		Engine.element("ui-grid-cell", attrs.concat(edit), [Engine.text_s(value)])
	}

	## An image's accessible label and dimensions in device-independent pixels.
	AssetImageProps := { label : Str, width : U32, height : U32 }

	## Show a BMP using a path relative to the app's assets directory.
	asset_image : Signal(ctx, Str), AssetImageProps -> Elem(ctx)
	asset_image = |path, props|
		Engine.element(
			"ui-asset-image",
			[
				Engine.attr_s("asset", path),
				Engine.attr("label", props.label),
				Engine.attr("width", props.width.to_str()),
				Engine.attr("height", props.height.to_str()),
			],
			[],
		)
}

stack : Engine.Tag, Ui.StackProps, List(Elem(ctx)) -> Elem(ctx)
stack = |kind, props, children|
	Engine.element(
		kind,
		[
			Engine.attr("spacing", props.spacing.to_str()),
			Engine.attr("padding", props.padding.to_str()),
		],
		children,
	)

bool_word : Bool -> Str
bool_word = |value| if (value) "true" else "false"

check_word : Ui.CheckState -> Str
check_word = |state| match state {
	Unchecked => "false"
	Checked => "true"
	Mixed => "mixed"
}

selection_key : Ui.Selection -> Str
selection_key = |selection| match selection {
	NoSelection => ""
	Selected(key) => key
}

selection_from_key : Str -> Ui.Selection
selection_from_key = |key| if (key.is_empty()) NoSelection else Selected(key)

static_choice : Ui.Choice -> Elem(ctx)
static_choice = |choice|
	Engine.element("ui-choice", [Engine.attr("key", choice.key), Engine.attr("label", choice.label)], [])

choice_control : Engine.Tag, Str, Signal(ctx, List(Ui.Choice)), Signal(ctx, Ui.Selection), HandlerOf(ctx, Ui.Selection), Signal(ctx, Bool), U32 -> Elem(ctx)
choice_control = |tag, label, choices, selected, on_select, enabled, rows|
	Engine.element(
		tag,
		[
			Engine.attr("label", label),
			Engine.attr("visible-rows", rows.to_str()),
			Engine.attr_s("enabled", enabled.map(bool_word)),
			Engine.attr_s("selected", selected.map(selection_key)),
			Engine.on("change", on_select.from_payload_by(Engine.const({}), |_, key| selection_from_key(key))),
		],
		[
			Engine.each(
				choices.map(|options| options.map(|item| { key: item.key, label: item.label })),
				|choice| choice.key,
				|row| {
					choice = row.item()
					Engine.element("ui-choice", [Engine.attr_s("key", choice.map(|item| item.key)), Engine.attr_s("label", choice.map(|item| item.label))], [])
				},
			),
		],
	)

list_column : Ui.Column -> Elem(ctx)
list_column = |column| {
	width = match column.width {
		Auto => "auto"
		Fixed(dip) => dip.to_str()
	}
	Engine.element("ui-list-column", [Engine.attr("list-key", column.key), Engine.attr("list-width", width)], [Engine.text_s(Engine.const(column.heading))])
}

two_digits : U8 -> Str
two_digits = |n| if (n < 10) "0${n.to_str()}" else n.to_str()

civil_picker : Engine.Tag, Signal(ctx, Str), Signal(ctx, Str), Signal(ctx, Bool), Handler(ctx) -> Elem(ctx)
civil_picker = |tag, value, label, enabled, handler|
	Engine.element(
		tag,
		[
			Engine.attr_s("value", value),
			Engine.attr_s("label", label),
			Engine.attr_s("enabled", enabled.map(bool_word)),
			Engine.on("change", handler),
		],
		[],
	)

decode_date : Str -> Try(Ui.DateSelection, [InvalidDatePayload])
decode_date = |payload|
	match payload.split_on("-") {
		[""] => Ok(NoDate)
		[y, m, d] => {
			year = U16.from_str(y) ? |_| InvalidDatePayload
			month = U8.from_str(m) ? |_| InvalidDatePayload
			day = U8.from_str(d) ? |_| InvalidDatePayload
			date = Ui.CivilDate.from_parts({ year, month, day }) ? |_| InvalidDatePayload
			Ok(Date(date))
		}
		_ => Err(InvalidDatePayload)
	}

decode_time : Str -> Try(Ui.TimeSelection, [InvalidTimePayload])
decode_time = |payload|
	match payload.split_on(":") {
		[""] => Ok(NoTime)
		[h, m] => {
			hour = U8.from_str(h) ? |_| InvalidTimePayload
			minute = U8.from_str(m) ? |_| InvalidTimePayload
			time = Ui.CivilTime.from_parts({ hour, minute }) ? |_| InvalidTimePayload
			Ok(Time(time))
		}
		_ => Err(InvalidTimePayload)
	}

expect decode_date("2000-02-29").map_ok(
	|selection| match selection {
		Date(date) => date.to_iso()
		NoDate => ""
	},
) == Ok("2000-02-29")
expect decode_date("1900-02-29") == Err(InvalidDatePayload)
expect decode_date("1753-01-01").is_ok()
expect decode_date("9999-12-31").is_ok()
expect Ui.CivilDate.from_parts({ year: 1752, month: 12, day: 31 }) == Err(YearOutOfRange)
expect Ui.CivilDate.from_parts({ year: 10000, month: 1, day: 1 }) == Err(YearOutOfRange)
expect Ui.CivilDate.from_parts({ year: 2026, month: 4, day: 31 }) == Err(InvalidCalendarDate)
expect decode_date("") == Ok(NoDate)
expect decode_time("00:00").is_ok()
expect decode_time("23:59").is_ok()
expect decode_time("24:00") == Err(InvalidTimePayload)
expect decode_time("12:60") == Err(InvalidTimePayload)
expect decode_time("") == Ok(NoTime)

feedback_text : Ui.Feedback -> Str
feedback_text = |feedback| match feedback {
	Clear => ""
	Invalid(message) => message
}

grid_paste_from_payload : Str -> Try(Ui.GridPaste, Ui.GridPasteError)
grid_paste_from_payload = |payload| match payload.split_on("\u(0000)") {
	["unavailable"] => Err(Unavailable)
	["oversized"] => Err(TooLarge)
	[first_row, selected_rows, selected_columns, available_rows, available_columns, .. as fields] => {
		row = U64.from_str(first_row) ? |_| Malformed
		selected_height = U64.from_str(selected_rows) ? |_| Malformed
		selected_width = U64.from_str(selected_columns) ? |_| Malformed
		height = U64.from_str(available_rows) ? |_| Malformed
		width = U64.from_str(available_columns) ? |_| Malformed
		if (row == 0 or height > 64 or width > 32 or fields.len() != width + 2) return Err(Malformed)
		columns = fields.take_first(width)
		mask_text = fields.get(width) ? |_| Malformed
		raw = fields.get(width + 1) ? |_| Malformed
		if (raw.to_utf8().len() > 98_304) return Err(TooLarge)
		normalize = raw.replace_each("\r\n", "\n").replace_each("\r", "\n")
		values = normalize.drop_suffix("\n").split_on("\n").map(|line| line.split_on("\t"))
		actual_height = values.len()
		actual_width = values.get(0).map_ok(List.len) ?? 0
		if (actual_height == 0 or actual_width == 0 or values.any(|line| line.len() != actual_width)) return Err(Malformed)
		if (actual_height > 64 or actual_width > 32) return Err(TooLarge)
		if ((selected_height > 1 or selected_width > 1) and (actual_height != selected_height or actual_width != selected_width)) return Err(SelectionMismatch)
		if (actual_height > height or actual_width > width) return Err(OutsideGrid)
		mask = mask_text.to_utf8()
		if (mask.len() != width * height) return Err(Malformed)
		for r in (0.U64).until(actual_height) {
			for c in (0.U64).until(actual_width) {
				editable = mask.get(r * width + c) ? |_| Malformed
				if (editable != 49) return Err(ReadOnlyCell)
			}
		}
		Ok({ first_row: row, columns: columns.take_first(actual_width), rows: values })
	}
	_ => Err(Malformed)
}

progress_word : Ui.Progress -> Str
progress_word = |state|
	match state {
		Idle => "idle"
		Indeterminate => "indeterminate"
		Determinate({ total: 0, .. }) => "idle"
		Determinate({ completed, total }) => "${completed.min(total).to_str()}/${total.to_str()}"
	}

command_icon_path : Ui.CommandIcon -> Str
command_icon_path = |icon| match icon {
	NoIcon => ""
	IconAsset(path) => path
}
