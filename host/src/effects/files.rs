use super::{Result, put_string, tagged};
use crate::wire::Reader;
use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptHash,
    BCryptOpenAlgorithmProvider,
};
use windows_sys::Win32::UI::Controls::Dialogs::*;

const MAX_FILE_BYTES: u64 = 256 << 20;
const MAX_HANDLES: usize = 64;

#[derive(Clone)]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) size: u64,
    name: String,
    mime: String,
    _temporary: Option<Arc<Temporary>>,
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Entry {
    fn new(path: PathBuf) -> io::Result<Self> {
        let meta = path.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a regular file",
            ));
        }
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let mime = mime(&path).to_owned();
        Ok(Self {
            path,
            size: meta.len(),
            name,
            mime,
            _temporary: None,
        })
    }

    pub(super) fn temporary(name: &str, mime: &str, bytes: &[u8]) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "roc-windows-{}-{}.tmp",
            std::process::id(),
            next_token()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let backing = Arc::new(Temporary(path.clone()));
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            return Err(error);
        }
        Ok(Self {
            path,
            size: bytes.len() as u64,
            name: name.into(),
            mime: mime.into(),
            _temporary: Some(backing),
        })
    }

    pub(super) fn read_range(&self, start: u64, len: u64) -> io::Result<Vec<u8>> {
        if len > MAX_FILE_BYTES || !start.checked_add(len).is_some_and(|end| end <= self.size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file range is outside the supported bounds",
            ));
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(start))?;
        let mut data = Vec::new();
        file.take(len).read_to_end(&mut data)?;
        if data.len() as u64 != len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file changed while being read",
            ));
        }
        Ok(data)
    }

    pub(super) fn encode(&self, record: &mut Vec<u8>) {
        record.extend(self.size.to_le_bytes());
        put_string(record, &self.name);
        put_string(record, &self.mime);
    }
}

struct Handles {
    files: HashMap<u64, Entry>,
    boxes: HashMap<usize, u64>,
    released: Vec<u64>,
}

static HANDLES: std::sync::LazyLock<Mutex<Handles>> = std::sync::LazyLock::new(|| {
    Mutex::new(Handles {
        files: HashMap::new(),
        boxes: HashMap::new(),
        released: Vec::new(),
    })
});
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_token() -> u64 {
    NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn register_handle_box(base: *mut c_void, token: u64) {
    HANDLES.lock().unwrap().boxes.insert(base as usize, token);
}

pub(crate) fn released_box(base: *mut c_void) {
    let mut handles = HANDLES.lock().unwrap();
    if let Some(token) = handles.boxes.remove(&(base as usize))
        && handles.files.contains_key(&token)
    {
        handles.released.push(token);
    }
}

pub(super) fn collect_released() {
    let mut handles = HANDLES.lock().unwrap();
    for token in std::mem::take(&mut handles.released) {
        handles.files.remove(&token);
    }
}

pub(super) fn resolve(token: u64) -> Result<Entry> {
    HANDLES
        .lock()
        .unwrap()
        .files
        .get(&token)
        .cloned()
        .ok_or_else(|| format!("unknown file handle {token}"))
}

pub(super) fn selection(paths: Vec<PathBuf>) -> Option<(Vec<u8>, Vec<u64>)> {
    let mut entries: Vec<_> = paths
        .into_iter()
        .filter_map(|path| Entry::new(path).ok())
        .collect();
    if entries.is_empty() {
        return None;
    }
    let table = HANDLES.lock().unwrap();
    let accepted = entries
        .len()
        .min(MAX_HANDLES.saturating_sub(table.files.len()));
    let mut record = Vec::new();
    record.extend((accepted as u32).to_le_bytes());
    record.extend(((entries.len() - accepted) as u32).to_le_bytes());
    drop(table);
    entries.truncate(accepted);
    let handles = mint_entries(entries, &mut record)?;
    Some((record, handles))
}

pub(super) fn mint_entries(entries: Vec<Entry>, record: &mut Vec<u8>) -> Option<Vec<u64>> {
    let mut table = HANDLES.lock().unwrap();
    if entries.len() > MAX_HANDLES.saturating_sub(table.files.len()) {
        return None;
    }
    let mut handles = Vec::with_capacity(entries.len());
    for entry in entries {
        let token = next_token();
        entry.encode(record);
        table.files.insert(token, entry);
        handles.push(token);
    }
    Some(handles)
}

enum Operation {
    Read(u8, u64),
    Digest,
    Url,
}
pub(super) struct FileTask {
    entry: Entry,
    start: u64,
    len: u64,
    operation: Operation,
}

impl FileTask {
    pub(super) fn read(kind: u8, reader: &mut Reader<'_>) -> Result<Self> {
        let mode = if kind == 14 { 0 } else { reader.u8()? };
        let entry = resolve(reader.u64()?)?;
        let start = reader.u64()?;
        let len = reader.u64()?;
        let operation = match (kind, mode) {
            (11, 1 | 2) => Operation::Read(15 + mode, reader.u64()?),
            (13, 1) => Operation::Digest,
            (14, 0) => Operation::Url,
            _ => return Err(format!("invalid file effect {kind} mode {mode}")),
        };
        Ok(Self {
            entry,
            start,
            len,
            operation,
        })
    }

    pub(super) fn run(self, stop: &AtomicBool) -> Vec<u8> {
        if let Operation::Url = self.operation {
            if self.start != 0 || self.len != self.entry.size {
                return vec![13, 3];
            }
            return tagged(19, file_url(&self.entry.path).as_bytes());
        }
        let limit = match self.operation {
            Operation::Read(_, limit) => limit,
            _ => MAX_FILE_BYTES,
        };
        if self.len > limit.min(MAX_FILE_BYTES) {
            return vec![13, 2];
        }
        if stop.load(Ordering::Relaxed) {
            return vec![13, 3];
        }
        let Ok(data) = self.entry.read_range(self.start, self.len) else {
            return vec![13, 1];
        };
        match self.operation {
            Operation::Read(tag, _) => tagged(tag, &data),
            Operation::Digest => {
                sha256(&data).map_or_else(|| vec![13, 3], |digest| tagged(18, &digest))
            }
            Operation::Url => unreachable!(),
        }
    }
}

pub(super) fn sha256(data: &[u8]) -> Option<[u8; 32]> {
    let mut provider: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
    let mut digest = [0; 32];
    // The provider and buffers are local to this operation and closed on every path.
    unsafe {
        if BCryptOpenAlgorithmProvider(&mut provider, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0)
            < 0
        {
            return None;
        }
        let status = BCryptHash(
            provider,
            std::ptr::null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            digest.as_mut_ptr(),
            digest.len() as u32,
        );
        BCryptCloseAlgorithmProvider(provider, 0);
        (status >= 0).then_some(digest)
    }
}

pub(super) fn open_dialog(window: HWND, multiple: bool) -> Result<Vec<PathBuf>> {
    dialog(window, None, multiple)
}

pub(super) fn save(window: HWND, token: u64, start: u64, len: u64, name: &str) -> Result<()> {
    let entry = resolve(token)?;
    if let Some(destination) = dialog(window, Some(name), false)?.into_iter().next() {
        if !start.checked_add(len).is_some_and(|end| end <= entry.size) {
            return Err("save range is outside the file".into());
        }
        let mut input = File::open(&entry.path).map_err(file_error)?;
        input.seek(SeekFrom::Start(start)).map_err(file_error)?;
        if destination
            .canonicalize()
            .ok()
            .is_some_and(|path| entry.path.canonicalize().ok() == Some(path))
        {
            let bytes = entry.read_range(start, len).map_err(file_error)?;
            return std::fs::write(destination, bytes).map_err(file_error);
        }
        let mut output = File::create(destination).map_err(file_error)?;
        if io::copy(&mut input.take(len), &mut output).map_err(file_error)? != len {
            return Err("file changed while being saved".into());
        }
    }
    Ok(())
}

fn file_error(error: io::Error) -> String {
    format!("file operation failed: {error}")
}

fn dialog(window: HWND, name: Option<&str>, multiple: bool) -> Result<Vec<PathBuf>> {
    let mut names = vec![0u16; 32_768];
    if let Some(name) = name {
        for (target, char) in names.iter_mut().take(260).zip(name.encode_utf16()) {
            *target = char;
        }
    }
    let mut flags = OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    flags |= if name.is_some() {
        OFN_OVERWRITEPROMPT
    } else {
        OFN_FILEMUSTEXIST
    };
    if multiple {
        flags |= OFN_ALLOWMULTISELECT;
    }
    let mut form = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFile: names.as_mut_ptr(),
        nMaxFile: names.len() as u32,
        Flags: flags,
        ..Default::default()
    };
    // The buffer and form outlive the synchronous modal dialog.
    let ok = unsafe {
        if name.is_some() {
            GetSaveFileNameW(&mut form)
        } else {
            GetOpenFileNameW(&mut form)
        }
    };
    if ok == 0 {
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 {
            Ok(Vec::new())
        } else {
            Err(format!("Windows file dialog failed ({code:#x})"))
        };
    }
    let mut parts = names
        .split(|&char| char == 0)
        .take_while(|part| !part.is_empty())
        .map(OsString::from_wide);
    let Some(first) = parts.next() else {
        return Ok(Vec::new());
    };
    let path = PathBuf::from(first);
    let results: Vec<_> = parts.map(|leaf| path.join(leaf)).collect();
    if results.is_empty() {
        Ok(vec![path])
    } else {
        Ok(results)
    }
}

fn file_url(path: &Path) -> String {
    let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let path = String::from_utf16_lossy(&path).replace('\\', "/");
    let (mut url, suffix) = match path.strip_prefix("//") {
        Some(hosted) => (String::from("file://"), hosted),
        None => (String::from("file:///"), path.as_str()),
    };
    for byte in suffix.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "bmp" => "image/bmp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}
