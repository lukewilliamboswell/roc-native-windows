app [target] {
	fuzz: platform "https://github.com/lukewilliamboswell/roc-fuzz/releases/download/0.7.0/HBigHQhVZWEF2EkLxpYK35u8P6UNsAmPYxpNFTxXK1Br.tar.zst",
	logic: "../../logic/main.roc",
}

import fuzz.Fuzz
import logic.ReadingList

target = Fuzz.target_with({
	name: "reading-list-invariants",
	generator: Fuzz.list(Fuzz.str, 24),
	examples: [Fuzz.example({ name: "trimmed-duplicate", revision: "1", make: || [" Roc ", "", "Roc", "Guide"] })],
	show: Str.inspect,
	test: |titles| {
		items = titles.fold(ReadingList.empty, |list, title| ReadingList.update(list, Add(title)))
		expected = titles.map(|title| title.trim()).drop_if(Str.is_empty).fold([], |list, title| if (list.contains(title)) list else list.append(title))
		toggled = items.fold(items, |list, item| ReadingList.update(list, Toggle(item.title)))
		removed = expected.fold(toggled, |list, title| ReadingList.update(list, Remove(title)))
		if items.map(|item| item.title) == expected and ReadingList.remaining(items) == expected.len() and ReadingList.remaining(toggled) == 0 and removed == [] {
			Fuzz.keep
		} else {
			crash "reading list violated its add, toggle or remove rules"
		}
	},
})
