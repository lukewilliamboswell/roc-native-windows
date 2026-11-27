//! Execute a checked spec on the app's own UI thread.

use super::{APP, App, FAILURE, last_error, mount};
use crate::Result;
use crate::capture::{self, Recorder};
use crate::civil::{Kind as CivilKind, Value as CivilValue};
use crate::effects::SpecOptions;
use crate::spec_plan::{MAX_PLAN_BYTES, Mode, PROTOCOL, Plan, Step, valid_file_name};
use crate::view::Control;
use crate::wire::{Key, Kind, ROOT};
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::UpdateWindow;
use windows_sys::Win32::UI::Controls::TCN_SELCHANGE;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const SAMPLE_RATE: u32 = 10;
const WORK_LIMIT: Duration = Duration::from_secs(30);

pub(super) fn run(window: HWND, plan_path: &Path) -> Result<()> {
    let mut text = String::new();
    File::open(plan_path)
        .and_then(|file| file.take(MAX_PLAN_BYTES + 1).read_to_string(&mut text))
        .map_err(|err| format!("cannot read spec plan: {err}"))?;
    let plan = Plan::read(&text)?;
    let output = read_path("ROC_WINDOWS_SPEC_OUT")?;
    let workspace = plan_path
        .parent()
        .ok_or("spec plan needs a parent directory")?;
    let (options, size) = read_setup(&plan, workspace, &output)?;
    let failure_picture =
        std::env::var("ROC_WINDOWS_SPEC_FAILURE_SCREENSHOT").is_ok_and(|value| value == "1");
    let recording = std::env::var("ROC_WINDOWS_SPEC_RECORD").unwrap_or_default();
    let mut capture = Capture::new(&recording, &plan.mode, output)?;
    size_window(window, size)?;
    mount(window, Some(options))?;
    if plan.mode == Mode::Demo
        || failure_picture
        || plan.steps.iter().any(|step| step.command == "screenshot")
    {
        // SAFETY: The runner owns this window throughout the scenario.
        unsafe {
            ShowWindow(window, SW_SHOWNOACTIVATE);
            UpdateWindow(window);
        }
    }

    let started = Instant::now();
    capture.checkpoint(window, started, 0)?;
    for (index, step) in plan
        .steps
        .iter()
        .filter(|step| !step.command.starts_with("setup-"))
        .enumerate()
    {
        let result = perform(window, step, &mut capture, started)
            .and_then(|()| pump(window, &mut capture, started))
            .and_then(|()| capture.checkpoint(window, started, index + 1));
        if let Err(problem) = result {
            if failure_picture {
                let path = capture.output.join("failure.png");
                match capture::screenshot(window, &path) {
                    Ok(()) => eprintln!("Failure screenshot: {}", path.display()),
                    Err(error) => eprintln!("could not take failure screenshot: {error}"),
                }
            }
            return Err(step.problem(problem));
        }
    }
    capture.finish(started.elapsed())?;
    println!("PASS {}", plan.name);
    println!("{PROTOCOL} done");
    Ok(())
}

fn read_path(var: &str) -> Result<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("{var} must be an absolute path"))
}

fn read_setup(
    plan: &Plan<'_>,
    workspace: &Path,
    output: &Path,
) -> Result<(SpecOptions, (i32, i32))> {
    let demo = plan.mode == Mode::Demo;
    let mut options = SpecOptions {
        storage: workspace.join("storage"),
        output: output.to_owned(),
        local: Vec::new(),
        session: Vec::new(),
        initial_time: 0,
        live_time: demo,
        live_http: demo,
        worker_limit: 32,
    };
    let mut size = (640, 520);
    for step in plan
        .steps
        .iter()
        .take_while(|step| step.command.starts_with("setup-"))
    {
        let parse = |problem| step.problem(problem);
        match (step.command, step.args.as_slice()) {
            ("setup-initial-time", [value]) => {
                options.initial_time = number(value).map_err(parse)?
            }
            ("setup-live-effects", []) => {
                options.live_time = true;
                options.live_http = true;
            }
            ("setup-live-effects", ["time"]) => options.live_time = true,
            ("setup-live-effects", ["http"]) => options.live_http = true,
            ("setup-local-storage", [key, value]) => {
                options.local.push(((*key).into(), (*value).into()))
            }
            ("setup-session-storage", [key, value]) => {
                options.session.push(((*key).into(), (*value).into()))
            }
            ("setup-worker-pool", [count]) => {
                options.worker_limit = number(count).map_err(parse)?;
                if !(1..=32).contains(&options.worker_limit) {
                    return Err(step.problem("worker-pool needs a count from 1 through 32"));
                }
            }
            ("setup-window-size", [width, height]) => {
                size = (
                    number(width).map_err(parse)?,
                    number(height).map_err(parse)?,
                );
                if !(1..=4096).contains(&size.0) || !(1..=4096).contains(&size.1) {
                    return Err(step.problem("window dimensions must be from 1 through 4096"));
                }
            }
            _ => return Err(step.problem("unknown setup or invalid arguments")),
        }
    }
    Ok((options, size))
}

fn size_window(window: HWND, (width, height): (i32, i32)) -> Result<()> {
    // Measure the real frame, including the standard vertical scrollbar.
    // SAFETY: The HWND is owned by the UI thread and both rectangles are writable.
    unsafe {
        let dpi = GetDpiForWindow(window).max(96);
        let dip = |value: i32| ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32;
        let mut outer = RECT::default();
        let mut client = RECT::default();
        if GetWindowRect(window, &mut outer) == 0 || GetClientRect(window, &mut client) == 0 {
            return Err(last_error("measure the spec window frame"));
        }
        let frame_width = outer.right - outer.left - client.right;
        let frame_height = outer.bottom - outer.top - client.bottom;
        if SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            dip(width) + frame_width,
            dip(height) + frame_height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        ) == 0
        {
            return Err(last_error("size the spec window"));
        }
    }
    Ok(())
}

fn with_app<T>(run: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    APP.with(|slot| {
        run(slot
            .borrow_mut()
            .as_mut()
            .ok_or("spec app stopped before completion")?)
    })
}

fn pump(window: HWND, capture: &mut Capture, started: Instant) -> Result<()> {
    let mut message = MSG::default();
    // SAFETY: We are on the app UI thread with an initialized message record.
    unsafe {
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if message.message == WM_QUIT {
                return Err(FAILURE
                    .with(|slot| slot.borrow_mut().take())
                    .unwrap_or_else(|| "spec window closed before completion".into()));
            }
            if !(WM_KEYFIRST..=WM_KEYLAST).contains(&message.message) {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    with_app(App::poll)?;
    capture.sample(window, started)
}

fn perform(window: HWND, step: &Step<'_>, capture: &mut Capture, started: Instant) -> Result<()> {
    match (step.command, step.args.as_slice()) {
        ("wait", [value]) => {
            let duration = Duration::from_millis(number(value)?);
            if duration > capture::MAX_RECORDING_DURATION {
                return Err("wait exceeds 60 seconds".into());
            }
            let begin = Instant::now();
            while begin.elapsed() < duration {
                pump(window, capture, started)?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
        ("await-work", []) => {
            let begin = Instant::now();
            loop {
                pump(window, capture, started)?;
                if with_app(|app| Ok(app.effects.work_done()))? {
                    // The worker publishes its answer before lowering running;
                    // this drain catches racing completions and their follow-up work.
                    pump(window, capture, started)?;
                    if with_app(|app| Ok(app.effects.work_done()))? {
                        return Ok(());
                    }
                }
                if begin.elapsed() >= WORK_LIMIT {
                    return Err("work did not finish within 30 seconds".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        ("screenshot", [stem]) => capture.screenshot(window, stem),
        _ => with_app(|app| app.spec_step(step)),
    }
}

struct Capture {
    output: PathBuf,
    recorder: Option<Recorder>,
    png_sequence: bool,
}

impl Capture {
    fn new(format: &str, mode: &Mode, output: PathBuf) -> Result<Self> {
        let (recorder, png_sequence) = match (format, mode) {
            ("", _) => (None, false),
            ("gif", Mode::Demo) => (
                Some(Recorder::new(&output.join("demo.gif"), SAMPLE_RATE)?),
                false,
            ),
            ("png", Mode::Demo) => (None, true),
            _ => return Err("recording requires a demo and gif or png output".into()),
        };
        Ok(Self {
            output,
            recorder,
            png_sequence,
        })
    }

    fn sample(&mut self, window: HWND, started: Instant) -> Result<()> {
        if let Some(recorder) = &mut self.recorder {
            recorder.record_sample(window, started.elapsed())?;
        }
        Ok(())
    }

    fn checkpoint(&mut self, window: HWND, started: Instant, index: usize) -> Result<()> {
        if self.png_sequence {
            if index > 600 {
                return Err("PNG recording exceeds 600 steps".into());
            }
            capture::screenshot(window, &self.output.join(format!("frame-{index:04}.png")))?;
        }
        self.sample(window, started)
    }

    fn screenshot(&self, window: HWND, stem: &str) -> Result<()> {
        if !valid_file_name(stem) {
            return Err("screenshot needs an ordinary filename stem".into());
        }
        capture::screenshot(window, &self.output.join(format!("{stem}.png")))
    }

    fn finish(self, elapsed: Duration) -> Result<()> {
        if let Some(recorder) = self.recorder {
            recorder.finish(elapsed)?;
        }
        Ok(())
    }
}

impl App {
    fn spec_step(&mut self, step: &Step<'_>) -> Result<()> {
        match (step.command, step.args.as_slice()) {
            ("files", _) => self.spec_files(&step.args),
            ("folder", []) => Ok(()),
            ("folder", [name]) => self.spec_folder(name),
            ("clipboard-busy", []) => {
                self.effects.clipboard_busy = true;
                Ok(())
            }
            ("expect-clipboard", [count, text]) => {
                expect(self.effects.clipboard_writes, number(count)?)?;
                expect(self.effects.clipboard_text.as_str(), *text)
            }
            ("stub-http", [url, status, body, count, headers @ ..]) => {
                let count: usize = number(count)?;
                if count.checked_mul(2) != Some(headers.len()) {
                    return Err("invalid header count".into());
                }
                self.effects.http_stub(url, number(status)?, headers, body)
            }
            ("fail-http", [kind @ ("timeout" | "network"), detail]) => {
                self.effects.http_failure(*kind == "timeout", detail)
            }
            ("run-effect", [count]) => self.run_simulated(number(count)?),
            ("advance-time", [milliseconds]) => {
                self.effects.simulated.advance(number(milliseconds)?)?;
                while self.effects.expire_simulated_http() {
                    self.poll()?;
                }
                Ok(())
            }
            ("tick-interval", [ms]) => {
                let effects = self.effects.simulated.timers(number(ms)?, true);
                if effects.is_empty() {
                    return Err(format!("no {ms} ms interval is live"));
                }
                for number in effects {
                    if let Some(response) = self.effects.simulated.tick_interval(number) {
                        self.effects.send_simulated(response);
                        self.poll()?;
                    }
                }
                Ok(())
            }
            ("fire-timer", [ms]) => {
                let effects = self.effects.simulated.timers(number(ms)?, false);
                self.run_simulated(
                    *effects
                        .first()
                        .ok_or_else(|| format!("no {ms} ms timer is pending"))?,
                )
            }
            ("expect-cancelled", [count]) => expect(self.effects.cancelled(number(count)?)?, true),
            ("expect-pending-effects", [count]) => expect(self.effects.pending(), number(count)?),
            ("expect-interval", [ms, count]) => {
                expect(self.effects.interval_count(number(ms)?), number(count)?)
            }
            ("expect-request", [index, count, fields @ ..]) => {
                let expected_count: usize = number(count)?;
                if expected_count.checked_mul(2) != Some(fields.len()) {
                    return Err("invalid request field count".into());
                }
                let req = self.effects.simulated.request(number(index)?)?;
                for pair in fields.as_chunks::<2>().0 {
                    let wanted = pair[1];
                    match pair[0] {
                        "method" => expect(req.method.as_str(), wanted)?,
                        "url" => expect(req.url.as_str(), wanted)?,
                        "body" => expect(req.body.as_str(), wanted)?,
                        "timeout" => expect(req.timeout, number(wanted)?)?,
                        name => return Err(format!("cannot inspect request field {name:?}")),
                    }
                }
                Ok(())
            }
            (
                "click" | "fill" | "toggle" | "press-enter" | "expect-visible" | "expect-absent"
                | "expect-text" | "expect-checked" | "expect-disabled" | "select"
                | "expect-selection" | "set-date" | "expect-date" | "set-time" | "expect-time",
                _,
            ) => self.control_step(step),
            (
                "select-row"
                | "activate-row"
                | "press-column"
                | "expect-row-selected"
                | "expect-list-rows",
                _,
            ) => self.control_step(step),
            (
                "focus-cell"
                | "begin-cell"
                | "fill-cell"
                | "commit-cell"
                | "cancel-cell"
                | "expect-cell-focused"
                | "expect-cell-text"
                | "expect-cell-feedback"
                | "select-cell-range"
                | "paste-grid",
                _,
            ) => self.control_step(step),
            _ => Err("unknown step or invalid arguments".into()),
        }
    }

    fn run_simulated(&mut self, number: usize) -> Result<()> {
        let response = self.effects.simulated.run(number)?;
        self.effects.send_simulated(response);
        self.poll()
    }

    fn spec_files(&mut self, args: &[&str]) -> Result<()> {
        let key = self.one_control(&[("file-button", "")])?;
        if self.surface.spec_disabled(key)? {
            return Err("the file picker is disabled".into());
        }
        let (node, listener) = self
            .tree
            .event(key, "change")
            .ok_or("file picker has no handler")?;
        let paths = args
            .iter()
            .map(|name| fixture_path(name))
            .collect::<Result<Vec<_>>>()?;
        if paths.len() > 1
            && !self
                .tree
                .node(key)
                .attr("multiple")
                .is_some_and(super::enabled_flag)
        {
            return Err("the file picker accepts one file".into());
        }
        self.effects.deliver_paths(node, listener.handler, paths)?;
        self.poll()
    }

    fn spec_folder(&mut self, fixture: &str) -> Result<()> {
        let key = self.one_control(&[("folder-button", "")])?;
        if self.surface.spec_disabled(key)? {
            return Err("the folder picker is disabled".into());
        }
        let (node, listener) = self
            .tree
            .event(key, "change")
            .ok_or("folder picker has no handler")?;
        self.effects
            .deliver_folder(node, listener.handler, Some(fixture_path(fixture)?))?;
        self.poll()
    }

    fn control_step(&mut self, step: &Step<'_>) -> Result<()> {
        let (count, rest) = step.args.split_first().ok_or("step needs a locator")?;
        let count: usize = number(count)?;
        let width = count
            .checked_mul(2)
            .filter(|&width| width != 0)
            .ok_or("invalid locator size")?;
        let (encoded, args) = rest.split_at_checked(width).ok_or("truncated locator")?;
        let selectors: Vec<_> = encoded
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (pair[0], pair[1]))
            .collect();
        if step.command == "expect-absent" && args.is_empty() {
            return expect(self.controls(&selectors)?.len(), 0);
        }
        let key = self.one_control(&selectors)?;
        match (step.command, args) {
            ("expect-visible", []) => Ok(()),
            ("expect-text", [expected]) => expect(self.surface.spec_text(key)?.as_str(), *expected),
            ("expect-checked", [value]) => {
                if self.surface.spec_handle(key).is_none() || !self.surface.kind(key).is_checkbox()
                {
                    return Err("expect-checked requires a checkbox".into());
                }
                expect(self.surface.check_state(key), *value)
            }
            ("press-enter", []) => {
                let focused = self
                    .surface
                    .spec_handle(key)
                    .ok_or("locator is not a native control")?;
                self.enter(focused, None)?;
                Ok(())
            }
            ("expect-disabled", [value]) => {
                expect(self.surface.spec_disabled(key)?, number(value)?)
            }
            ("expect-selection", [wanted]) => expect(self.surface.spec_selection(key)?, *wanted),
            ("set-date" | "set-time" | "expect-date" | "expect-time", [value]) => {
                let mode = if step.command.ends_with("date") {
                    CivilKind::Date
                } else {
                    CivilKind::Time
                };
                let change = step.command.starts_with("set-");
                let actual = self
                    .surface
                    .spec_civil(key, mode, change.then_some(*value))?;
                if change {
                    self.civil_change(key, actual)
                } else {
                    expect(actual, CivilValue::parse(mode, value)?)
                }
            }
            ("click" | "fill" | "toggle" | "select", _) => {
                if self.surface.spec_disabled(key)? {
                    return Err("control is disabled".into());
                }
                let (handle, notification) = match (step.command, args) {
                    ("click", []) => {
                        let handle = self
                            .surface
                            .spec_handle(key)
                            .ok_or("locator is not a native control")?;
                        if self.surface.kind(key) != Control::Button {
                            return Err("click requires a push button".into());
                        }
                        (handle, BN_CLICKED)
                    }
                    ("fill", [value]) => (self.surface.spec_input(key, value)?, EN_CHANGE),
                    ("select", [value]) => self.surface.spec_choice(key, value)?,
                    ("toggle", []) => (self.surface.spec_toggle(key)?, BN_CLICKED),
                    _ => return Err("invalid control arguments".into()),
                };
                if notification == TCN_SELCHANGE {
                    self.notify(handle, notification)
                } else {
                    self.command(handle, notification as u16)
                }
            }
            (
                "select-row"
                | "activate-row"
                | "press-column"
                | "expect-row-selected"
                | "expect-list-rows",
                _,
            ) => self.report_step(key, step.command, args),
            (
                "focus-cell"
                | "begin-cell"
                | "fill-cell"
                | "commit-cell"
                | "cancel-cell"
                | "expect-cell-focused"
                | "expect-cell-text"
                | "expect-cell-feedback"
                | "select-cell-range"
                | "paste-grid",
                _,
            ) => self.grid_step(key, step.command, args),
            _ => Err("invalid control arguments".into()),
        }
    }

    fn grid_step(&mut self, key: Key, command: &str, args: &[&str]) -> Result<()> {
        let (root, font, dpi) = self.surface.grid_coordinates();
        let (hwnd, grid) = self
            .surface
            .grid_mut(key)
            .ok_or("step requires a data grid")?;
        let cell = |grid: &crate::grid::Grid, row: &str, column: &str| {
            let index = number::<usize>(row)?
                .checked_sub(1)
                .ok_or("grid rows start at one")?;
            grid.location(index, column)
        };
        match (command, args) {
            ("expect-cell-focused", [row, column]) => {
                let (r, c) = cell(grid, row, column)?;
                expect(grid.active(r, c), true)
            }
            ("expect-cell-text", [row, column, value]) => {
                let (r, c) = cell(grid, row, column)?;
                expect(grid.native_text(hwnd, r, c).as_str(), *value)
            }
            ("expect-cell-feedback", [row, column, value]) => {
                let (r, c) = cell(grid, row, column)?;
                expect(grid.error(r, c), *value)
            }
            ("focus-cell" | "begin-cell", [row, column]) => {
                let (r, c) = cell(grid, row, column)?;
                grid.activate(hwnd, r, c)?;
                if command == "begin-cell" {
                    grid.begin(hwnd, root, font)?;
                }
                grid.arrange(hwnd, root, font, dpi).map(|_| ())
            }
            ("select-cell-range", [first_row, first_column, last_row, last_column]) => {
                let first = cell(grid, first_row, first_column)?;
                let last = cell(grid, last_row, last_column)?;
                grid.select_range(hwnd, first, last)?;
                grid.arrange(hwnd, root, font, dpi).map(|_| ())
            }
            ("paste-grid", [text]) => {
                let clipboard = crate::grid_clipboard::ClipboardText::from_text(text);
                let intent = grid
                    .paste(key, clipboard)
                    .ok_or("the grid does not accept paste")?;
                self.deliver_grid(intent)
            }
            ("fill-cell", [text]) => grid.fill(text),
            ("commit-cell", []) => {
                if let Some(intent) = grid.commit(key, hwnd) {
                    self.deliver_grid(intent)?;
                }
                Ok(())
            }
            ("cancel-cell", []) => {
                grid.cancel(hwnd, true);
                Ok(())
            }
            _ => Err("invalid grid arguments".into()),
        }
    }

    fn report_step(&mut self, key: Key, command: &str, args: &[&str]) -> Result<()> {
        let (hwnd, report) = self
            .surface
            .report(key)
            .ok_or("step requires a list view")?;
        match (command, args) {
            ("expect-row-selected", [expected]) => {
                return expect(report.selection(hwnd).unwrap_or(""), *expected);
            }
            ("expect-list-rows", rows) => return expect(report.row_keys(hwnd)?.as_slice(), rows),
            _ => {}
        }
        if self.surface.spec_disabled(key)? {
            return Err("control is disabled".into());
        }
        match (command, args) {
            ("select-row", [row]) => {
                report.select(hwnd, (!row.is_empty()).then_some(*row))?;
                self.report_selected(key)
            }
            ("activate-row", [row]) if report.has_row(row) => {
                self.report_event(key, "activate", (*row).into())
            }
            ("press-column", [column]) if report.has_column(column) => {
                self.report_event(key, "column-press", (*column).into())
            }
            ("activate-row", [row]) => Err(format!("unknown report row {row:?}")),
            ("press-column", [column]) => Err(format!("unknown report column {column:?}")),
            _ => Err("invalid report arguments".into()),
        }
    }

    fn one_control(&self, selectors: &[(&str, &str)]) -> Result<Key> {
        let controls = self.controls(selectors)?;
        match controls.as_slice() {
            [key] => Ok(*key),
            [] => {
                if let [(kind, label)] = selectors
                    && *kind != "test-id"
                    && !label.is_empty()
                {
                    let seen: Vec<_> = self
                        .controls(&[(kind, "")])?
                        .into_iter()
                        .filter_map(|key| self.surface.spec_label(key, kind))
                        .take(12)
                        .collect();
                    return Err(format!(
                        "no control matches {selectors:?}, observed {seen:?}"
                    ));
                }
                Err(format!("no control matches {selectors:?}"))
            }
            _ => Err(format!("{} controls match {selectors:?}", controls.len())),
        }
    }

    fn controls(&self, selectors: &[(&str, &str)]) -> Result<Vec<Key>> {
        let mut origin = ROOT;
        let mut found = Vec::new();
        for (index, &(kind, label)) in selectors.iter().enumerate() {
            let mut walk = self.tree.node(origin).children.clone();
            found.clear();
            while let Some(key) = walk.pop() {
                let node = self.tree.node(key);
                walk.extend(&node.children);
                let matches = if kind == "test-id" {
                    !label.is_empty() && node.attr("test-id") == Some(label)
                } else {
                    let expected = if kind == "image" { "asset-image" } else { kind };
                    matches!(&node.kind, Kind::Element(tag) if tag.strip_prefix("ui-") == Some(expected)
                        || (expected == "checkbox" && tag == "ui-aggregate-checkbox")
                        || (expected == "text-area" && tag == "textarea"))
                        && (label.is_empty()
                            || self.surface.spec_label(key, kind).as_deref() == Some(label))
                };
                if matches {
                    found.push(key);
                }
            }
            if index + 1 < selectors.len() {
                let [parent] = found.as_slice() else {
                    return Err(format!(
                        "within scope ({kind} {label:?}) matches {} controls",
                        found.len()
                    ));
                };
                origin = *parent;
            }
        }
        Ok(found)
    }
}

fn fixture_path(name: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(name);
    if name.is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err("fixtures must be relative to the app directory".into());
    }
    std::env::current_dir()
        .map(|root| root.join(path))
        .map_err(|error| format!("cannot resolve fixture directory: {error}"))
}

fn number<T: FromStr>(value: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| format!("invalid value {value:?}"))
}

fn expect<T: PartialEq + Debug>(actual: T, expected: T) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {expected:?}, observed {actual:?}"))
    }
}
