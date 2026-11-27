import pf.Ui

Stock := [].{
	Item : { name : Str, expected : U64, counted : U64, feedback : Ui.Feedback }
	Count : { expected : U64, counted : U64 }
	Summary : { matched : U64, short : U64, surplus : U64 }

	new : Str, U64 -> Item
	new = |name, expected| { name, expected, counted: expected, feedback: Clear }

	recount : Item, Ui.CellEdit -> Item
	recount = |item, { draft }|
		match U64.from_str(draft.trim()) {
			Ok(counted) if counted <= 1_000_000 => { ..item, counted, feedback: Clear }
			_ => { ..item, feedback: Invalid("Enter a whole number from 0 to 1,000,000.") }
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
