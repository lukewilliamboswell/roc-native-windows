## The display state for a file selection and its asynchronous results.
Inspector := [].{
	Display : { status : Str, preview : Str, digest : Str }
	Action : [Choose(Str, U64), Refused, Clear, Preview(Try(List(U8), Str)), Digest(Try(List(U8), Str))]

	preview_limit : U64
	preview_limit = 256

	idle : Display
	idle = { status: "Choose a file to preview its contents.", preview: "", digest: "" }

	update : Display, Action -> Display
	update = |display, action|
		match action {
			Choose(name, size) => { status: "${name} · ${size.to_str()} bytes", preview: "Reading…", digest: "Hashing…" }
			Refused => { ..Inspector.idle, status: "Could not open the selected file." }
			Clear => Inspector.idle
			Preview(Ok(bytes)) => { ..display, preview: Inspector.preview(bytes) }
			Preview(Err(problem)) => { ..display, preview: "Read failed: ${problem}" }
			Digest(Ok(bytes)) => { ..display, digest: "SHA-256: ${Inspector.hex(bytes)}" }
			Digest(Err(problem)) => { ..display, digest: "Digest failed: ${problem}" }
		}

	preview : List(U8) -> Str
	preview = |bytes|
		if bytes.is_empty() {
			"The file is empty."
		} else {
			Str.join_with(Str.from_utf8_lossy(bytes).split_on("\n").map(|line| line.trim()), " ")
		}

	hex : List(U8) -> Str
	hex = |bytes| {
		digits = "0123456789abcdef".to_utf8()
		Str.from_utf8_lossy(
			bytes.fold([], |out, byte| out.append(digits.get((byte // 16).to_u64()) ?? '0').append(digits.get((byte % 16).to_u64()) ?? '0')),
		)
	}
}

expect Inspector.hex([0, 15, 255]) == "000fff"
expect Inspector.preview("hello\n world".to_utf8()) == "hello world"
