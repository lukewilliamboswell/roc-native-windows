import pf.Ui

Stock := [].{
	Item : { name : Str, expected : U64, counted : U64, feedback : Ui.Feedback }
	Count : { expected : U64, counted : U64 }
	Summary : { matched : U64, short : U64, surplus : U64 }

	new : Str, U64 -> Item
	new = |name, expected| { name, expected, counted: expected, feedback: Clear }

	parse_count : Str -> Try(U64, Str)
	parse_count = |draft| match U64.from_str(draft.trim()) {
		Ok(counted) if counted <= 1_000_000 => Ok(counted)
		_ => Err("Enter a whole number from 0 to 1,000,000.")
	}

	recount : Item, Ui.CellEdit -> Item
	recount = |item, { draft }| match parse_count(draft) {
		Ok(counted) => { ..item, counted, feedback: Clear }
		Err(message) => { ..item, feedback: Invalid(message) }
	}

	paste : List({ key : U64, row : Item }), Ui.GridPaste -> Try(List({ key : U64, row : Item }), Str)
	paste = |visible, batch| {
		if (batch.columns != ["counted"]) return Err("Only the Counted column can be pasted.")
		var $changes = []
		var $index = batch.first_row - 1
		for line in batch.rows {
			held = visible.get($index) ? |_| "The pasted rows no longer fit the mounted page."
			draft = line.get(0) ? |_| "Each line needs one count."
			counted = parse_count(draft)?
			$changes = $changes.append({ key: held.key, row: { ..held.row, counted, feedback: Clear } })
			$index = $index + 1
		}
		Ok($changes)
	}

	count : Item -> Count
	count = |item| { expected: item.expected, counted: item.counted }

	reconcile : List(Count) -> Summary
	reconcile = |snapshot|
		snapshot.fold(
			{ matched: 0, short: 0, surplus: 0 },
			|total, item|
				if item.counted == item.expected {
					{ ..total, matched: total.matched + 1 }
				} else if item.counted < item.expected {
					{ ..total, short: total.short + item.expected - item.counted }
				} else {
					{ ..total, surplus: total.surplus + item.counted - item.expected }
				},
		)
}

expect Stock.recount(Stock.new("Paper", 4), { draft: " 08 " }).counted == 8
expect Stock.recount(Stock.new("Paper", 4), { draft: "-1" }).counted == 4
expect match Stock.recount(Stock.new("Paper", 4), { draft: "1000001" }).feedback {
	Invalid("Enter a whole number from 0 to 1,000,000.") => Bool.True
	_ => Bool.False
}
expect Stock.reconcile([{ expected: 4, counted: 7 }, { expected: 8, counted: 2 }, { expected: 3, counted: 3 }]) == { matched: 1, short: 6, surplus: 3 }
