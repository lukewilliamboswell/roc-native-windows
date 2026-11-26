## Pure decisions shared by the desktop view, behaviour specs and fuzz suite.
ReadingList := [].{
	Item : { title : Str, read : Bool }
	Model : List(Item)
	Action : [Add(Str), Toggle(Str), Remove(Str)]

	empty : Model
	empty = []

	update : Model, Action -> Model
	update = |items, action|
		match action {
			Add(raw) => {
				title = raw.trim()
				if title.is_empty() or items.any(|item| item.title == title) {
					items
				} else {
					items.append({ title, read: Bool.False })
				}
			}
			Toggle(title) => items.map(|item| if (item.title == title) { ..item, read: !item.read } else item)
			Remove(title) => items.drop_if(|item| item.title == title)
		}

	remaining : Model -> U64
	remaining = |items| items.keep_if(|item| !item.read).len()
}

expect ReadingList.update(ReadingList.empty, Add("  Roc guide  ")) == [{ title: "Roc guide", read: Bool.False }]
