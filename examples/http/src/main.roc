app [main] { pf: platform "../../../platform/main.roc" }

import pf.Clipboard
import pf.System
import pf.Engine exposing [Elem, Signal]
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Request : { url : Str, serial : U64, phase : Phase }

Phase : [Idle, Starting, Fetching(StartMark), Finishing(StartMark, Transfer)]

StartMark : Try(System.Mark, System.Failure)

Transfer : [Received({ status : U16, size : U64, preview : Str }), Failed(Str)]

store = Engine.store(
	HttpDemo,
	{
		url: Engine.cell("https://example.com"),
		request: Engine.cell(empty_request),
		reference: Engine.cell(""),
		completed: Engine.cell(""),
		copies: Engine.cell(Engine.outbox({})),
		status: Engine.cell("Enter a URL and press Fetch."),
		body: Engine.cell(""),
	}.Engine.Cells,
)

cells = store.cells()

HttpStyle := {
	page : Ui.StackProps ?? { padding: 16, spacing: 12 },
	header : Ui.StackProps ?? { spacing: 12 },
	title : Ui.TextProps ?? { style: Title },
	response_panel : Ui.PanelProps ?? { border: Outline, width: Fill },
	response_status : Ui.TextProps ?? { style: Heading },
	response_body : Ui.TextProps ?? {
		style: Monospace,
		font: PackagedFont({ asset: "fonts/IBMPlexMono-Regular.ttf", family: "IBM Plex Mono" }),
	},
}

empty_request : Request
empty_request = { url: "", serial: 0, phase: Idle }

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	design = HttpStyle.{}
	request = cells.request.signal()
	progress = request.map(|pending| if (pending.phase != Idle) Ui.Progress.Indeterminate else Ui.Progress.Idle)
	url = cells.url.signal()
	start = { url, previous: request }.Signal.map(|{ url: typed, previous }| { url: typed.trim(), serial: previous.serial + 1, phase: Starting })
	details = { completed: cells.completed.signal(), reference: cells.reference.signal() }.Signal.map(
		|{ completed, reference }|
			if completed.is_empty() {
				""
			} else {
				id = if (reference.is_empty()) "unavailable" else reference
				"${completed}\nReference: ${id}"
			},
	)
	content = Ui.vstack(
		design.page,
		[
			Ui.hstack(
				design.header,
				[
					Ui.asset_image("http.bmp", { description: Decorative, width: 28, height: 28 }),
					Ui.text_with("HTTP", design.title),
				],
			),
			Ui.hstack(
				{},
				[
					Ui.text_field(url, { label: "URL", placeholder: "https://example.com", on_input: Engine.handle(|typed| [cells.url.set(typed)]) }),
					Ui.button("Fetch", { enter_action: WindowDefault, enabled: start.map(|r| !r.url.is_empty()), on_press: Engine.handle_by(start, |r, _| [cells.request.set(r), cells.status.set("Fetching…"), cells.body.set(""), cells.reference.set(""), cells.completed.set("")]) }),
					Ui.button("Cancel", { enabled: request.map(|r| r.phase != Idle), on_press: Engine.handle(|_| [cells.request.update(|r| { ..r, phase: Idle }), cells.status.set("Cancelled.")]) }),
				],
			),
			Ui.progress_bar(progress, { label: "Request progress" }),
			Ui.panel(
				design.response_panel,
				Ui.vstack(
					{},
					[
						Ui.status_text(cells.status.signal(), { text: design.response_status }),
						Ui.text_with(cells.body.signal(), design.response_body),
					],
				),
			),
			Ui.button(
				"Copy request details",
				{
					enabled: details.map(|value| !value.is_empty()),
					on_press: Engine.handle_by(details, |value, _| [cells.copies.update(|outbox| outbox.post(value))]),
				},
			),
			Engine.on_post(cells.copies.signal(), |value| [Clipboard.text(value)], Engine.ignore({})),
			Engine.watch_latest(
				request,
				|r| if (r.phase == Starting) [System.monotonic({})] else [],
				Engine.react(|r, mark| [cells.request.set({ ..r, phase: Fetching(mark) })]),
			),
			Engine.watch_latest(
				request.map(|r| if (r.phase == Idle) 0 else r.serial),
				|serial| if (serial == 0) [] else [System.secure_random(16)],
				Engine.react(
					|_, result| [
						cells.reference.set(
							match result {
								Ok(bytes) => hex(bytes)
								Err(_) => ""
							},
						),
					],
				),
			),
			Engine.watch_latest(
				request,
				|r| match r.phase {
					Fetching(_) => [Engine.http({ url: r.url, timeout: Ms(15_000), cache: NoStore })]
					_ => []
				},
				Engine.react(answered),
			),
			Engine.watch_latest(
				request,
				|r| match r.phase {
					Finishing(_, _) => [System.monotonic({})]
					_ => []
				},
				Engine.react(completed),
			),
		],
	)
	Ui.window({ taskbar_progress: progress }, content)
}

answered : Request, Try(Engine.Response, Engine.HttpFailure) -> List(Engine.Write(_))
answered = |request, result| {
	transfer = match result {
		Ok(response) => Received({ status: response.status, size: response.body.count_utf8_bytes(), preview: preview(response.body) })
		Err(Network(message)) => Failed("Network error: ${message}")
		Err(Timeout) => Failed("The request timed out.")
		Err(Refused) => Failed("The request was refused.")
	}
	match request.phase {
		Fetching(mark) => [cells.request.set({ ..request, phase: Finishing(mark, transfer) })]
		_ => []
	}
}

completed : Request, StartMark -> List(Engine.Write(_))
completed = |request, finished| match request.phase {
	Finishing(started, transfer) => {
		elapsed = match (started, finished) {
			(Ok(first), Ok(last)) => match last.elapsed_ms_since(first) {
				Ok(ms) => "${ms.to_str()} ms"
				Err(_) => "elapsed time unavailable"
			}
			_ => "elapsed time unavailable"
		}
		writes = match transfer {
			Received(reply) => {
				base = "HTTP ${reply.status.to_str()} · ${reply.size.to_str()} bytes"
				[cells.status.set("${base} · ${elapsed}"), cells.completed.set("${base}\nElapsed: ${elapsed}"), cells.body.set(reply.preview)]
			}
			Failed(message) => [cells.status.set(message), cells.completed.set("${message}\nElapsed: ${elapsed}")]
		}
		writes.append(cells.request.set({ ..request, phase: Idle }))
	}
	_ => []
}

hex : List(U8) -> Str
hex = |bytes| {
	digits = "0123456789abcdef".to_utf8()
	to_digit = |n| digits.get(n.to_u64()) ?? 48
	Str.from_utf8_lossy(bytes.fold([], |out, byte| out.append(to_digit(byte // 16)).append(to_digit(byte % 16))))
}

preview : Str -> Str
preview = |body| Str.from_utf8_lossy(body.to_utf8().take_first(200))

expect preview("a\n  b") == "a\n  b"
