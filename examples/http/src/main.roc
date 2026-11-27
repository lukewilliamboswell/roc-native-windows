app [main] { pf: platform "../../../platform/main.roc" }

import pf.Engine exposing [Elem, Signal]
import pf.Ui

main : Engine.Program(_)
main = Engine.program_store({}, |_boot| store, |_boot| view())

Request : { url : Str, serial : U64, active : Bool }

store = Engine.store(
	HttpDemo,
	{
		url: Engine.cell("https://example.com"),
		request: Engine.cell({ url: "", serial: 0, active: Bool.False }),
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

view : () -> Elem(Engine.StoreRoot(_))
view = || {
	design = HttpStyle.{}
	request = cells.request.signal()
	progress = request.map(|pending| if (pending.active) Ui.Progress.Indeterminate else Ui.Progress.Idle)
	url = cells.url.signal()
	start = { url, previous: request }.Signal.map(|{ url: typed, previous }| { url: typed.trim(), serial: previous.serial + 1, active: Bool.True })
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
					Ui.button("Fetch", { enter_action: WindowDefault, enabled: start.map(|r| !r.url.is_empty()), on_press: Engine.handle_by(start, |r, _| [cells.request.set(r), cells.status.set("Fetching…"), cells.body.set("")]) }),
					Ui.button("Cancel", { enabled: request.map(|r| r.active), on_press: Engine.handle(|_| [cells.request.update(|r| { ..r, active: Bool.False }), cells.status.set("Cancelled.")]) }),
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
			Engine.watch_latest(
				request,
				|r| if (r.active) [Engine.http({ url: r.url, timeout: Ms(15_000), cache: NoStore })] else [],
				Engine.react(answered),
			),
		],
	)
	Ui.window({ taskbar_progress: progress }, content)
}

answered : Request, Try(Engine.Response, Engine.HttpFailure) -> List(Engine.Write(_))
answered = |request, result| {
	update = match result {
		Ok(response) => [cells.status.set("HTTP ${response.status.to_str()} · ${response.body.count_utf8_bytes().to_str()} bytes"), cells.body.set(preview(response.body))]
		Err(Network(message)) => [cells.status.set("Network error: ${message}")]
		Err(Timeout) => [cells.status.set("The request timed out.")]
		Err(Refused) => [cells.status.set("The request was refused.")]
	}
	update.append(cells.request.set({ ..request, active: Bool.False }))
}

preview : Str -> Str
preview = |body| Str.from_utf8_lossy(body.to_utf8().take_first(200))

expect preview("a\n  b") == "a\n  b"
