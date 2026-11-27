import Engine

## App-authored clipboard requests. Windows denies requests without native
## clipboard authority. Use Win.Ui.copy_button for an explicit Copy action.
Clipboard := [].{
	text : Str -> Engine.Ask({}, [NotAllowed, Unavailable, Refused])
	text = |value| Engine.ask_of(CopyText.{}, value)
}

CopyText := {}.{
	request : CopyText, Str -> Engine.Request
	request = |_, text| ClipboardWrite({ text, html: Plain })

	result : CopyText, Engine.Result -> Try({}, [NotAllowed, Unavailable, Refused])
	result = |_, answer|
		match answer {
			Copied => Ok({})
			ClipboardFailed(reason) => Err(reason)
			_ => Err(Refused)
		}
}

expect Clipboard.text("hello").sent() == ClipboardWrite({ text: "hello", html: Plain })
