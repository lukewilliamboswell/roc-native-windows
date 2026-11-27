//! Execute a checked spec on the app's own UI thread.

use super::{APP, App, FAILURE, SpecSurface, last_error, mount};
use crate::Result;
use crate::capture::{self, Recorder};
use crate::civil::{Kind as CivilKind, Value as CivilValue};
use crate::effects::SpecOptions;
use crate::progress::Progress;
use crate::spec_plan::{Locator, MAX_PLAN_BYTES, Mode, PROTOCOL, Plan, Step, valid_file_name};
use crate::surface_input::Phase;
use crate::view::Control;
use crate::windows::OwnedWindow;
use crate::wire::{Key, Kind};
use std::collections::BTreeMap;
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
const EXIT_LIMIT: Duration = Duration::from_secs(5);

pub(super) fn run(window: OwnedWindow, plan_path: &Path) -> Result<()> {
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
    size_window(window.0, size)?;
    mount(window, Some(options))?;
    let visible = plan.mode == Mode::Demo
        || failure_picture
        || plan.steps.iter().any(|step| step.command == "screenshot");
    let mut windows = Windows::new(size, visible);
    windows.refresh()?;

    let started = Instant::now();
    if let Some(window) = windows.recording_window() {
        capture.checkpoint(window, started, 0)?;
    }
    let mut steps = plan
        .steps
        .iter()
        .filter(|step| !step.command.starts_with("setup-"))
        .peekable();
    let mut index = 0;
    while let Some(step) = steps.next() {
        index += 1;
        // The close request in the preceding step can post WM_QUIT before
        // the explicit terminal assertion runs.
        windows.expect_exit = step.command == "expect-exit"
            || steps
                .peek()
                .is_some_and(|next| next.command == "expect-exit");
        let result = perform(&mut windows, step, &mut capture, started)
            .and_then(|()| pump(&mut windows, &mut capture, started))
            .and_then(|()| match windows.recording_window() {
                Some(window) => capture.checkpoint(window, started, index),
                None => Ok(()),
            });
        if let Err(problem) = result {
            if failure_picture && let Some(window) = windows.failure_window(step) {
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
        live_entropy: demo,
        regional: crate::spec_regional::Regional::default(),
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
                options.live_entropy = true;
            }
            ("setup-live-effects", ["time"]) => options.live_time = true,
            ("setup-live-effects", ["http"]) => options.live_http = true,
            ("setup-live-effects", ["entropy"]) => options.live_entropy = true,
            ("setup-regional-fixture", [path]) => {
                let bytes = read_regional_fixture(path).map_err(parse)?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| step.problem("expected a UTF-8 regional fixture"))?;
                options.regional = crate::spec_regional::Regional::read(text).map_err(parse)?;
            }
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

struct Windows {
    size: (i32, i32),
    visible: bool,
    mounted: BTreeMap<Key, HWND>,
    first: Option<(Key, HWND)>,
    default: Option<HWND>,
    expect_exit: bool,
    exited: bool,
}

impl Windows {
    fn new(size: (i32, i32), visible: bool) -> Self {
        Self {
            size,
            visible,
            mounted: BTreeMap::new(),
            first: None,
            default: None,
            expect_exit: false,
            exited: false,
        }
    }

    fn refresh(&mut self) -> Result<()> {
        let surfaces = with_app(|app| Ok(app.spec_surfaces()))?;
        let first = surfaces.iter().find(|surface| !surface.is_dialog);
        self.default = first.map(|surface| surface.handle);
        if self.first.is_none() {
            self.first = first.map(|surface| (surface.id, surface.handle));
        }

        let mut mounted = BTreeMap::new();
        for surface in surfaces {
            let handle = surface.handle;
            if self.mounted.get(&surface.id) != Some(&handle) {
                if !surface.is_dialog {
                    size_window(handle, self.size)?;
                }
                if self.visible {
                    // Showing a window can synchronously send its window
                    // procedure a message. No App borrow is held here.
                    unsafe {
                        ShowWindow(handle, SW_SHOWNOACTIVATE);
                        UpdateWindow(handle);
                    }
                }
            }
            mounted.insert(surface.id, handle);
        }
        self.mounted = mounted;
        Ok(())
    }

    fn recording_window(&self) -> Option<HWND> {
        let (id, handle) = self.first?;
        // Automatic recordings keep their original window and frame size.
        (!self.exited && self.mounted.get(&id) == Some(&handle)).then_some(handle)
    }

    fn failure_window(&self, step: &Step<'_>) -> Option<HWND> {
        if self.exited {
            return None;
        }
        let scoped = Locator::read(&step.args).ok().and_then(|(locator, _)| {
            if !locator.names_surface() {
                return None;
            }
            with_app(|app| app.one_surface(&locator).map(|surface| surface.handle)).ok()
        });
        scoped.or(self.default).filter(|&window| {
            // SAFETY: IsWindow accepts a possibly destroyed HWND.
            (unsafe { IsWindow(window) }) != 0
        })
    }

    fn messages(&mut self) -> Result<()> {
        let mut message = MSG::default();
        // SAFETY: We are on the UI thread with an initialized message record.
        unsafe {
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    if let Some(error) = FAILURE.with(|slot| slot.borrow_mut().take()) {
                        return Err(error);
                    }
                    if message.wParam != 0 {
                        return Err(format!("application exited with status {}", message.wParam));
                    }
                    if !self.expect_exit || !with_app(|app| Ok(app.exited()))? {
                        return Err("spec window closed before completion".into());
                    }
                    self.exited = true;
                    return Ok(());
                }
                if !(WM_KEYFIRST..=WM_KEYLAST).contains(&message.message) {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        Ok(())
    }
}

fn pump(windows: &mut Windows, capture: &mut Capture, started: Instant) -> Result<()> {
    if windows.exited {
        return Ok(());
    }
    windows.messages()?;
    if !windows.exited {
        with_app(App::poll)?;
        windows.messages()?;
    }
    if !windows.exited {
        windows.refresh()?;
        if let Some(window) = windows.recording_window() {
            capture.sample(window, started)?;
        }
    }
    Ok(())
}

fn perform(
    windows: &mut Windows,
    step: &Step<'_>,
    capture: &mut Capture,
    started: Instant,
) -> Result<()> {
    match (step.command, step.args.as_slice()) {
        ("wait", [value]) => {
            let duration = Duration::from_millis(number(value)?);
            if duration > capture::MAX_RECORDING_DURATION {
                return Err("wait exceeds 60 seconds".into());
            }
            let begin = Instant::now();
            while begin.elapsed() < duration && !windows.exited {
                pump(windows, capture, started)?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
        ("await-work", []) => {
            let begin = Instant::now();
            loop {
                pump(windows, capture, started)?;
                if windows.exited {
                    return Ok(());
                }
                if with_app(|app| Ok(app.work_done()))? {
                    // The worker publishes its answer before lowering running;
                    // this drain catches racing completions and follow-up work.
                    pump(windows, capture, started)?;
                    if windows.exited || with_app(|app| Ok(app.work_done()))? {
                        return Ok(());
                    }
                }
                if begin.elapsed() >= WORK_LIMIT {
                    return Err("work did not finish within 30 seconds".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        ("expect-exit", []) => {
            let begin = Instant::now();
            while !windows.exited {
                pump(windows, capture, started)?;
                if windows.exited {
                    break;
                }
                if begin.elapsed() >= EXIT_LIMIT {
                    return Err("the application did not exit within five seconds".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
        ("screenshot", [stem]) => {
            capture.screenshot(windows.default.ok_or("no live window to capture")?, stem)
        }
        ("screenshot", _) => {
            let (locator, args) = Locator::read(&step.args)?;
            let [stem] = args else {
                return Err("screenshot needs a surface and filename stem".into());
            };
            let surface = with_app(|app| app.one_surface(&locator))?;
            capture.screenshot(surface.handle, stem)
        }
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
            ("drop-files-outside", _) => {
                let surface = self.default_spec_surface()?;
                self.prepare_surface(&surface, true)?;
                self.spec_drop(None, &step.args)
            }
            ("close-window" | "dismiss", _) => {
                let (locator, args) = Locator::read(&step.args)?;
                if !args.is_empty() {
                    return Err("close gestures take one surface locator".into());
                }
                let surface = self.one_surface(&locator)?;
                if surface.is_dialog != (step.command == "dismiss") {
                    return Err("close-window needs a window and dismiss needs a dialog".into());
                }
                self.prepare_surface(&surface, true)?;
                self.request_close(surface.id)
            }
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
            ("set-wall-time", [value]) => {
                self.effects.simulated.now = number(value)?;
                Ok(())
            }
            ("random-bytes", [count, path]) => {
                let bytes = read_regional_fixture(path)?;
                let answer = self
                    .effects
                    .simulated
                    .answer_random(number(count)?, Ok(bytes))?;
                self.effects.send_simulated(answer);
                self.poll()
            }
            ("fail-random", [count, reason]) => {
                let code = match *reason {
                    "unavailable" => 1,
                    "refused" => 5,
                    "failed" => 6,
                    _ => return Err("unknown random refusal".into()),
                };
                let answer = self
                    .effects
                    .simulated
                    .answer_random(number(count)?, Err(code))?;
                self.effects.send_simulated(answer);
                self.poll()
            }
            ("expect-random-cancelled", [count]) => expect(
                self.effects.simulated.random_cancelled(number(count)?)?,
                true,
            ),
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
            ("expect-taskbar-progress", [wanted]) => {
                let surface = self.default_spec_surface()?;
                self.prepare_surface(&surface, false)?;
                expect(self.surface.taskbar_progress(), Progress::parse(wanted)?)
            }
            ("expect-status", panes) => {
                let surface = self.default_spec_surface()?;
                self.prepare_surface(&surface, false)?;
                let texts = self.surface.status_texts();
                expect(
                    texts
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .as_slice(),
                    panes,
                )
            }
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
                "pointer-down" | "pointer-move" | "pointer-up" | "pointer-cancel" | "key-press"
                | "set-slider" | "expect-slider",
                _,
            ) => self.control_step(step),
            (
                "click" | "fill" | "toggle" | "press-enter" | "expect-visible" | "expect-absent"
                | "expect-text" | "expect-checked" | "expect-disabled" | "select"
                | "expect-selection" | "set-date" | "expect-date" | "set-time" | "expect-time"
                | "press-escape" | "expect-invalid" | "expect-progress" | "expand" | "collapse"
                | "expect-expanded" | "drop-files",
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
            ("expect-shader" | "expect-shader-pixel", _) => self.control_step(step),
            _ => Err("unknown step or invalid arguments".into()),
        }
    }

    fn run_simulated(&mut self, number: usize) -> Result<()> {
        let response = self.effects.simulated.run(number)?;
        self.effects.send_simulated(response);
        self.poll()
    }

    fn spec_files(&mut self, args: &[&str]) -> Result<()> {
        let key = self.spec_picker("file-button")?;
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

    fn spec_drop(&mut self, target: Option<Key>, args: &[&str]) -> Result<()> {
        let point = self.surface.spec_drop_point(&self.tree, target)?;
        let files = args
            .iter()
            .map(|name| fixture_path(name))
            .collect::<Result<Vec<_>>>()?;
        self.dropped(Some(point), files)?;
        self.poll()
    }

    fn spec_folder(&mut self, fixture: &str) -> Result<()> {
        let key = self.spec_picker("folder-button")?;
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
        let (locator, args) = Locator::read(&step.args)?;
        if locator.controls.is_empty() {
            if step.command == "expect-absent" && args.is_empty() {
                return expect(self.located_surfaces(&locator)?.len(), 0);
            }
            let surface = self.one_surface(&locator)?;
            return match (step.command, args) {
                ("expect-visible", []) => Ok(()),
                ("expect-disabled", [value]) => expect(surface.blocked, number(value)?),
                _ => Err("this step needs a control within its window or dialog scope".into()),
            };
        }
        if step.command == "expect-absent" && args.is_empty() {
            return expect(self.controls(&locator)?.len(), 0);
        }
        let (surface, key) = self.one_control(&locator)?;
        self.prepare_surface(&surface, !step.command.starts_with("expect-"))?;
        match (step.command, args) {
            ("pointer-down" | "pointer-move" | "pointer-up" | "pointer-cancel", _) => {
                let phase = match step.command {
                    "pointer-down" => Phase::Down,
                    "pointer-move" => Phase::Move,
                    "pointer-up" => Phase::Up,
                    _ => Phase::Cancel,
                };
                if phase == Phase::Cancel && args.is_empty() {
                    return self.spec_pointer(key, phase, None);
                }
                if args.is_empty() || phase == Phase::Cancel {
                    return Err("this pointer gesture needs position arguments".into());
                }
                let mut remaining = args;
                while !remaining.is_empty() {
                    let position = match remaining {
                        ["outside", rest @ ..] => {
                            remaining = rest;
                            None
                        }
                        [x, y, rest @ ..] => {
                            remaining = rest;
                            let (x, y): (u16, u16) = (number(x)?, number(y)?);
                            if x > 10_000 || y > 10_000 {
                                return Err("pointer positions must be in 0–10000".into());
                            }
                            Some((x, y))
                        }
                        _ => return Err("a pointer position needs x and y or outside".into()),
                    };
                    if phase == Phase::Down && position.is_none() {
                        return Err("pointer-down needs an in-zone position".into());
                    }
                    if phase != Phase::Move && !remaining.is_empty() {
                        return Err("this pointer gesture takes one position".into());
                    }
                    self.spec_pointer(key, phase, position)?;
                }
                Ok(())
            }
            ("key-press", [name]) => self.spec_key(key, name),
            ("expect-slider", [value]) => {
                let (_, actual) = self.surface.spec_slider(key, None)?;
                expect(actual, number(value)?)
            }
            ("set-slider", [value]) => {
                let (handle, _) = self.surface.spec_slider(key, Some(number(value)?))?;
                self.slider_changed(handle)
            }
            ("expect-shader", [status]) => expect(self.surface.shader_status(key)?, *status),
            ("expect-shader-pixel", [x, y, hex, tolerance]) => {
                let pixel = self.surface.shader_pixel(key, number(x)?, number(y)?)?;
                let hex = hex.strip_prefix('#').unwrap_or(hex);
                if hex.len() != 6 || !hex.bytes().all(|value| value.is_ascii_hexdigit()) {
                    return Err("shader color must be six RGB hex digits".into());
                }
                let wanted = [0, 2, 4].map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap());
                let tolerance: u8 = number(tolerance)?;
                if pixel
                    .iter()
                    .zip(wanted)
                    .any(|(&actual, wanted)| actual.abs_diff(wanted) > tolerance)
                {
                    return Err(format!(
                        "expected shader pixel {wanted:?} ± {tolerance}, got {pixel:?}"
                    ));
                }
                Ok(())
            }
            ("expect-visible", []) => Ok(()),
            ("drop-files", files) => self.spec_drop(Some(key), files),
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
                if !self.commit_civil(focused)? {
                    self.enter(focused, None)?;
                }
                Ok(())
            }
            ("expect-disabled", [value]) => {
                expect(self.surface.spec_disabled(key)?, number(value)?)
            }
            ("expect-progress", [wanted]) => self.surface.spec_progress(key, wanted),
            ("expect-selection", [wanted]) => expect(self.surface.spec_selection(key)?, *wanted),
            ("press-escape", []) => {
                let handle = self
                    .surface
                    .spec_handle(key)
                    .ok_or("locator is not a native control")?;
                self.cancel_civil(handle)?;
                Ok(())
            }
            ("expect-invalid", [wanted]) => {
                expect(self.surface.spec_civil_invalid(key)?, number(wanted)?)
            }
            ("set-date" | "set-time" | "expect-date" | "expect-time", [value]) => {
                let mode = if step.command.ends_with("date") {
                    CivilKind::Date
                } else {
                    CivilKind::Time
                };
                let change = step.command.starts_with("set-");
                let (handle, actual) =
                    self.surface
                        .spec_civil(key, mode, change.then_some(*value))?;
                if change {
                    self.commit_civil(handle)?;
                    Ok(())
                } else {
                    expect(actual, CivilValue::parse(mode, value)?)
                }
            }
            ("expect-expanded", [item, value]) => {
                expect(self.surface.spec_tree_expanded(key, item)?, number(value)?)
            }
            ("expand" | "collapse", [item]) => {
                if self.surface.spec_disabled(key)? {
                    return Err("control is disabled".into());
                }
                let intent = self
                    .surface
                    .spec_tree_expand(key, item, step.command == "expand")?;
                intent.map_or(Ok(()), |intent| self.tree_event(key, intent))
            }
            ("select", [value])
                if self.surface.spec_handle(key).is_some()
                    && self.surface.kind(key) == Control::TreeView =>
            {
                if self.surface.spec_disabled(key)? {
                    return Err("control is disabled".into());
                }
                let intent = self.surface.spec_tree_select(key, value)?;
                intent.map_or(Ok(()), |intent| self.tree_event(key, intent))
            }
            ("click" | "fill" | "toggle" | "select", _) => {
                if self.surface.spec_disabled(key)? {
                    return Err("control is disabled".into());
                }
                let (handle, notification) = match (step.command, args) {
                    ("click", []) => {
                        if let Some((toolbar, id)) = self.surface.toolbar_command(key) {
                            return self.toolbar_command(toolbar, id);
                        }
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
        let (root, font) = self.surface.grid_coordinates();
        let (hwnd, grid) = self
            .surface
            .grid_mut(key)
            .ok_or("step requires a data grid")?;
        let cell = |grid: &dyn crate::grid::MountedGrid, row: &str, column: &str| {
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
                self.surface.arrange(&self.tree)
            }
            ("select-cell-range", [first_row, first_column, last_row, last_column]) => {
                let first = cell(grid, first_row, first_column)?;
                let last = cell(grid, last_row, last_column)?;
                grid.select_range(hwnd, first, last)?;
                self.surface.arrange(&self.tree)
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

    fn default_spec_surface(&self) -> Result<SpecSurface> {
        self.spec_surfaces()
            .into_iter()
            .find(|surface| !surface.is_dialog)
            .ok_or_else(|| "no live application window".into())
    }

    fn prepare_surface(&mut self, surface: &SpecSurface, action: bool) -> Result<()> {
        if action && surface.blocked {
            return Err(format!(
                "window {:?} is blocked by its modal dialog",
                surface.title
            ));
        }
        self.select_surface(surface.id)
    }

    fn spec_picker(&mut self, kind: &str) -> Result<Key> {
        let locator = Locator {
            controls: vec![(kind, "")],
            ..Locator::default()
        };
        let (surface, key) = self.one_control(&locator)?;
        self.prepare_surface(&surface, true)?;
        Ok(key)
    }

    fn located_surfaces(&self, locator: &Locator<'_>) -> Result<Vec<SpecSurface>> {
        let surfaces = self.spec_surfaces();
        if let Some(title) = locator.window {
            let mut windows = surfaces
                .iter()
                .filter(|surface| !surface.is_dialog && surface.title == title);
            if let Some(dialog_title) = locator.dialog {
                let window = windows.next().map(|surface| surface.id);
                if windows.next().is_some() {
                    return Err(format!("more than one window matches {title:?}"));
                }
                return Ok(surfaces
                    .into_iter()
                    .filter(|surface| {
                        window.is_some()
                            && surface.is_dialog
                            && surface.title == dialog_title
                            && surface.owner == window
                    })
                    .collect());
            }
            return Ok(surfaces
                .into_iter()
                .filter(|surface| !surface.is_dialog && surface.title == title)
                .collect());
        }
        Ok(surfaces
            .into_iter()
            .filter(|surface| {
                locator
                    .dialog
                    .is_none_or(|title| surface.is_dialog && surface.title == title)
            })
            .collect())
    }

    fn one_surface(&self, locator: &Locator<'_>) -> Result<SpecSurface> {
        if !locator.names_surface() || !locator.controls.is_empty() {
            return Err("expected a window or dialog locator".into());
        }
        let mut found = self.located_surfaces(locator)?.into_iter();
        let result = found.next().ok_or_else(|| {
            format!(
                "no live window or dialog matches {:?}",
                locator.dialog.or(locator.window)
            )
        })?;
        if found.next().is_some() {
            return Err(format!(
                "more than one window or dialog matches {:?}",
                locator.dialog.or(locator.window)
            ));
        }
        Ok(result)
    }

    fn one_control(&mut self, locator: &Locator<'_>) -> Result<(SpecSurface, Key)> {
        let matches = self.controls(locator)?;
        if let [(id, key)] = matches.as_slice() {
            let surface = self
                .spec_surfaces()
                .into_iter()
                .find(|surface| surface.id == *id)
                .ok_or("the selected surface is no longer mounted")?;
            return Ok((surface, *key));
        }
        let selectors = &locator.controls;
        if matches.is_empty() {
            if let [(kind, label)] = selectors.as_slice()
                && *kind != "test-id"
                && !label.is_empty()
            {
                let wild = Locator {
                    controls: vec![(*kind, "")],
                    ..locator.clone()
                };
                let mut seen = Vec::new();
                for (surface, key) in self.controls(&wild)?.into_iter().take(12) {
                    self.select_surface(surface)?;
                    seen.extend(self.surface.spec_label(key, kind));
                }
                return Err(format!(
                    "no control matches {selectors:?}, observed {seen:?}"
                ));
            }
            Err(format!("no control matches {selectors:?}"))
        } else {
            Err(format!(
                "{} controls match {selectors:?}; scope the locator to one window or dialog",
                matches.len()
            ))
        }
    }

    fn controls(&mut self, locator: &Locator<'_>) -> Result<Vec<(Key, Key)>> {
        let mut result = Vec::new();
        for surface in self.located_surfaces(locator)? {
            self.select_surface(surface.id)?;
            for key in self.controls_in(surface.root, &locator.controls)? {
                result.push((surface.id, key));
            }
        }
        Ok(result)
    }

    fn controls_in(&self, root: Key, selectors: &[(&str, &str)]) -> Result<Vec<Key>> {
        let mut origin = root;
        let mut found = Vec::new();
        for (index, &(kind, label)) in selectors.iter().enumerate() {
            let mut walk = if index == 0 {
                vec![origin]
            } else {
                self.tree.node(origin).children.clone()
            };
            found.clear();
            while let Some(key) = walk.pop() {
                let node = self.tree.node(key);
                if key != root
                    && matches!(&node.kind, Kind::Element(tag) if matches!(tag.as_str(), "ui-window" | "ui-windows" | "ui-dialog"))
                {
                    continue;
                }
                walk.extend(&node.children);
                let matches = if kind == "test-id" {
                    !label.is_empty() && node.attr("test-id") == Some(label)
                } else {
                    let expected = match kind {
                        "image" => "asset-image",
                        "date-field" => "date-picker",
                        "time-field" => "time-picker",
                        _ => kind,
                    };
                    matches!(&node.kind, Kind::Element(tag) if tag.strip_prefix("ui-") == Some(expected)
                        || (kind == "image" && tag == "ui-image")
                        || (kind == "text" && tag == "ui-status-text")
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
                match found.as_slice() {
                    [parent] => origin = *parent,
                    [] => return Ok(Vec::new()),
                    _ => {
                        return Err(format!(
                            "within scope ({kind} {label:?}) matches {} controls",
                            found.len()
                        ));
                    }
                }
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

fn read_regional_fixture(path: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    const MAX_BYTES: u64 = 65_536;
    let file = std::fs::File::open(fixture_path(path)?).map_err(|error| error.to_string())?;
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() as u64 > MAX_BYTES {
        return Err("regional or random fixture exceeds 65,536 bytes".into());
    }
    Ok(data)
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
