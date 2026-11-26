import Engine exposing [Elem, Handler, HandlerOf, Signal]
import Files

## Native Windows controls backed by the roc-signals engine.
Ui := [].{

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

	## A button's action and enabled state.
	ButtonProps(ctx) := {
		on_press : HandlerOf(ctx, Engine.Click),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		enter_action : [Standard, WindowDefault] ?? Standard,
	}

	## A push button.
	button : Signal(ctx, Str), ButtonProps(ctx) -> Elem(ctx)
	button = |title, props|
		Engine.element(
			"ui-button",
			[
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr("window-default", bool_word(props.enter_action == WindowDefault)),
				Engine.on_click(props.on_press),
			],
			[Engine.text_s(title)],
		)

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
			props.choices.map(|choice| Engine.element("ui-choice", [Engine.attr("key", choice.key), Engine.attr("label", choice.label)], [])),
		)

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
