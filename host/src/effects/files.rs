use super::{Result, folders::Folder, put_string, tagged};
use crate::wire::Reader;
use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptHash,
    BCryptOpenAlgorithmProvider,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_ID_INFO, FILE_TYPE_DISK, FileIdInfo,
    GetFileInformationByHandleEx, GetFileType,
};
use windows_sys::Win32::UI::Controls::Dialogs::*;

const MAX_FILE_BYTES: u64 = 256 << 20;
const MAX_HANDLES: usize = 64;

#[derive(Clone)]
pub(super) struct Entry {
    pub(super) size: u64,
    name: String,
    mime: String,
    pub(super) folder: Option<Arc<Folder>>,
    file: Option<Arc<Mutex<File>>>,
    pub(super) image: Option<Arc<crate::raster::Pixels>>,
}

impl Entry {
    fn new(path: PathBuf) -> io::Result<Self> {
        let file = File::open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || !is_disk_file(&file) {
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
            size: meta.len(),
            name,
            mime,
            folder: None,
            file: Some(Arc::new(Mutex::new(file))),
            image: None,
        })
    }

    fn for_folder(path: PathBuf) -> io::Result<Self> {
        let folder = Arc::new(Folder::open(&path)?);
        let name = path
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("Selected folder"))
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            size: 0,
            name,
            mime: String::new(),
            folder: Some(folder),
            file: None,
            image: None,
        })
    }

    pub(super) fn folder_file(path: PathBuf, file: File, size: u64) -> Self {
        let name = path.to_string_lossy().into_owned();
        let mime = mime(&path).into();
        Self {
            size,
            name,
            mime,
            folder: None,
            file: Some(Arc::new(Mutex::new(file))),
            image: None,
        }
    }

    pub(super) fn temporary(name: &str, mime: &str, bytes: &[u8]) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "roc-windows-{}-{}.tmp",
            std::process::id(),
            next_token()
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .open(&path)?;
        file.write_all(bytes)?;
        Ok(Self {
            size: bytes.len() as u64,
            name: name.into(),
            mime: mime.into(),
            folder: None,
            file: Some(Arc::new(Mutex::new(file))),
            image: None,
        })
    }

    pub(super) fn image(pixels: Arc<crate::raster::Pixels>) -> Self {
        Self {
            size: 0,
            name: "Image".into(),
            mime: "application/x-roc-windows-raster".into(),
            folder: None,
            file: None,
            image: Some(pixels),
        }
    }

    pub(super) fn read_range(&self, start: u64, len: u64) -> io::Result<Vec<u8>> {
        if len > MAX_FILE_BYTES || !start.checked_add(len).is_some_and(|end| end <= self.size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file range is outside the supported bounds",
            ));
        }
        let mut file = self.lock_file()?;
        read_range(&mut file, start, len)
    }

    fn lock_file(&self) -> io::Result<std::sync::MutexGuard<'_, File>> {
        let file = self.file.as_ref().ok_or_else(|| {
            if self.folder.is_some() {
                io::Error::from(io::ErrorKind::IsADirectory)
            } else {
                io::Error::from(io::ErrorKind::InvalidInput)
            }
        })?;
        file.lock()
            .map_err(|_| io::Error::other("file handle lock poisoned"))
    }

    pub(super) fn encode(&self, record: &mut Vec<u8>) {
        record.extend(self.size.to_le_bytes());
        put_string(record, &self.name);
        put_string(record, &self.mime);
    }
}

fn read_range(file: &mut File, start: u64, len: u64) -> io::Result<Vec<u8>> {
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

pub(super) fn discard_entries(tokens: Vec<u64>) {
    let mut handles = HANDLES.lock().unwrap();
    for token in tokens {
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
    let requested = paths.len();
    if requested == 0 {
        return None;
    }
    let remaining = MAX_HANDLES.saturating_sub(HANDLES.lock().unwrap().files.len());
    let entries: Vec<_> = paths
        .into_iter()
        .filter_map(|path| Entry::new(path).ok())
        .take(remaining)
        .collect();
    let accepted = entries.len();
    let mut record = Vec::new();
    record.extend((accepted as u32).to_le_bytes());
    record.extend(((requested - accepted) as u32).to_le_bytes());
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

pub(super) fn folder_selection(path: Option<PathBuf>) -> (Vec<u8>, Vec<u64>) {
    if let Some(entry) = path.and_then(|path| Entry::for_folder(path).ok()) {
        let mut record = vec![1, 0, 0, 0, 0, 0, 0, 0];
        if let Some(handles) = mint_entries(vec![entry], &mut record) {
            return (record, handles);
        }
    }
    (vec![0, 0, 0, 0, 1, 0, 0, 0], Vec::new())
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
            // A file URL would give the pathname rather than the opened object.
            return vec![13, 3];
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
        write_range(entry, start, len, destination)?;
    }
    Ok(())
}

pub(super) fn save_to(token: u64, start: u64, len: u64, destination: PathBuf) -> Result<()> {
    write_range(resolve(token)?, start, len, destination)
}

fn write_range(entry: Entry, start: u64, len: u64, destination: PathBuf) -> Result<()> {
    let end = start
        .checked_add(len)
        .filter(|&end| end <= entry.size)
        .ok_or("save range is outside the file")?;
    let mut input = entry.lock_file().map_err(file_error)?;
    let current_size = input.metadata().map_err(file_error)?.len();
    if end > current_size {
        return Err("file changed before it could be saved".into());
    }
    let source_id = file_id(&input).map_err(file_error)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(destination)
        .map_err(file_error)?;
    if !is_disk_file(&output) || !output.metadata().map_err(file_error)?.is_file() {
        return Err("expected a regular file for export".into());
    }
    let destination_id = file_id(&output).map_err(file_error)?;
    if source_id == destination_id {
        if start == 0 && len == current_size {
            return Ok(());
        }
        if len > MAX_FILE_BYTES {
            return Err("in-place export range exceeds the supported size".into());
        }
        let bytes = read_range(&mut input, start, len).map_err(file_error)?;
        output.write_all(&bytes).map_err(file_error)?;
        return output.set_len(len).map_err(file_error);
    }
    input.seek(SeekFrom::Start(start)).map_err(file_error)?;
    output.set_len(0).map_err(file_error)?;
    if io::copy(&mut (&mut *input).take(len), &mut output).map_err(file_error)? != len {
        return Err("file changed while being saved".into());
    }
    Ok(())
}

fn is_disk_file(file: &File) -> bool {
    // SAFETY: The handle is live for the duration of the Win32 call.
    unsafe { GetFileType(file.as_raw_handle()) == FILE_TYPE_DISK }
}

fn file_id(file: &File) -> io::Result<(u64, [u8; 16])> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: The live handle and correctly sized output buffer outlive the call.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "roc-file-tests-{}-{}",
                std::process::id(),
                next_token()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn selection_reads_and_exports_the_open_object_after_its_path_changes() {
        let directory = TestDirectory::new();
        let selected = directory.0.join("selected.txt");
        let renamed = directory.0.join("renamed.txt");
        let exported = directory.0.join("exported.txt");
        std::fs::write(&selected, b"first").unwrap();

        let (_, tokens) = selection(vec![selected.clone()]).unwrap();
        let entry = resolve(tokens[0]).unwrap();
        discard_entries(tokens);
        std::fs::rename(&selected, &renamed).unwrap();
        std::fs::write(&selected, b"other").unwrap();

        assert_eq!(entry.read_range(0, 5).unwrap(), b"first");
        let url = FileTask {
            entry: entry.clone(),
            start: 0,
            len: 5,
            operation: Operation::Url,
        };
        assert_eq!(url.run(&AtomicBool::new(false)), [13, 3]);
        write_range(entry, 1, 3, exported.clone()).unwrap();
        assert_eq!(std::fs::read(exported).unwrap(), b"irs");
        assert_eq!(std::fs::read(selected).unwrap(), b"other");
    }

    #[test]
    fn a_shortened_source_leaves_the_export_target_untouched() {
        let directory = TestDirectory::new();
        let selected = directory.0.join("selected.txt");
        let target = directory.0.join("export.txt");
        std::fs::write(&selected, b"first").unwrap();
        std::fs::write(&target, b"existing").unwrap();

        let entry = Entry::new(selected.clone()).unwrap();
        std::fs::write(selected, b"x").unwrap();

        assert!(write_range(entry, 0, 5, target.clone()).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"existing");
    }

    #[test]
    fn worker_temporary_bytes_are_read_from_the_open_handle() {
        let entry = Entry::temporary("built.txt", "text/plain", b"worker output").unwrap();
        assert_eq!(entry.read_range(0, 13).unwrap(), b"worker output");
        assert_eq!(entry.read_range(7, 6).unwrap(), b"output");
    }

    #[test]
    fn export_to_an_alias_reads_before_truncation() {
        let directory = TestDirectory::new();
        let selected = directory.0.join("selected.txt");
        let alias = directory.0.join("alias.txt");
        std::fs::write(&selected, b"first").unwrap();
        std::fs::hard_link(&selected, &alias).unwrap();

        let entry = Entry::new(selected.clone()).unwrap();
        write_range(entry, 1, 3, alias).unwrap();

        assert_eq!(std::fs::read(selected).unwrap(), b"irs");
    }
}
