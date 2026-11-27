use super::files::{self, Entry};
use super::{MAX_TIMEOUT, Result, put_string};
use crate::abi::{
    RocBox, RocList, box_handle, roc_engine_ask, roc_engine_forget, roc_engine_prepare,
    roc_engine_work,
};
use crate::wire::Reader;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_HANDLES: usize = 64;
const CONTRACT: u8 = 1;
const RESOURCE_LIMIT: u8 = 2;

pub(super) struct Order {
    task: String,
    input: Vec<u8>,
    files: Vec<Entry>,
    prep: Option<(String, usize, usize)>,
    pub(super) timeout: u32,
    pub(super) finish_on_cancel: bool,
}

impl Order {
    pub(super) fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let task = reader.string()?.to_owned();
        let len = reader.u32()? as usize;
        let input = reader.raw(len)?.to_vec();
        let count = reader.u32()? as usize;
        if count > MAX_HANDLES {
            return Err("too many worker file handles".into());
        }
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            let entry = files::resolve(reader.u64()?)?;
            let start = reader.u64()?;
            let len = reader.u64()?;
            if entry.folder.is_some() {
                return Err("worker file input is a folder capability".into());
            }
            if !start.checked_add(len).is_some_and(|end| end <= entry.size) {
                return Err("worker input range is outside its file".into());
            }
            files.push(entry);
        }
        let timeout = reader.u32()?.min(MAX_TIMEOUT);
        let policy = reader.u8()?;
        if !matches!(policy, 1 | 2) {
            return Err("invalid worker cancellation policy".into());
        }
        let prep = match reader.u8()? {
            0 => None,
            1 => {
                let name = reader.string()?.to_owned();
                let init_len = reader.u32()? as usize;
                let init_files = reader.u32()? as usize;
                if init_len > input.len() || init_files > files.len() {
                    return Err("invalid worker preparation arguments".into());
                }
                Some((name, init_len, init_files))
            }
            _ => return Err("invalid worker preparation flag".into()),
        };
        Ok(Self {
            task,
            input,
            files,
            prep,
            timeout,
            finish_on_cancel: policy == 2,
        })
    }

    pub(super) fn run(self, stop: &AtomicBool) -> super::Outcome {
        let mut output = Vec::new();
        let mut files = Vec::new();
        let result = self.call(stop, &mut output, &mut files);
        WORKER.with(|worker| worker.borrow_mut().clear());
        match result {
            Ok(()) => super::Outcome::Work { output, files },
            Err(reason) => super::Outcome::Bytes(vec![11, reason]),
        }
    }

    fn call(
        self,
        stop: &AtomicBool,
        output: &mut Vec<u8>,
        files: &mut Vec<Entry>,
    ) -> std::result::Result<(), u8> {
        let (input, inputs, prepared) = if let Some((name, len, count)) = &self.prep {
            let boxes = mint_inputs(&self.files[..*count])?;
            // The instance, its lists and any resulting state are used on this worker only.
            let held = unsafe { roc_engine_prepare(payload(name, &self.input[..*len]), boxes) };
            (&self.input[*len..], &self.files[*count..], Some(held))
        } else {
            (&self.input[..], &self.files[..], None)
        };
        let answer = match (mint_inputs(inputs), prepared) {
            (Ok(boxes), Some(held)) => unsafe {
                let answer = roc_engine_ask(held, payload(&self.task, input), boxes);
                roc_engine_forget(answer.held);
                answer.out
            },
            (Ok(boxes), None) => unsafe { roc_engine_work(payload(&self.task, input), boxes) },
            (Err(reason), held) => {
                if let Some(held) = held {
                    unsafe {
                        roc_engine_forget(held);
                    }
                }
                return Err(reason);
            }
        };
        let result = if stop.load(Ordering::Relaxed) {
            Err(6)
        } else {
            decode_answer(answer.bytes(), output, files)
        };
        unsafe {
            answer.release_bytes();
        }
        result
    }
}

fn payload(name: &str, input: &[u8]) -> RocList {
    let mut data = Vec::new();
    put_string(&mut data, name);
    data.extend(input);
    RocList::owned_bytes(&data)
}

fn decode_answer(
    bytes: &[u8],
    output: &mut Vec<u8>,
    files: &mut Vec<Entry>,
) -> std::result::Result<(), u8> {
    let mut reader = Reader::new(bytes);
    let count = reader.u32().map_err(|_| CONTRACT)?;
    if count as usize > MAX_HANDLES {
        return Err(RESOURCE_LIMIT);
    }
    for _ in 0..count {
        let token = reader.u64().map_err(|_| CONTRACT)?;
        files.push(entry(token).ok_or(CONTRACT)?);
    }
    output.extend(
        reader
            .raw(bytes.len() - 4 - 8 * count as usize)
            .map_err(|_| CONTRACT)?,
    );
    Ok(())
}

enum Resource {
    Empty,
    File(Entry),
    Builder {
        name: String,
        mime: String,
        data: Vec<u8>,
    },
}
thread_local! {
    static WORKER: RefCell<HashMap<u64, Resource>> = RefCell::new(HashMap::new());
}

fn mint_inputs(files: &[Entry]) -> std::result::Result<RocList, u8> {
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        if worker.len() + files.len() > MAX_HANDLES {
            return Err(RESOURCE_LIMIT);
        }
        let mut tokens = Vec::with_capacity(files.len());
        for file in files {
            let token = files::next_token();
            worker.insert(token, Resource::File(file.clone()));
            tokens.push(token);
        }
        Ok(RocList::owned_handles(&tokens))
    })
}

fn entry(token: u64) -> Option<Entry> {
    WORKER.with(|worker| match worker.borrow().get(&token) {
        Some(Resource::File(file)) => Some(file.clone()),
        _ => None,
    })
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_size(token: u64) -> u64 {
    entry(token).map_or(0, |file| file.size)
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_read(token: u64, start: u64, len: u64) -> RocList {
    let bytes = entry(token)
        .and_then(|file| file.read_range(start, len).ok())
        .unwrap_or_default();
    RocList::owned_bytes(&bytes)
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_mint() -> RocBox {
    let token = files::next_token();
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        if worker.len() < MAX_HANDLES {
            worker.insert(token, Resource::Empty);
        }
    });
    box_handle(token)
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_open(
    token: u64,
    name: RocList,
    mime: RocList,
    from: u64,
    start: u64,
    len: u64,
) -> u8 {
    let chosen_name = String::from_utf8_lossy(name.bytes()).into_owned();
    let chosen_mime = String::from_utf8_lossy(mime.bytes()).into_owned();
    unsafe {
        name.release_bytes();
        mime.release_bytes();
    }
    let data = if from == 0 {
        Vec::new()
    } else {
        let Some(data) = entry(from).and_then(|file| file.read_range(start, len).ok()) else {
            return 2;
        };
        data
    };
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        if !matches!(worker.get(&token), Some(Resource::Empty)) {
            return 2;
        }
        worker.insert(
            token,
            Resource::Builder {
                name: chosen_name,
                mime: chosen_mime,
                data,
            },
        );
        0
    })
}

fn append(token: u64, bytes: &[u8]) -> u8 {
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        let Some(Resource::Builder { data, .. }) = worker.get_mut(&token) else {
            return 2;
        };
        if data.len().saturating_add(bytes.len()) > 64 << 20 {
            return 1;
        }
        data.extend(bytes);
        0
    })
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_append(token: u64, bytes: RocList) -> u8 {
    let status = append(token, bytes.bytes());
    unsafe {
        bytes.release_bytes();
    }
    status
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_append_file(token: u64, file: u64, start: u64, len: u64) -> u8 {
    let Some(data) = entry(file).and_then(|file| file.read_range(start, len).ok()) else {
        return 2;
    };
    append(token, &data)
}

#[unsafe(no_mangle)]
extern "C" fn roc_task_finish(token: u64, destination: u64) -> u8 {
    WORKER.with(|worker| {
        let mut worker = worker.borrow_mut();
        if !matches!(worker.get(&destination), Some(Resource::Empty)) {
            return 2;
        }
        let Some(Resource::Builder { name, mime, data }) = worker.get(&token) else {
            return 2;
        };
        let Ok(file) = Entry::temporary(name, mime, data) else {
            return 2;
        };
        worker.remove(&token);
        worker.insert(destination, Resource::File(file));
        0
    })
}
