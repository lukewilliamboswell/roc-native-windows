import engine.Port
import engine.Wire as ImageWire
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
		appearance : AppearanceProps ?? {},
		status_bar : [NoStatusBar, StatusBar(StatusBarProps(ctx))] ?? NoStatusBar,
		taskbar_progress : Signal(ctx, Progress) ?? Engine.const(Idle),
		title : Signal(ctx, WindowTitle) ?? Engine.const(AppTitle),
		close : [UseDefaultClose, Handle(HandlerOf(ctx, {}))] ?? UseDefaultClose,
		startup : WindowStartup ?? {},
	}

	## Apply placement once when a top-level window opens. At uses virtual-screen pixels.
	WindowStartup := {
		position : [SystemPosition, Centered, At({ x : I32, y : I32 })] ?? SystemPosition,
		state : [Normal, Minimized, Maximized, Fullscreen] ?? Normal,
	}

	Rgb := { red : U8, green : U8, blue : U8 }

	## The accent applies to panels and text that opt into accent styling.
	AppearanceProps := { accent : [SystemAccent, CustomAccent(Rgb)] ?? SystemAccent }

	WindowTitle := [AppTitle, Title(Str)]

	## Configure the app's outermost window or declare a member of `Ui.windows`.
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
		attrs = [
			Engine.attr("accent", accent_color(props.appearance)),
			Engine.attr_s("taskbar-progress", props.taskbar_progress.map(progress_word)),
			Engine.attr_s("window-title", props.title.map(window_title)),
			Engine.attr(
				"startup-position",
				match props.startup.position {
					SystemPosition => "system"
					Centered => "centered"
					At({ x, y }) => "${x.to_str()},${y.to_str()}"
				},
			),
			Engine.attr(
				"startup-state",
				match props.startup.state {
					Normal => "normal"
					Minimized => "minimized"
					Maximized => "maximized"
					Fullscreen => "fullscreen"
				},
			),
		]
		listeners = match props.close {
			UseDefaultClose => attrs
			Handle(handler) => attrs.append(close_listener(handler))
		}
		Engine.element("ui-window", listeners, [body].concat(chrome))
	}

	accent_color : AppearanceProps -> Str
	accent_color = |appearance| match appearance.accent {
		SystemAccent => ""
		CustomAccent(rgb) => "${rgb.red.to_str()},${rgb.green.to_str()},${rgb.blue.to_str()}"
	}

	## Mount a keyed collection of independent Windows windows.
	windows : Elem(ctx) -> Elem(ctx)
	windows = |declarations| Engine.element("ui-windows", [], [declarations])

	DialogProps(ctx) := {
		title : Signal(ctx, Str),
		open : Signal(ctx, Bool),
		on_close_request : HandlerOf(ctx, {}),
	}

	## Present an owned modal dialog while `open` is true.
	dialog : DialogProps(ctx), Elem(ctx) -> Elem(ctx)
	dialog = |props, content| Engine.when(
		props.open,
		|| Engine.element(
			"ui-dialog",
			[Engine.attr_s("dialog-title", props.title), close_listener(props.on_close_request)],
			[Engine.element("ui-dialog-content", [], [content])],
		),
		|| Engine.text_s(Engine.const("")),
	)

	## Stack spacing and padding in device-independent pixels.
	StackProps := { spacing : U32 ?? 8, padding : U32 ?? 0, stack_below : U32 ?? 0 }

	## Lay out children vertically.
	vstack : StackProps, List(Elem(ctx)) -> Elem(ctx)
	vstack = |props, children| stack("ui-vstack", props, children)

	## Lay out children horizontally, or vertically below a configured width in DIP.
	hstack : StackProps, List(Elem(ctx)) -> Elem(ctx)
	hstack = |props, children| stack("ui-hstack", props, children)

	## A padded surface for grouping related content.
	PanelProps := {
		surface : [Inset, AccentTint] ?? Inset,
		border : [NoBorder, Outline] ?? NoBorder,
		padding : U32 ?? 12,
		width : [Natural, Fill] ?? Natural,
	}

	panel : PanelProps, Elem(ctx) -> Elem(ctx)
	panel = |props, content| Engine.element(
		"ui-panel",
		[
			Engine.attr("accent-tint", bool_word(props.surface == AccentTint)),
			Engine.attr("outline", bool_word(props.border == Outline)),
			Engine.attr("fill", bool_word(props.width == Fill)),
			Engine.attr("padding", props.padding.to_str()),
		],
		[content],
	)

	## A label whose text follows a signal.
	text : Signal(ctx, Str) -> Elem(ctx)
	text = |value| Ui.text_with(value, {})

	## A static TrueType face in the app's assets directory.
	PackagedFontProps := {
		asset : Str,
		family : Str,
		face : [Regular, Bold, Italic, BoldItalic] ?? Regular,
	}

	## Typography for a label. The role supplies size and the Windows fallback.
	TextFont := [SystemFont, PackagedFont(PackagedFontProps)]

	## Wrapped text or a single line with an ellipsis when it does not fit.
	TextProps := {
		overflow : [Wrap, EllipsizeEnd] ?? Wrap,
		style : [Body, Caption, Heading, Title, Display, Monospace] ?? Body,
		tone : [PrimaryText, SecondaryText, AccentText] ?? PrimaryText,
		align : [Start, Center, End] ?? Start,
		font : TextFont ?? SystemFont,
	}

	## The full caption remains available when the display truncates it.
	text_with : Signal(ctx, Str), TextProps -> Elem(ctx)
	text_with = |value, props|
		Engine.element(
			"ui-text",
			text_attributes(props),
			[Engine.text_s(value)],
		)

	StatusTextProps := { announcement : [Polite, Quiet] ?? Polite, text : TextProps ?? {} }

	## Persistent feedback that can announce a changed message without moving focus.
	status_text : Signal(ctx, Str), StatusTextProps -> Elem(ctx)
	status_text = |message, props| Engine.element(
		"ui-status-text",
		text_attributes(props.text).append(Engine.attr("announcement", if (props.announcement == Polite) "polite" else "quiet")),
		[Engine.text_s(message)],
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

	CopyResult : [Copied, Unavailable, InvalidText, TooLarge]

	CopyButtonProps(ctx) := {
		text : Signal(ctx, Str),
		on_result : HandlerOf(ctx, CopyResult),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## A native command that copies the currently committed plain text once.
	## The result reports busy, invalid text, or output over 64 KiB in UTF-16.
	copy_button : Signal(ctx, Str), CopyButtonProps(ctx) -> Elem(ctx)
	copy_button = |title, props| Engine.element(
		"ui-button",
		[
			Engine.attr_s("enabled", props.enabled.map(bool_word)),
			Engine.attr_s("host-copy", props.text),
			Engine.on(
				"host-copy-result",
				props.on_result.from_payload_by(
					Engine.const({}),
					|_, report| match report {
						"copied" => Copied
						"invalid" => InvalidText
						"too-large" => TooLarge
						_ => Unavailable
					},
				),
			),
		],
		[Engine.text_s(title)],
	)

	## The accessible name of a toolbar.
	ToolbarProps(ctx) := { label : Signal(ctx, Str) }

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
	toolbar : ToolbarProps(ctx), List(ToolbarItem(ctx)) -> Elem(ctx)
	toolbar = |props, items| {
		children = items.map(
			|item| match item {
				ToolbarCommand(command) => command
				ToolbarSeparator => Engine.element("ui-toolbar-separator", [], [])
			},
		)
		Engine.element("ui-toolbar", [Engine.attr_s("label", props.label)], children)
	}

	## A text field sends the current value after each edit.
	TextFieldProps(ctx) := {
		on_input : Handler(ctx),
		placeholder : Str ?? "",
		label : Signal(ctx, Str),
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
				Engine.attr_s("label", props.label),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.on("input", props.on_input),
			],
			[],
		)

	## A read-only preview or an editor that sends its entire LF-delimited draft.
	TextAreaProps(ctx) := {
		label : Signal(ctx, Str),
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
			Engine.attr_s("label", props.label),
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

	## An optional Gregorian civil date.
	DateSelection : [NoDate, Date(CivilDate)]

	DatePickerProps(ctx) := {
		label : Signal(ctx, Str),
		on_change : HandlerOf(ctx, DateSelection),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
	}

	## Enter an optional Gregorian date from 1753 through 9999. Use the Windows
	## short-date format shown in the field, or ISO YYYY-MM-DD. Enter or Tab
	## commits a valid date. An empty field clears it. Escape discards the draft.
	## Choose date opens a keyboard-navigable calendar for the same value.
	date_field : Signal(ctx, DateSelection), DatePickerProps(ctx) -> Elem(ctx)
	date_field = |value, props| {
		encoded = value.map(
			|selection| match selection {
				NoDate => ""
				Date(date) => date.to_iso()
			},
		)
		handler = props.on_change.from_payload_by(value, |current, payload| decode_date(payload) ?? current)
		civil_picker("ui-date-picker", encoded, props.label, props.enabled, handler)
	}

	## Enter an optional Gregorian civil date. See `date_field`.
	date_picker : Signal(ctx, DateSelection), DatePickerProps(ctx) -> Elem(ctx)
	date_picker = date_field

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

	## Enter an optional local clock time in the Windows short-time format
	## shown in the field, or 24-hour HH:MM. Enter or Tab commits a valid time.
	## An empty field clears it. Escape discards the draft.
	time_field : Signal(ctx, TimeSelection), TimePickerProps(ctx) -> Elem(ctx)
	time_field = |value, props| {
		encoded = value.map(
			|selection| match selection {
				NoTime => ""
				Time(time) => time.to_iso()
			},
		)
		handler = props.on_change.from_payload_by(value, |current, payload| decode_time(payload) ?? current)
		civil_picker("ui-time-picker", encoded, props.label, props.enabled, handler)
	}

	## Enter an optional local clock time. See `time_field`.
	time_picker : Signal(ctx, TimeSelection), TimePickerProps(ctx) -> Elem(ctx)
	time_picker = time_field

	## Presentation progress for work with optional measurable counts.
	Progress := [Idle, Indeterminate, Determinate({ completed : U64, total : U64 })]

	ProgressBarProps(ctx) := {
		label : Signal(ctx, Str),
		height : U32 ?? 16,
	}

	## Show a native progress indicator. Idle keeps its space and hides the indicator.
	progress_bar : Signal(ctx, Progress), ProgressBarProps(ctx) -> Elem(ctx)
	progress_bar = |progress, props|
		Engine.element(
			"ui-progress-bar",
			[
				Engine.attr_s("label", props.label),
				Engine.attr("height", props.height.to_str()),
				Engine.attr_s("progress", progress.map(progress_word)),
			],
			[],
		)

	## Read-only surfaces stay out of keyboard traversal by default.
	SurfaceInput(ctx) := [ReadOnly, Interactive(SurfaceInputProps(ctx))]

	SurfaceInputProps(ctx) := {
		pointer : [NoPointer, Pointer(PointerProps(ctx))] ?? NoPointer,
		keys : [NoKeys, FocusedKeys(FocusedKeyProps(ctx))] ?? NoKeys,
	}

	PointerProps(ctx) := {
		zone : Str,
		down : [NoHandler, Handle(HandlerOf(ctx, Engine.Pointer))] ?? NoHandler,
		move : [NoHandler, Handle(HandlerOf(ctx, Engine.Pointer))] ?? NoHandler,
		up : [NoHandler, Handle(HandlerOf(ctx, Engine.Pointer))] ?? NoHandler,
		cancel : [NoHandler, Handle(HandlerOf(ctx, Engine.Pointer))] ?? NoHandler,
		capture : [NoCapture, CaptureOnPress] ?? NoCapture,
		move_policy : [EveryMove, LatestMove] ?? EveryMove,
	}

	FocusedKeyProps(ctx) := { key : Engine.KeyProps(ctx), handle : HandlerOf(ctx, Engine.Key) }

	## Apply input declarations to a custom surface's element.
	surface_input : SurfaceInput(ctx) -> List(Engine.Attr(ctx))
	surface_input = |input| match input {
		ReadOnly => []
		Interactive(props) => {
			pointer = match props.pointer {
				NoPointer => []
				Pointer(binding) => {
					down = match (binding.down, binding.capture) {
						(Handle(handler), CaptureOnPress) => [Engine.on_pointer_capture(handler)]
						(Handle(handler), NoCapture) => [Engine.on_pointer(Down, handler)]
						(NoHandler, _) => []
					}
					move = match (binding.move, binding.move_policy) {
						(Handle(handler), LatestMove) => [Engine.on_pointer_paced(handler)]
						(Handle(handler), EveryMove) => [Engine.on_pointer(Move, handler)]
						(NoHandler, _) => []
					}
					[Engine.prop(Engine.Prop.pointer_zone, binding.zone)]
						.concat(down).concat(move)
						.concat(pointer_handler(Up, binding.up))
						.concat(pointer_handler(Cancel, binding.cancel))
				}
			}
			match props.keys {
				NoKeys => pointer
				FocusedKeys(binding) => pointer.append(Engine.on_key(binding.key, binding.handle))
			}
		}
	}

	SliderProps(ctx) := {
		label : Signal(ctx, Str),
		on_change : HandlerOf(ctx, I32),
		range : { min : I32, max : I32 } ?? { min: 0, max: 100 },
		step : I32 ?? 1,
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		reference : Str ?? "",
	}

	## A horizontal native trackbar controlled by an integer signal.
	slider : Signal(ctx, I32), SliderProps(ctx) -> Elem(ctx)
	slider = |value, props|
		Engine.element(
			"ui-slider",
			[
				Engine.ref(props.reference),
				Engine.attr_s("label", props.label),
				Engine.attr("range-min", props.range.min.to_str()),
				Engine.attr("range-max", props.range.max.to_str()),
				Engine.attr("step", props.step.to_str()),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr_s("value", value.map(I32.to_str)),
				Engine.on("change", props.on_change.from_payload_by(value, |current, payload| I32.from_str(payload) ?? current)),
			],
			[],
		)

	## A stable native option key and its display label.
	Choice := { key : Str, label : Str }

	## The selected option, if any.
	Selection := [NoSelection, Selected(Str)]

	ComboBoxProps(ctx) := {
		label : Signal(ctx, Str),
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
		label : Signal(ctx, Str),
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
		label : Signal(ctx, Str),
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
				Engine.attr_s("label", props.label),
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
		label : Signal(ctx, Str),
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
		nodes = { nodes: props.nodes, expanded: props.expanded }.Signal.map(
			|{ nodes: items, expanded }|
				items.map(
					|item| {
						key: item.key,
						label: item.label,
						parent: match item.parent {
							Root => ""
							ChildOf(key) => "1${key}"
						},
						expanded: expanded.contains(item.key),
					},
				),
		)
		Engine.element(
			"ui-tree-view",
			[
				Engine.attr_s("label", props.label),
				Engine.attr("visible-rows", props.visible_rows.to_str()),
				Engine.attr_s("enabled", props.enabled.map(bool_word)),
				Engine.attr_s("selected", props.selected.map(selection_key)),
				Engine.on("change", props.on_select.from_payload_by(Engine.const({}), |_, key| selection_from_key(key))),
				Engine.on(
					"expand",
					props.on_expand.from_payload_by(
						Engine.const({}),
						|_, payload| {
							key: payload.drop_prefix(if (payload.starts_with("1")) "1" else "0"),
							expansion: if (payload.starts_with("1")) Expanded else Collapsed,
						},
					),
				),
			],
			[
				Engine.each(
					nodes,
					|item| item.key,
					|row| {
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
					},
				),
			],
		)
	}

	## A native tab strip with an app-owned selection and active page.
	TabsProps(ctx) := {
		label : Signal(ctx, Str),
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
				Engine.attr_s("label", props.label),
				Engine.attr_s("selected", props.selected),
				Engine.on("change", props.on_select),
			],
			props.tabs.map(static_choice).append(Engine.element("ui-tab-panel", [], [page])),
		)

	## A labeled area that receives Explorer file drops.
	FileDropTargetProps(ctx) := { label : Signal(ctx, Str), on_files : Engine.FilesHandler(ctx) }

	## Mount content inside a native file drop target.
	file_drop_target : FileDropTargetProps(ctx), Elem(ctx) -> Elem(ctx)
	file_drop_target = |props, content|
		Engine.element(
			"ui-file-drop-target",
			[
				Engine.attr_s("label", props.label),
				Engine.on_files(Drop, props.on_files),
			],
			[content],
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
		label : Signal(ctx, Str),
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
				Engine.attr_s("label", props.label),
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
		label : Signal(ctx, Str),
		columns : List(Column),
		enabled : Signal(ctx, Bool) ?? Engine.const(Bool.True),
		visible_rows : U32 ?? 8,
		on_paste : [Ignore, Handle(HandlerOf(ctx, Try(GridPaste, GridPasteError)))] ?? Ignore,
	}

	## Display rows from an Engine.table_view using its mounted identities.
	data_grid : DataGridProps(ctx), Elem(ctx) -> Elem(ctx)
	data_grid = |props, rows| {
		attrs = [
			Engine.attr_s("label", props.label),
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

	## Texture input for a shader's t0 or t1 slot.
	ShaderTexture := [NoTexture, AssetTexture(Str), RasterTexture(Raster)]

	ShaderParameter := { name : Str, value : F32 }

	## Describe an informative graphic or omit a decorative graphic from accessibility views.
	GraphicDescription(ctx) := [Described(GraphicDescriptionProps(ctx)), Decorative]

	GraphicDescriptionProps(ctx) := {
		label : Signal(ctx, Str),
		details : Signal(ctx, [NoDetails, Details(Str)]) ?? Engine.const(NoDetails),
	}

	ShaderViewProps(ctx) := {
		description : GraphicDescription(ctx),
		shader : Str,
		width : U32,
		height : U32,
		texture : Signal(ctx, ShaderTexture) ?? Engine.const(NoTexture),
		second_texture : Signal(ctx, ShaderTexture) ?? Engine.const(NoTexture),
		parameters : Signal(ctx, List(ShaderParameter)) ?? Engine.const([]),
		sampling : [Nearest, Linear] ?? Linear,
		input : SurfaceInput(ctx) ?? ReadOnly,
	}

	## Draw one precompiled ps_5_0 pixel shader into a native child view.
	shader_view : ShaderViewProps(ctx) -> Elem(ctx)
	shader_view = |props| Engine.element(
		"ui-shader-view",
		[
			Engine.attr("shader", props.shader),
			Engine.attr("width", props.width.to_str()),
			Engine.attr("height", props.height.to_str()),
			Engine.attr_s("texture", props.texture.map(shader_texture)),
			Engine.attr_s("second-texture", props.second_texture.map(shader_texture)),
			Engine.attr_s("parameters", props.parameters.map(shader_parameters)),
			Engine.attr("sampling", if (props.sampling == Nearest) "nearest" else "linear"),
		].concat(graphic_attributes(props.description)).concat(Ui.surface_input(props.input)),
		[shader_raster("0", props.texture), shader_raster("1", props.second_texture)],
	)

	## An image's accessible label and dimensions in device-independent pixels.
	AssetImageProps(ctx) := { description : GraphicDescription(ctx), width : U32, height : U32, fit : [Stretch, Contain] ?? Stretch }

	## Show a packaged PNG, JPEG or BMP. An empty path clears the image.
	asset_image : Signal(ctx, Str), AssetImageProps(ctx) -> Elem(ctx)
	asset_image = |path, props|
		Engine.element(
			"ui-asset-image",
			[Engine.attr_s("asset", path), Engine.attr_s("graphic-present", path.map(|name| bool_word(!name.is_empty())))]
				.concat(image_attributes(props.width, props.height, props.fit == Contain))
				.concat(graphic_attributes(props.description)),
			[],
		)

	## Immutable image data in sRGB with straight (unpremultiplied) RGBA input.
	Raster :: { data : [Raw({ width : U32, height : U32, rgba : List(U8) }), Decoded(Engine.File)] }.{
		is_eq : Raster, Raster -> Bool
		is_eq = |left, right| left.data == right.data
	}

	RasterError : [InvalidDimensions, InvalidBytes, TooLarge, UnsupportedFormat, Unreadable, DecodeFailed, Unavailable]

	RasterProps : { width : U32, height : U32, rgba : List(U8) }

	## Validate a top-down row-major raster before it can be displayed.
	raster : RasterProps -> Try(Raster, RasterError)
	raster = |props| {
		width = props.width.to_u64()
		height = props.height.to_u64()
		if width == 0 or height == 0 {
			Err(InvalidDimensions)
		} else if width > 8192 or height > 8192 or width * height > 8388608 {
			Err(TooLarge)
		} else if props.rgba.len() != width * height * 4 {
			Err(InvalidBytes)
		} else {
			Ok(Raster.{ data: Raw(props) })
		}
	}

	## Decode a selected file capability on a Windows worker.
	decode_image : Engine.File -> Engine.Ask(Raster, RasterError)
	decode_image = |file| Engine.ask_of(DecodeImage.{}, file)

	ImageSource : [NoImage, RasterImage(Raster)]
	ImageProps(ctx) := {
		description : GraphicDescription(ctx),
		width : U32,
		height : U32,
		fit : [Stretch, Contain] ?? Stretch,
		input : SurfaceInput(ctx) ?? ReadOnly,
	}

	## Show immutable app-produced or decoded raster data.
	image : Signal(ctx, ImageSource), ImageProps(ctx) -> Elem(ctx)
	image = |source, props| {
		raster_input = Ui.with_raster_binding(source, |bind, attachment| Engine.element("ui-image-raster", [bind], [attachment]))
		attrs = image_attributes(props.width, props.height, props.fit == Contain)
		present = Engine.attr_s("graphic-present", source.map(|current| bool_word(current != NoImage)))
		Engine.element("ui-image", attrs.append(present).concat(graphic_attributes(props.description)).concat(Ui.surface_input(props.input)), [raster_input])
	}

	## The input identity crosses as a private typed port attachment.
	with_raster_binding : Signal(ctx, ImageSource), (Engine.Attr(Engine.In(ctx, U64)), Elem(Engine.In(ctx, U64)) -> Elem(Engine.In(ctx, U64))) -> Elem(ctx)
	with_raster_binding = |source, build| Engine.state(
		0.U64,
		|local| {
			update = { binding: local.signal(), source: source.lift() }.Engine.Signal
			bind = Engine.on("roc-raster-bind", local.set_value(|_, token| U64.from_str(token) ?? 0))
			attachment = Engine.watch(
				update,
				|{ binding, source: current }| if (binding == 0) [] else [Engine.command(attach_raster.send(raster_payload(binding, current)))],
				Engine.ignore({}),
			)
			build(bind, attachment)
		},
	)

	image_ports : {} -> List(Engine.PortRegistration)
	image_ports = |_| Port.registrations({ attach: attach_raster, decode: decode_image_port })

}

image_attributes : U32, U32, Bool -> List(Engine.Attr(ctx))
image_attributes = |width, height, contain| [
	Engine.attr("width", width.to_str()),
	Engine.attr("height", height.to_str()),
	Engine.attr("fit", if (contain) "contain" else "stretch"),
]

graphic_attributes : Ui.GraphicDescription(ctx) -> List(Engine.Attr(ctx))
graphic_attributes = |description| match description {
	Decorative => [Engine.attr("graphic-kind", "decorative")]
	Described(props) => [
		Engine.attr("graphic-kind", "described"),
		Engine.attr_s("label", props.label),
		Engine.attr_s(
			"graphic-details",
			props.details.map(
				|current| match current {
					NoDetails => ""
					Details(details) => details
				},
			),
		),
	]
}

RasterAttachment : { binding : U64, bytes : ImageWire.Bytes, files : List(Engine.File), height : U32, width : U32 }

attach_raster : Port.Command(RasterAttachment)
attach_raster = Port.command({ name: "roc-native-windows/bind-raster" })

raster_payload : U64, Ui.ImageSource -> RasterAttachment
raster_payload = |binding, source| {
	empty = { binding, bytes: ImageWire.Bytes.from_list([]), files: [], height: 0, width: 0 }
	match source {
		NoImage => empty
		RasterImage(Ui.Raster.{ data }) => match data {
			Raw({ width, height, rgba }) => { ..empty, width, height, bytes: ImageWire.Bytes.from_list(rgba) }
			Decoded(file) => { ..empty, files: [file] }
		}
	}
}

decode_image_port : Port.Call(Engine.File, { code : U8, files : List(Engine.File) })
decode_image_port = Port.call({ name: "roc-native-windows/decode-image", timeout: Ms(15000) })

DecodeImage := {}.{
	request : DecodeImage, Engine.File -> Engine.Request
	request = |_, file| decode_image_port.request(file)

	result : DecodeImage, Engine.Result -> Try(Ui.Raster, Ui.RasterError)
	result = |_, answer| match answer {
		PortAnswer(_) => match decode_image_port.result(answer) {
			Ok({ code: 0, files: [file] }) => Ok(Ui.Raster.{ data: Decoded(file) })
			Ok({ code: 1, .. }) => Err(Unreadable)
			Ok({ code: 2, .. }) => Err(UnsupportedFormat)
			Ok({ code: 3, .. }) => Err(TooLarge)
			Ok({ code: 4, .. }) => Err(DecodeFailed)
			_ => Err(Unavailable)
		}
		_ => Err(Unavailable)
	}
}

stack : Engine.Tag, Ui.StackProps, List(Elem(ctx)) -> Elem(ctx)
stack = |kind, props, children|
	Engine.element(
		kind,
		[
			Engine.attr("spacing", props.spacing.to_str()),
			Engine.attr("padding", props.padding.to_str()),
			Engine.attr("stack-below", props.stack_below.to_str()),
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

choice_control : Engine.Tag, Signal(ctx, Str), Signal(ctx, List(Ui.Choice)), Signal(ctx, Ui.Selection), HandlerOf(ctx, Ui.Selection), Signal(ctx, Bool), U32 -> Elem(ctx)
choice_control = |tag, label, choices, selected, on_select, enabled, rows|
	Engine.element(
		tag,
		[
			Engine.attr_s("label", label),
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

window_title : Ui.WindowTitle -> Str
window_title = |title| match title {
	AppTitle => "app"
	Title(text) => "text:${text}"
}

close_listener : HandlerOf(ctx, {}) -> Engine.Attr(ctx)
close_listener = |handler| Engine.on("close-request", handler.from_payload_by(Engine.const({}), |_, _| {}))

pointer_handler : [Down, Move, Up, Cancel], [NoHandler, Handle(HandlerOf(ctx, Engine.Pointer))] -> List(Engine.Attr(ctx))
pointer_handler = |phase, binding| match binding {
	NoHandler => []
	Handle(handler) => [Engine.on_pointer(phase, handler)]
}

shader_texture : Ui.ShaderTexture -> Str
shader_texture = |texture| match texture {
	NoTexture => ""
	AssetTexture(path) => "asset:${path}"
	RasterTexture(_) => "raster"
}

shader_raster : Str, Signal(ctx, Ui.ShaderTexture) -> Elem(ctx)
shader_raster = |slot, texture| {
	source = texture.map(
		|value| match value {
			RasterTexture(raster) => RasterImage(raster)
			_ => NoImage
		},
	)
	Ui.with_raster_binding(source, |binding, attachment|
		Engine.element("ui-shader-raster", [Engine.attr("slot", slot), binding], [attachment]))
}

shader_parameters : List(Ui.ShaderParameter) -> Str
shader_parameters = |parameters| Str.join_with(
	parameters.map(
		|parameter| {
			if parameter.name.contains("=") or parameter.name.contains("\n") or parameter.name.contains("\r") {
				"invalid shader parameter"
			} else {
				"${parameter.name}=${parameter.value.to_str()}"
			}
		},
	),
	"\n",
)

text_attributes : Ui.TextProps -> List(Engine.Attr(ctx))
text_attributes = |props| [
	Engine.attr("ellipsize-end", bool_word(props.overflow == EllipsizeEnd)),
	Engine.attr(
		"text-style",
		match props.style {
			Body => "body"
			Caption => "caption"
			Heading => "heading"
			Title => "title"
			Display => "display"
			Monospace => "monospace"
		},
	),
	Engine.attr(
		"text-tone",
		match props.tone {
			PrimaryText => "primary"
			SecondaryText => "secondary"
			AccentText => "accent"
		},
	),
	Engine.attr(
		"text-align",
		match props.align {
			Start => "start"
			Center => "center"
			End => "end"
		},
	),
].concat(font_attrs(props.font))

font_attrs : Ui.TextFont -> List(Engine.Attr(ctx))
font_attrs = |font| match font {
	SystemFont => []
	PackagedFont(props) => [
		Engine.attr("font-asset", props.asset),
		Engine.attr("font-family", props.family),
		Engine.attr(
			"font-face",
			match props.face {
				Regular => "regular"
				Bold => "bold"
				Italic => "italic"
				BoldItalic => "bold-italic"
			},
		),
	]
}
