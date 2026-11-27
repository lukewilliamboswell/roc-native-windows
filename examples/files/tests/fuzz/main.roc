app [target] {
	fuzz: platform "https://github.com/lukewilliamboswell/roc-fuzz/releases/download/0.7.0/HBigHQhVZWEF2EkLxpYK35u8P6UNsAmPYxpNFTxXK1Br.tar.zst",
	logic: "../../logic/main.roc",
}

import fuzz.Fuzz
import logic.Inspector

target = Fuzz.target_with({
	name: "file-preview-and-digest",
	generator: Fuzz.bytes,
	examples: [Fuzz.example({ name: "binary-and-newline", revision: "1", make: || [0, 10, 255] })],
	show: Str.inspect,
	test: |bytes| {
		preview = Inspector.preview(bytes.take_first(Inspector.preview_limit))
		digits = Inspector.hex(bytes).to_utf8()
		hexadecimal = digits.all(|byte| (byte >= '0' and byte <= '9') or (byte >= 'a' and byte <= 'f'))
		if !preview.contains("\r") and !preview.contains("\u(0000)") and digits.len() == bytes.len() * 2 and hexadecimal {
			Fuzz.keep
		} else {
			crash "preview contained a CR or NUL, or hex output was malformed"
		}
	},
})
