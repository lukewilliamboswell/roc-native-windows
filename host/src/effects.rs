//! Operating-system effects requested by the engine. Only the UI thread delivers turns.

mod files;
mod folders;
mod http;
mod http_policy;
mod images;
mod pickers;
mod storage;
mod system;
mod workers;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::Foundation::HWND;

use crate::clipboard;
use crate::regional::{self, Snapshot};
use crate::spec_effects::{self, Simulated};
use crate::spec_plan::valid_file_name;
use crate::wire::{self, Declarations, Reader};
use crate::{Event, Result};

pub(crate) use files::{register_handle_box, released_box};

const EFFECT_NODE: u32 = u32::MAX;
const MAX_TIMEOUT: u32 = 600_000;
// The pinned environment adapter maps any non-environment result to Refused.
// Timeout is the supported empty wire result for an unsuccessful refresh.
const REFRESH_FAILED: u8 = 7;
type Completion = (u32, Arc<AtomicBool>, Outcome);
type Job = Box<dyn FnOnce(&AtomicBool, &dyn Fn(Vec<u8>)) -> Outcome + Send>;

struct WorkOptions {
    timeout: u32,
    timeout_tag: u8,
    finish_on_cancel: bool,
    unavailable: Vec<u8>,
}

struct Queued {
    id: u32,
    cancelled: Arc<AtomicBool>,
    options: WorkOptions,
    run: Job,
}

enum Outcome {
    Bytes(Vec<u8>),
    Progress(Vec<u8>),
    Raster(Arc<crate::raster::Pixels>),
    Work {
        output: Vec<u8>,
        files: Vec<files::Entry>,
    },
    Folder {
        flags: u8,
        files: Vec<files::Entry>,
    },
}

struct Timer {
    next: Instant,
    interval: Duration,
    recurring: bool,
}

struct Flight {
    cancelled: Arc<AtomicBool>,
    deadline: Option<(Instant, u8)>,
    finish_on_cancel: bool,
    delivers: bool,
}

pub(crate) struct SpecOptions {
    pub(crate) storage: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) local: Vec<(String, String)>,
    pub(crate) session: Vec<(String, String)>,
    pub(crate) initial_time: i64,
    pub(crate) live_time: bool,
    pub(crate) live_http: bool,
    pub(crate) live_entropy: bool,
    pub(crate) regional: crate::spec_regional::Regional,
    pub(crate) worker_limit: usize,
}

pub(crate) struct Effects {
    started: Instant,
    declarations: Declarations,
    http_policy: http_policy::Policy,
    send: mpsc::Sender<Event>,
    complete: mpsc::Sender<Completion>,
    completed: mpsc::Receiver<Completion>,
    flights: HashMap<u32, Flight>,
    running: Arc<std::sync::atomic::AtomicUsize>,
    timers: HashMap<u32, Timer>,
    session: HashMap<String, String>,
    local: Option<storage::Local>,
    queued: VecDeque<Queued>,
    spec: Option<SpecOptions>,
    pub(crate) simulated: Simulated,
    pub(crate) clipboard_writes: usize,
    pub(crate) clipboard_text: String,
    pub(crate) clipboard_busy: bool,
}

impl Effects {
    pub(crate) fn new(send: mpsc::Sender<Event>, declarations: Declarations) -> Result<Self> {
        let (complete, completed) = mpsc::channel();
        Ok(Self {
            started: Instant::now(),
            declarations,
            http_policy: http_policy::Policy::load()?,
            send,
            complete,
            completed,
            flights: HashMap::new(),
            running: Arc::default(),
            timers: HashMap::new(),
            session: HashMap::new(),
            local: storage::Local::discover(),
            queued: VecDeque::new(),
            spec: None,
            simulated: Simulated::default(),
            clipboard_writes: 0,
            clipboard_text: String::new(),
            clipboard_busy: false,
        })
    }

    pub(crate) fn configure_spec(&mut self, mut options: SpecOptions) -> Result<()> {
        self.local = storage::Local::fixture(&options.storage);
        self.simulated.now = options.initial_time;
        for (key, value) in std::mem::take(&mut options.local) {
            self.storage(1, &key, Some(value))?;
        }
        for (key, value) in std::mem::take(&mut options.session) {
            self.storage(3, &key, Some(value))?;
        }
        self.spec = Some(options);
        Ok(())
    }

    pub(crate) fn work_done(&self) -> bool {
        self.queued.is_empty() && self.running.load(Ordering::Acquire) == 0
    }

    pub(crate) fn pending(&self) -> usize {
        self.simulated.pending()
            + self
                .timers
                .values()
                .filter(|timer| !timer.recurring)
                .count()
            + self
                .flights
                .values()
                .filter(|flight| flight.delivers)
                .count()
    }

    pub(crate) fn interval_count(&self, ms: u32) -> usize {
        self.simulated.timers(ms, true).len()
            + self
                .timers
                .values()
                .filter(|timer| timer.recurring && timer.interval == millis(ms))
                .count()
    }

    pub(crate) fn cancelled(&self, number: usize) -> Result<bool> {
        self.simulated.cancelled(number)
    }

    pub(crate) fn send_simulated(&self, (id, payload): (u32, Vec<u8>)) {
        self.answer(id, payload);
    }

    pub(crate) fn expire_simulated_http(&mut self) -> bool {
        if let Some(id) = self.simulated.next_timeout() {
            self.answer(id, vec![7]);
            true
        } else {
            false
        }
    }

    pub(crate) fn http_stub(
        &mut self,
        url: &str,
        status: u16,
        fields: &[&str],
        body: &str,
    ) -> Result<()> {
        let headers = fields
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (pair[0], pair[1]));
        self.simulated
            .stage(url, http::encode_response(status, headers, body.as_bytes()))
    }

    pub(crate) fn http_failure(&mut self, timeout: bool, detail: &str) -> Result<()> {
        self.simulated.stage(
            "",
            if timeout {
                vec![7]
            } else {
                http::failure(detail)
            },
        )
    }

    pub(crate) fn poll(&mut self) {
        let now = Instant::now();
        let mut answers = Vec::new();
        self.timers.retain(|&id, timer| {
            if now < timer.next {
                return true;
            }
            let payload = match regional::unix_ms(SystemTime::now()) {
                Ok(timestamp) => spec_effects::tick(timestamp),
                Err(_) => vec![REFRESH_FAILED],
            };
            answers.push((id, payload));
            timer.next = now + timer.interval;
            timer.recurring
        });
        self.flights.retain(|&id, flight| {
            if let Some((deadline, tag)) = flight.deadline
                && now >= deadline
            {
                flight.cancelled.store(true, Ordering::Relaxed);
                if flight.delivers {
                    answers.push((id, vec![tag]));
                }
                return false;
            }
            true
        });
        files::collect_released();
        for (id, token, outcome) in self.completed.try_iter() {
            let Some(flight) = self
                .flights
                .get(&id)
                .filter(|flight| Arc::ptr_eq(&flight.cancelled, &token))
            else {
                continue;
            };
            let delivers = flight.delivers;
            if !matches!(outcome, Outcome::Progress(_)) {
                self.flights.remove(&id);
            }
            if delivers {
                match outcome {
                    Outcome::Bytes(payload) | Outcome::Progress(payload) => {
                        answers.push((id, payload))
                    }
                    Outcome::Work { output, files } => {
                        let mut record = vec![10];
                        record.extend((files.len() as u32).to_le_bytes());
                        self.file_answer(id, record, files, &output, vec![11, 2]);
                    }
                    Outcome::Raster(pixels) => {
                        self.file_answer(
                            id,
                            images::answer(0, true),
                            vec![files::Entry::image(pixels)],
                            &[],
                            images::answer(5, false),
                        );
                    }
                    Outcome::Folder { flags, files } => {
                        self.file_answer(
                            id,
                            folders::answer_prefix(files.len(), flags),
                            files,
                            &[],
                            vec![15, 2],
                        );
                    }
                }
            }
        }
        for (id, payload) in answers {
            self.answer(id, payload);
        }
        self.start_queued();
    }

    fn file_answer(
        &self,
        id: u32,
        mut record: Vec<u8>,
        entries: Vec<files::Entry>,
        suffix: &[u8],
        unavailable: Vec<u8>,
    ) {
        let Some(handles) = files::mint_entries(entries, &mut record) else {
            self.answer(id, unavailable);
            return;
        };
        record.extend_from_slice(suffix);
        if handles.is_empty() {
            self.answer(id, record);
        } else {
            let _ = self.send_files(EFFECT_NODE, id, record, handles);
        }
    }

    fn answer(&self, id: u32, payload: Vec<u8>) {
        let _ = self.send.send(Event::Turn {
            node: EFFECT_NODE,
            handler: id,
            payload,
        });
    }

    fn spawn(
        &mut self,
        id: u32,
        options: WorkOptions,
        run: impl FnOnce(&AtomicBool, &dyn Fn(Vec<u8>)) -> Outcome + Send + 'static,
    ) {
        // A cancelled job still occupies its thread until it returns.
        if (self.spec.is_none() && self.running.load(Ordering::Acquire) >= 32)
            || self.queued.len() >= 1024
        {
            self.answer(id, options.unavailable);
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = (options.timeout != 0).then(|| {
            (
                Instant::now() + millis(options.timeout.min(MAX_TIMEOUT)),
                options.timeout_tag,
            )
        });
        self.flights.insert(
            id,
            Flight {
                cancelled: cancelled.clone(),
                deadline,
                finish_on_cancel: options.finish_on_cancel,
                delivers: true,
            },
        );
        self.queued.push_back(Queued {
            id,
            cancelled,
            options,
            run: Box::new(run),
        });
        self.start_queued();
    }

    fn start_queued(&mut self) {
        let limit = self.spec.as_ref().map_or(32, |spec| spec.worker_limit);
        while self.running.load(Ordering::Acquire) < limit {
            let Some(Queued {
                id,
                cancelled,
                options,
                run,
            }) = self.queued.pop_front()
            else {
                break;
            };
            if cancelled.load(Ordering::Relaxed) {
                continue;
            }
            self.running.fetch_add(1, Ordering::Relaxed);
            let running = Arc::clone(&self.running);
            let token = Arc::clone(&cancelled);
            let sender = self.complete.clone();
            if std::thread::Builder::new()
                .name("roc-effect".into())
                .stack_size(8 << 20)
                .spawn(move || {
                    if !token.load(Ordering::Relaxed) {
                        let outcome = run(&token, &|payload| {
                            if !token.load(Ordering::Relaxed) {
                                let _ = sender.send((
                                    id,
                                    Arc::clone(&token),
                                    Outcome::Progress(payload),
                                ));
                            }
                        });
                        let _ = sender.send((id, token, outcome));
                    }
                    running.fetch_sub(1, Ordering::Release);
                })
                .is_err()
            {
                self.running.fetch_sub(1, Ordering::Relaxed);
                if self
                    .flights
                    .remove(&id)
                    .is_some_and(|flight| flight.delivers)
                {
                    self.answer(id, options.unavailable);
                }
            }
        }
    }

    pub(crate) fn cancel(&mut self, id: u32) {
        self.simulated.cancel(id);
        self.timers.remove(&id);
        if let Some(flight) = self.flights.get_mut(&id) {
            if flight.finish_on_cancel {
                flight.delivers = false;
                flight
                    .deadline
                    .get_or_insert_with(|| (Instant::now() + millis(MAX_TIMEOUT), 7));
            } else {
                let flight = self.flights.remove(&id).unwrap();
                flight.cancelled.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn start(&mut self, id: u32, kind: u8, reader: &mut Reader<'_>) -> Result<()> {
        match kind {
            1 => {
                let request = http::Request::read(reader)?;
                let target = match request.target() {
                    Ok(target) => target,
                    Err(error) => {
                        self.answer(id, http::failure(&error.to_string()));
                        return Ok(());
                    }
                };
                if !self.http_policy.admits(&target) {
                    self.answer(id, http::policy_denied());
                    return Ok(());
                }
                if self.spec.as_ref().is_some_and(|spec| !spec.live_http) {
                    self.simulated
                        .hold(id, spec_effects::Kind::Http(request.spec_request()));
                } else {
                    let policy = self.http_policy.clone();
                    self.spawn(
                        id,
                        WorkOptions {
                            timeout: request.timeout,
                            timeout_tag: 7,
                            finish_on_cancel: false,
                            unavailable: http::failure("HTTP worker unavailable"),
                        },
                        move |stop, report| {
                            Outcome::Bytes(request.run(target, &policy, stop, report))
                        },
                    );
                }
            }
            2 | 17 => {
                let key = reader.string()?;
                let payload = self
                    .boot_value(kind == 17, key)
                    .map_or_else(|| vec![4], |value| tagged(3, value.as_bytes()));
                self.answer(id, payload);
            }
            3 | 4 => {
                let delay = reader.u32()?;
                if self.spec.as_ref().is_some_and(|spec| !spec.live_time) {
                    let effect = if kind == 4 {
                        spec_effects::Kind::Interval(delay.max(1))
                    } else {
                        spec_effects::Kind::Timer(delay)
                    };
                    self.simulated.hold(id, effect);
                } else {
                    let interval = millis(delay.max(1));
                    self.timers.insert(
                        id,
                        Timer {
                            next: Instant::now() + interval,
                            interval,
                            recurring: kind == 4,
                        },
                    );
                }
            }
            5..=7 | 20 => {}
            21 => {
                let payload = match system::crypto_count(reader)? {
                    Some(count) if count > regional::MAX_RANDOM => vec![30, 2],
                    Some(0) => vec![28],
                    Some(count) if self.spec.as_ref().is_some_and(|spec| !spec.live_entropy) => {
                        self.simulated.hold_random(id, count);
                        return Ok(());
                    }
                    Some(count) => system::secure_random(count),
                    None => vec![30, 1],
                };
                self.answer(id, payload);
            }
            22 => self.environment(id),
            19 => {
                // A raw request carries no native Copy gesture. Consume the
                // whole frame and deny it before the simulator or clipboard.
                clipboard::request(reader)?;
                self.answer(id, vec![24, 1]);
            }
            8 => {
                let order = workers::Order::read(reader)?;
                self.spawn(
                    id,
                    WorkOptions {
                        timeout: order.timeout,
                        timeout_tag: 7,
                        finish_on_cancel: order.finish_on_cancel,
                        unavailable: vec![11, 6],
                    },
                    move |stop, _| order.run(stop),
                );
            }
            9 => match reader.string()? {
                crate::wire::FOLDER_PORT => {
                    if let Some(folders::Request {
                        folder,
                        limit,
                        timeout,
                    }) = folders::request(reader)?
                    {
                        self.spawn(
                            id,
                            WorkOptions {
                                timeout,
                                timeout_tag: 7,
                                finish_on_cancel: false,
                                unavailable: vec![15, 2],
                            },
                            move |stop, _| match folder.list(limit, stop) {
                                Ok((files, flags)) => Outcome::Folder { files, flags },
                                Err(_) => Outcome::Bytes(vec![15, 3]),
                            },
                        );
                    } else {
                        self.answer(id, vec![15, 3]);
                    }
                }
                crate::wire::DECODE_IMAGE_PORT => {
                    let request = images::Request::read(reader)?;
                    self.spawn(
                        id,
                        WorkOptions {
                            timeout: request.timeout,
                            timeout_tag: 7,
                            finish_on_cancel: false,
                            unavailable: images::answer(5, false),
                        },
                        move |_, _| match request.decode() {
                            Ok(pixels) => Outcome::Raster(pixels),
                            Err(code) => Outcome::Bytes(images::answer(code, false)),
                        },
                    );
                }
                wire::MONOTONIC_PORT => {
                    let (body, _) = system::port_body(reader)?;
                    let answer = if !matches!(body, Some("{}" | "null")) {
                        system::monotonic(2, 0)
                    } else {
                        let mark = if self.spec.as_ref().is_some_and(|spec| !spec.live_time) {
                            Some(self.simulated.mono)
                        } else {
                            u64::try_from(self.started.elapsed().as_millis()).ok()
                        };
                        system::monotonic(u8::from(mark.is_none()), mark.unwrap_or_default())
                    };
                    self.answer(id, answer);
                }
                wire::REGIONAL_PORT => {
                    let (body, timeout) = system::port_body(reader)?;
                    if !matches!(body, Some("{}" | "null")) {
                        self.answer(id, system::regional_answer(2, "", None));
                    } else if let Some(spec) = self.spec.as_ref().filter(|spec| !spec.live_time) {
                        self.answer(
                            id,
                            system::regional_answer(
                                0,
                                &spec.regional.locale,
                                spec.regional.name.as_deref(),
                            ),
                        );
                    } else {
                        self.system_request(id, timeout, system::regional_settings);
                    }
                }
                wire::FORMAT_TIME_PORT => {
                    let (body, timeout) = system::port_body(reader)?;
                    match body.map(system::FormatRequest::read) {
                        Some(Ok(Some(request))) => {
                            if let Some(spec) = self.spec.as_ref().filter(|spec| !spec.live_time) {
                                let result = spec.regional.format(
                                    &request.instants,
                                    request.local,
                                    request.long,
                                );
                                let answer = match result {
                                    Ok(parts) => system::formatted_answer(0, &parts),
                                    Err(code) => system::formatted_answer(code, &[]),
                                };
                                self.answer(id, answer);
                            } else {
                                self.system_request(id, timeout, move || request.format());
                            }
                        }
                        Some(Ok(None)) => self.answer(id, system::formatted_answer(3, &[])),
                        _ => self.answer(id, system::formatted_answer(2, &[])),
                    }
                }
                port => return Err(format!("unsupported native port {port:?}")),
            },
            11 | 13 | 14 => {
                let task = files::FileTask::read(kind, reader)?;
                self.spawn(
                    id,
                    WorkOptions {
                        timeout: 0,
                        timeout_tag: 0,
                        finish_on_cancel: false,
                        unavailable: vec![13, 3],
                    },
                    move |stop, _| Outcome::Bytes(task.run(stop)),
                );
            }
            16 | 18 => {
                reader.string()?;
            }
            _ => return Err(format!("unsupported effect kind {kind}")),
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self, boot: bool) -> Result<Snapshot> {
        let span = self.declarations.zone_days;
        let live_entropy = self.spec.as_ref().is_none_or(|spec| spec.live_entropy);
        if let Some(spec) = self.spec.as_ref().filter(|spec| !spec.live_time) {
            let mut snapshot = spec
                .regional
                .snapshot(self.simulated.now, span)
                .or_else(|| boot.then(|| Snapshot::simulated(self.simulated.now, span)))
                .ok_or("the regional fixture does not cover the wall sample")?;
            if live_entropy && !regional::random(&mut snapshot.seed) {
                return Err("could not obtain random seed from Windows".into());
            }
            Ok(snapshot)
        } else {
            regional::snapshot(span, boot, live_entropy)
        }
    }

    fn environment(&mut self, id: u32) {
        let declarations = self.declarations.clone();
        let sampled = |session, keys: &[String]| {
            keys.iter()
                .filter_map(|key| Some((key.clone(), self.boot_value(session, key)?)))
                .collect::<HashMap<_, _>>()
        };
        let local = sampled(false, &declarations.local_keys);
        let session = sampled(true, &declarations.session_keys);
        let encode = move |facts: Result<Snapshot>| match facts {
            Ok(facts) => {
                let mut payload = vec![31];
                payload.extend(declarations.environment(&facts, |is_session, key| {
                    if is_session {
                        session.get(key)
                    } else {
                        local.get(key)
                    }
                    .cloned()
                }));
                payload
            }
            Err(_) => vec![REFRESH_FAILED],
        };
        if self.spec.as_ref().is_some_and(|spec| !spec.live_time) {
            self.answer(id, encode(self.snapshot(false)));
        } else {
            let live_entropy = self.spec.as_ref().is_none_or(|spec| spec.live_entropy);
            let span = self.declarations.zone_days;
            self.system_request(id, 15_000, move || {
                encode(regional::snapshot(span, false, live_entropy))
            });
        }
    }

    fn system_request(
        &mut self,
        id: u32,
        timeout: u32,
        work: impl FnOnce() -> Vec<u8> + Send + 'static,
    ) {
        self.spawn(
            id,
            WorkOptions {
                timeout,
                timeout_tag: REFRESH_FAILED,
                finish_on_cancel: false,
                unavailable: vec![REFRESH_FAILED],
            },
            move |_, _| Outcome::Bytes(work()),
        );
    }

    pub(crate) fn copy_from_button(&mut self, owner: HWND, text: &str) -> &'static str {
        match clipboard::validate_copy(text) {
            Ok(()) if self.copy_text(owner, text) => "copied",
            Ok(()) => "unavailable",
            Err(clipboard::CopyRefusal::InvalidText) => "invalid",
            Err(clipboard::CopyRefusal::TooLarge) => "too-large",
        }
    }

    fn copy_text(&mut self, owner: HWND, text: &str) -> bool {
        if text.contains('\0') {
            return false;
        }
        if self.spec.is_none() {
            return clipboard::write(owner, text).is_some();
        }
        if std::mem::take(&mut self.clipboard_busy) {
            return false;
        }
        self.clipboard_writes += 1;
        self.clipboard_text.clear();
        self.clipboard_text.push_str(text);
        true
    }

    pub(crate) fn boot_value(&self, session: bool, key: &str) -> Option<String> {
        if session {
            return self.session.get(key).cloned();
        }
        self.local.as_ref()?.read(key).ok()
    }

    pub(crate) fn storage(&mut self, op: u8, key: &str, value: Option<String>) -> Result<()> {
        match op {
            1 | 2 => {
                let local = self
                    .local
                    .as_ref()
                    .ok_or("local app data directory is unavailable")?;
                if op == 1 {
                    local
                        .write(key, value.as_deref().ok_or("missing local storage value")?)
                        .map_err(storage_error)?;
                } else {
                    local.remove(key).map_err(storage_error)?;
                }
            }
            3 => {
                self.session.insert(
                    key.to_owned(),
                    value.ok_or("missing session storage value")?,
                );
            }
            4 => {
                self.session.remove(key);
            }
            _ => return Err(format!("unsupported storage operation {op}")),
        }
        Ok(())
    }

    pub(crate) fn pick(
        &mut self,
        owner: HWND,
        node: u32,
        handler: u32,
        multiple: bool,
    ) -> Result<()> {
        let selection = pickers::open(owner, multiple)?;
        if let Some((record, handles)) = selection.and_then(files::selection_record) {
            self.send_files(node, handler, record, handles)?;
        }
        Ok(())
    }

    pub(crate) fn pick_folder(&mut self, owner: HWND, node: u32, handler: u32) -> Result<()> {
        let entry = match pickers::folder(owner) {
            pickers::FolderChoice::Chosen(entry) => Some(entry),
            pickers::FolderChoice::Refused => None,
            pickers::FolderChoice::Canceled => return Ok(()),
        };
        let (record, handles) = files::folder_record(entry);
        self.send_files(node, handler, record, handles)
    }

    pub(crate) fn deliver_folder(
        &mut self,
        node: u32,
        handler: u32,
        path: Option<PathBuf>,
    ) -> Result<()> {
        let (record, handles) = files::folder_selection(path);
        self.send_files(node, handler, record, handles)
    }

    pub(crate) fn deliver_paths(
        &mut self,
        node: u32,
        handler: u32,
        paths: Vec<PathBuf>,
    ) -> Result<()> {
        if let Some((record, handles)) = files::selection(paths) {
            self.send_files(node, handler, record, handles)?;
        }
        Ok(())
    }

    fn send_files(
        &self,
        node: u32,
        handler: u32,
        record: Vec<u8>,
        handles: Vec<u64>,
    ) -> Result<()> {
        if let Err(mpsc::SendError(Event::Files { handles, .. })) = self.send.send(Event::Files {
            node,
            handler,
            record,
            handles,
        }) {
            files::discard_entries(handles);
            return Err("window closed while delivering file capabilities".into());
        }
        Ok(())
    }

    pub(crate) fn save(
        &mut self,
        owner: HWND,
        token: u64,
        start: u64,
        len: u64,
        name: &str,
    ) -> Result<()> {
        if let Some(spec) = &self.spec {
            if !valid_file_name(name) {
                return Err("spec save needs an ordinary file name without a path".into());
            }
            let saved = spec.output.join("saved");
            std::fs::create_dir_all(&saved).map_err(storage_error)?;
            files::save_to(token, start, len, saved.join(name))
        } else {
            if let Err(error) = files::save(owner, token, start, len, name) {
                pickers::save_error(owner, &error);
            }
            Ok(())
        }
    }
}

impl Drop for Effects {
    fn drop(&mut self) {
        for flight in self.flights.values() {
            flight.cancelled.store(true, Ordering::Relaxed);
        }
        files::collect_released();
    }
}

pub(crate) fn raster_digest(bytes: &[u8]) -> Option<[u8; 32]> {
    files::sha256(bytes)
}

pub(crate) fn resolve_raster(
    token: u64,
    start: u64,
    len: u64,
) -> Result<Arc<crate::raster::Pixels>> {
    if start != 0 || len != 0 {
        return Err("a raster capability cannot be sliced".into());
    }
    files::resolve(token)?
        .image
        .ok_or_else(|| "expected a raster capability".into())
}

fn storage_error(error: io::Error) -> String {
    format!("could not update local storage: {error}")
}
fn millis(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}
fn tagged(tag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(1 + bytes.len());
    output.push(tag);
    output.extend(bytes);
    output
}
fn put_string(output: &mut Vec<u8>, string: &str) {
    output.extend((string.len() as u32).to_le_bytes());
    output.extend(string.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_clipboard_denial_leaves_a_native_copy_and_its_fixture_available() {
        let (send, receive) = mpsc::channel();
        let mut effects = Effects::new(send, Declarations::default()).unwrap();
        effects.spec = Some(SpecOptions {
            storage: PathBuf::new(),
            output: PathBuf::new(),
            local: Vec::new(),
            session: Vec::new(),
            initial_time: 0,
            live_time: false,
            live_http: false,
            live_entropy: false,
            regional: Default::default(),
            worker_limit: 1,
        });
        effects.clipboard_busy = true;
        let mut request = Reader::new(b"\x01\0\0\0a\x00\x55");
        effects.start(42, 19, &mut request).unwrap();
        assert_eq!(request.u8().unwrap(), 0x55);
        let Event::Turn {
            node,
            handler,
            payload,
        } = receive.try_recv().unwrap()
        else {
            panic!("expected a clipboard denial");
        };
        assert_eq!((node, handler, payload), (EFFECT_NODE, 42, vec![24, 1]));
        assert_eq!(effects.pending(), 0);
        assert_eq!(effects.clipboard_writes, 0);
        assert_eq!(
            effects.copy_from_button(std::ptr::null_mut(), "a"),
            "unavailable"
        );
        assert_eq!(
            effects.copy_from_button(std::ptr::null_mut(), "a"),
            "copied"
        );
        assert_eq!(effects.clipboard_text, "a");
    }
}
