app [main, native] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, Signal]
import pf.Win

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

native = Win.Features.use([Win.Features.http!, Win.Features.image_codecs!, Win.Features.taskbar!, Win.Features.clipboard!])

Request : { url : Str, serial : U64, phase : Phase }

Phase : [Idle, Starting, Fetching(StartMark), Finishing(StartMark, Transfer)]

StartMark : Try(Win.System.Mark, Win.System.Failure)

Transfer : [Received({ status : U16, size : U64, preview : Str }), Failed(Str)]

DiagnosticReference : [AwaitingReference, NoReference, Reference(Str)]

store = Engine.store(
	HttpDemo,
	{
		url: Engine.cell("https://example.com"),
		request: Engine.cell(empty_request),
		reference_for: Engine.cell(0.U64),
		reference: Engine.cell(NoReference),
		completed: Engine.cell(""),
		status: Engine.cell("Enter a URL and press Fetch."),
		body: Engine.cell(""),
	}.Engine.Cells,
)

cells = store.cells()

HttpStyle := {
	appearance : Win.Ui.AppearanceProps ?? { accent: CustomAccent({ red: 127, green: 46, blue: 84 }) },
	page : Win.Ui.StackProps ?? { padding: 14, spacing: 10 },
	header : Win.Ui.StackProps ?? { spacing: 10 },
	title : Win.Ui.TextProps ?? { style: Title, tone: AccentText },
	request_panel : Win.Ui.PanelProps ?? { surface: AccentTint, border: Outline, width: Fill, padding: 10 },
	request : Win.Ui.StackProps ?? { spacing: 6 },
	request_actions : Win.Ui.StackProps ?? { spacing: 8, stack_below: 260 },
	request_progress : Win.Ui.PanelProps ?? { surface: AccentTint, width: Fill, padding: 0 },
	response_panel : Win.Ui.PanelProps ?? { border: Outline, width: Fill, padding: 12 },
	response : Win.Ui.StackProps ?? { spacing: 8 },
	response_status : Win.Ui.TextProps ?? { style: Heading, tone: AccentText },
	response_body : Win.Ui.TextProps ?? {
		style: Monospace,
		font: PackagedFont({ asset: Win.asset("response-font"), family: "IBM Plex Mono" }),
	},
}

empty_request : Request
empty_request = { url: "", serial: 0, phase: Idle }

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	design = HttpStyle.{}
	request = cells.request.signal()
	progress = request.map(|pending| if (pending.phase != Idle) Win.Ui.Progress.Indeterminate else Win.Ui.Progress.Idle)
	url = cells.url.signal()
	start = { url, previous: request }.Signal.map(|{ url: typed, previous }| { url: typed.trim(), serial: previous.serial + 1, phase: Starting })
	details = { completed: cells.completed.signal(), reference: cells.reference.signal() }.Signal.map(
		|{ completed, reference }| match reference {
			_ if completed.is_empty() => ""
			AwaitingReference => ""
			NoReference => "${completed}\nReference: unavailable"
			Reference(id) => "${completed}\nReference: ${id}"
		},
	)
	content = Win.Ui.vstack(
		design.page,
		[
			Win.Ui.hstack(
				design.header,
				[
					Win.Ui.asset_image(Engine.const(Win.asset("app-art")), { description: Decorative, width: 28, height: 28 }),
					Win.Ui.text_with("HTTP", design.title),
				],
			),
			Win.Ui.panel(
				design.request_panel,
				Win.Ui.vstack(
					design.request,
					[
						Win.Ui.text_field(url, { label: "URL", placeholder: "https://example.com", on_input: Engine.handle(|typed| [cells.url.set(typed)]) }),
						Win.Ui.hstack(
							design.request_actions,
							[
								Win.Ui.button(
									"Fetch",
									{
										enter_action: WindowDefault,
										enabled: start.map(|r| !r.url.is_empty()),
										on_press: Engine.handle_by(
											start,
											|r, _| [
												cells.request.set(r),
												cells.status.set("Fetching…"),
												cells.body.set(""),
												cells.reference_for.set(r.serial),
												cells.reference.set(AwaitingReference),
												cells.completed.set(""),
											],
										),
									},
								),
								Win.Ui.button(
									"Cancel",
									{
										enabled: request.map(|r| r.phase != Idle),
										on_press: Engine.handle(
											|_| [
												cells.request.update(|r| { ..r, phase: Idle }),
												cells.reference_for.set(0),
												cells.status.set("Cancelled."),
											],
										),
									},
								),
								Win.Ui.panel(
									design.request_progress,
									Win.Ui.progress_bar(progress, { label: "Request progress" }),
								),
							],
						),
					],
				),
			),
			Win.Ui.panel(
				design.response_panel,
				Win.Ui.vstack(
					design.response,
					[
						Win.Ui.status_text(cells.status.signal(), { text: design.response_status }),
						Win.Ui.text_with(cells.body.signal(), design.response_body),
						Win.Ui.copy_button(
							"Copy request details",
							{
								enabled: details.map(|value| !value.is_empty()),
								text: details,
								on_result: Engine.handle(|_| []),
							},
						),
					],
				),
			),
			Engine.watch_latest(
				request,
				|r| if (r.phase == Starting) [Win.System.monotonic({})] else [],
				Engine.react(|r, mark| [cells.request.set({ ..r, phase: Fetching(mark) })]),
			),
			Engine.watch_latest(
				cells.reference_for.signal(),
				|serial| if (serial == 0) [] else [Win.System.secure_random(16)],
				Engine.react(
					|_, result| [
						cells.reference.set(
							match result {
								Ok(bytes) => Reference(hex(bytes))
								Err(_) => NoReference
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
					Finishing(_, _) => [Win.System.monotonic({})]
					_ => []
				},
				Engine.react(completed),
			),
		],
	)
	Win.Ui.window({ appearance: design.appearance, taskbar_progress: progress }, content)
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
