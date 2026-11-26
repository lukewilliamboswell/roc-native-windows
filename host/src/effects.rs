//! Operating-system effects requested by the engine. Only the UI thread delivers turns.

mod files;
mod http;
mod workers;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::HWND;

use crate::spec_effects::{self, Simulated};
use crate::spec_plan::valid_file_name;
use crate::wire::Reader;
use crate::{Event, Result};

pub(crate) use files::{register_handle_box, released_box};

const EFFECT_NODE: u32 = u32::MAX;
const MAX_TIMEOUT: u32 = 600_000;
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
    Work {
        output: Vec<u8>,
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
    pub(crate) worker_limit: usize,
}

pub(crate) struct Effects {
    window: HWND,
    send: mpsc::Sender<Event>,
    complete: mpsc::Sender<Completion>,
    completed: mpsc::Receiver<Completion>,
    flights: HashMap<u32, Flight>,
    running: Arc<std::sync::atomic::AtomicUsize>,
    timers: HashMap<u32, Timer>,
    session: HashMap<String, String>,
    local: Option<PathBuf>,
    queued: VecDeque<Queued>,
    spec: Option<SpecOptions>,
    pub(crate) simulated: Simulated,
}

impl Effects {
    pub(crate) fn new(window: HWND, send: mpsc::Sender<Event>) -> Self {
        let (complete, completed) = mpsc::channel();
        let local = std::env::var_os("LOCALAPPDATA").and_then(|directory| {
            let executable = std::env::current_exe().ok()?;
            Some(
                PathBuf::from(directory)
                    .join("roc-native-windows")
                    .join(executable.file_stem()?)
                    .join("storage"),
            )
        });
        Self {
            window,
            send,
            complete,
            completed,
            flights: HashMap::new(),
            running: Arc::default(),
            timers: HashMap::new(),
            session: HashMap::new(),
            local,
            queued: VecDeque::new(),
            spec: None,
            simulated: Simulated::default(),
        }
    }

    pub(crate) fn configure_spec(&mut self, mut options: SpecOptions) -> Result<()> {
        self.local = Some(options.storage.clone());
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
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let mut payload = vec![5];
            payload.extend(timestamp.to_le_bytes());
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
                    Outcome::Work { output, files } => self.work_answer(id, output, files),
                }
            }
        }
        for (id, payload) in answers {
            self.answer(id, payload);
        }
        self.start_queued();
    }

    fn work_answer(&self, id: u32, output: Vec<u8>, entries: Vec<files::Entry>) {
        let mut record = vec![10];
        record.extend((entries.len() as u32).to_le_bytes());
        let Some(handles) = files::mint_entries(entries, &mut record) else {
            self.answer(id, vec![11, 2]);
            return;
        };
        record.extend(output);
        if handles.is_empty() {
            self.answer(id, record);
        } else {
            let _ = self.send.send(Event::Files {
                node: EFFECT_NODE,
                handler: id,
                record,
                handles,
            });
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
                if self.spec.as_ref().is_some_and(|spec| !spec.live_http) {
                    self.simulated
                        .hold(id, spec_effects::Kind::Http(request.spec_request()));
                } else {
                    self.spawn(
                        id,
                        WorkOptions {
                            timeout: request.timeout,
                            timeout_tag: 7,
                            finish_on_cancel: false,
                            unavailable: http::failure("HTTP worker unavailable"),
                        },
                        move |stop, report| Outcome::Bytes(request.run(stop, report)),
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
            5..=7 | 20 | 22 => {}
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

    pub(crate) fn boot_value(&self, session: bool, key: &str) -> Option<String> {
        if session {
            return self.session.get(key).cloned();
        }
        std::fs::read_to_string(self.local_path(key)?).ok()
    }

    pub(crate) fn storage(&mut self, op: u8, key: &str, value: Option<String>) -> Result<()> {
        match op {
            1 | 2 => {
                let path = self
                    .local_path(key)
                    .ok_or("local app data directory is unavailable")?;
                if op == 1 {
                    std::fs::create_dir_all(path.parent().ok_or("invalid storage directory")?)
                        .map_err(storage_error)?;
                    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
                    std::fs::write(&staging, value.ok_or("missing local storage value")?)
                        .map_err(storage_error)?;
                    std::fs::rename(staging, path).map_err(storage_error)?;
                } else if let Err(error) = std::fs::remove_file(path)
                    && error.kind() != io::ErrorKind::NotFound
                {
                    return Err(storage_error(error));
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

    fn local_path(&self, key: &str) -> Option<PathBuf> {
        let hash = files::sha256(key.as_bytes())?;
        let mut file = String::with_capacity(64);
        for byte in hash {
            use std::fmt::Write;
            let _ = write!(file, "{byte:02x}");
        }
        Some(self.local.as_ref()?.join(file))
    }

    pub(crate) fn pick(&mut self, node: u32, handler: u32, multiple: bool) -> Result<()> {
        let paths = files::open_dialog(self.window, multiple)?;
        self.deliver_paths(node, handler, paths)
    }

    pub(crate) fn deliver_paths(
        &mut self,
        node: u32,
        handler: u32,
        paths: Vec<PathBuf>,
    ) -> Result<()> {
        if let Some((record, handles)) = files::selection(paths) {
            self.send
                .send(Event::Files {
                    node,
                    handler,
                    record,
                    handles,
                })
                .map_err(|_| "window closed while delivering files".to_owned())?;
        }
        Ok(())
    }

    pub(crate) fn save(&mut self, token: u64, start: u64, len: u64, name: &str) -> Result<()> {
        if let Some(spec) = &self.spec {
            if !valid_file_name(name) {
                return Err("spec save needs an ordinary file name without a path".into());
            }
            let saved = spec.output.join("saved");
            std::fs::create_dir_all(&saved).map_err(storage_error)?;
            files::save_to(token, start, len, saved.join(name))
        } else {
            files::save(self.window, token, start, len, name)
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
