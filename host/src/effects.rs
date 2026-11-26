//! Operating-system effects requested by the engine. Only the UI thread delivers turns.

mod files;
mod http;
mod workers;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::HWND;

use crate::wire::Reader;
use crate::{Event, Result};

pub(crate) use files::{register_handle_box, released_box};

const EFFECT_NODE: u32 = u32::MAX;
const MAX_TIMEOUT: u32 = 600_000;
type Completion = (u32, Arc<AtomicBool>, Outcome);

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
        }
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
                answers.push((id, vec![tag]));
                return false;
            }
            true
        });
        files::collect_released();
        for (id, token, outcome) in self.completed.try_iter() {
            if self
                .flights
                .get(&id)
                .is_some_and(|active| Arc::ptr_eq(&active.cancelled, &token))
            {
                if !matches!(outcome, Outcome::Progress(_)) {
                    self.flights.remove(&id);
                }
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
        timeout: u32,
        timeout_tag: u8,
        on_unavailable: Vec<u8>,
        task: impl FnOnce(&AtomicBool, &dyn Fn(Vec<u8>)) -> Outcome + Send + 'static,
    ) {
        // This count includes cancelled jobs until their OS call or Roc turn returns.
        if self.running.load(Ordering::Relaxed) >= 32 {
            self.answer(id, on_unavailable);
            return;
        }
        self.running.fetch_add(1, Ordering::Relaxed);
        let running = Arc::clone(&self.running);
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&cancelled);
        let sender = self.complete.clone();
        let deadline = (timeout != 0).then(|| {
            (
                Instant::now() + millis(timeout.min(MAX_TIMEOUT)),
                timeout_tag,
            )
        });
        match std::thread::Builder::new()
            .name("roc-effect".into())
            .stack_size(8 << 20)
            .spawn(move || {
                if !token.load(Ordering::Relaxed) {
                    let outcome = task(&token, &|payload| {
                        if !token.load(Ordering::Relaxed) {
                            let _ =
                                sender.send((id, Arc::clone(&token), Outcome::Progress(payload)));
                        }
                    });
                    let _ = sender.send((id, token, outcome));
                }
                running.fetch_sub(1, Ordering::Relaxed);
            }) {
            Ok(_) => {
                self.flights.insert(
                    id,
                    Flight {
                        cancelled,
                        deadline,
                    },
                );
            }
            Err(_) => {
                self.running.fetch_sub(1, Ordering::Relaxed);
                self.answer(id, on_unavailable);
            }
        }
    }

    pub(crate) fn cancel(&mut self, id: u32) {
        self.timers.remove(&id);
        if let Some(flight) = self.flights.remove(&id) {
            flight.cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn start(&mut self, id: u32, kind: u8, reader: &mut Reader<'_>) -> Result<()> {
        match kind {
            1 => {
                let request = http::Request::read(reader)?;
                self.spawn(
                    id,
                    request.timeout,
                    7,
                    http::failure("HTTP worker unavailable"),
                    move |stop, report| Outcome::Bytes(request.run(stop, report)),
                );
            }
            2 | 17 => {
                let key = reader.string()?;
                let payload = self
                    .boot_value(kind == 17, key)
                    .map_or_else(|| vec![4], |value| tagged(3, value.as_bytes()));
                self.answer(id, payload);
            }
            3 | 4 => {
                let interval = millis(reader.u32()?.max(1));
                self.timers.insert(
                    id,
                    Timer {
                        next: Instant::now() + interval,
                        interval,
                        recurring: kind == 4,
                    },
                );
            }
            5..=7 | 20 | 22 => {}
            8 => {
                let order = workers::Order::read(reader)?;
                self.spawn(id, order.timeout, 7, vec![11, 6], move |stop, _| {
                    order.run(stop)
                });
            }
            11 | 13 | 14 => {
                let task = files::FileTask::read(kind, reader)?;
                self.spawn(id, 0, 0, vec![13, 3], move |stop, _| {
                    Outcome::Bytes(task.run(stop))
                });
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
        files::save(self.window, token, start, len, name)
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
