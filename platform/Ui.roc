import Engine exposing [Elem, Handler, HandlerOf, Signal]

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
	text = |value| Engine.element("ui-text", [], [Engine.text_s(value)])

	## A button's action and enabled state.
	ButtonProps(ctx) := {
		on_press : HandlerOf(ctx, Engine.Click),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## A push button.
	button : Signal(ctx, Str), ButtonProps(ctx) -> Elem(ctx)
	button = |title, props|
		Engine.element(
			"ui-button",
			[Engine.attr_s("enabled", props.enabled.map(bool_word)), Engine.on_click(props.on_press)],
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
