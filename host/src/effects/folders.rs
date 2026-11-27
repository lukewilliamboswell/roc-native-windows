use super::{
    Result,
    files::{self, Entry},
};
use crate::wire::Reader;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_NO_MORE_FILES, HWND, RtlNtStatusToDosError, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdBothDirectoryInfo,
    FileIdBothDirectoryRestartInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

const MAX_RESULTS: usize = 16;
const MAX_SCANNED: usize = 4096;

pub(super) struct Folder(Mutex<File>);

pub(super) struct Request {
    pub(super) folder: Arc<Folder>,
    pub(super) limit: usize,
    pub(super) timeout: u32,
}

pub(super) fn request(reader: &mut Reader<'_>) -> Result<Option<Request>> {
    let json = reader.string()?;
    // The private typed record's PortJson codec emits ordered, compact keys.
    let limit = json
        .strip_prefix("{\"folder\":{\"$file\":0},\"limit\":")
        .and_then(|rest| rest.strip_suffix('}'))
        .and_then(|value| value.parse().ok())
        .filter(|limit| (1..=MAX_RESULTS).contains(limit));
    if reader.u32()? != 0 || reader.u32()? != 1 {
        return Err("folder port expects one file attachment and no blobs".into());
    }
    let token = reader.u64()?;
    let start = reader.u64()?;
    let len = reader.u64()?;
    let timeout = reader.u32()?;
    let folder = files::resolve(token).ok().and_then(|entry| entry.folder);
    Ok(if start == 0 && len == 0 {
        folder.zip(limit).map(|(folder, limit)| Request {
            folder,
            limit,
            timeout,
        })
    } else {
        None
    })
}

pub(super) fn answer_prefix(count: usize, flags: u8) -> Vec<u8> {
    use std::fmt::Write;
    let mut json = String::from("{\"files\":[");
    for index in 0..count {
        if index != 0 {
            json.push(',');
        }
        let _ = write!(json, "{{\"$file\":{index}}}");
    }
    let _ = write!(json, "],\"flags\":{flags}}}");
    let mut record = vec![14];
    super::put_string(&mut record, &json);
    record.extend(0u32.to_le_bytes());
    record.extend((count as u32).to_le_bytes());
    record
}

impl Folder {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let handle = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = handle.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self(Mutex::new(handle)))
    }

    pub(super) fn list(&self, limit: usize, stop: &AtomicBool) -> io::Result<(Vec<Entry>, u8)> {
        let handle = self.0.lock().unwrap();
        let (names, mut issues) = candidates(&handle, limit, stop)?;
        let mut files = Vec::new();
        for (name, id) in names {
            check_cancelled(stop)?;
            match open_child(&handle, name, id) {
                Ok(file) => files.push(file),
                Err(_) => issues |= 2,
            }
        }
        Ok((files, issues))
    }
}

type Names = BTreeMap<Vec<u16>, u64>;

fn candidates(folder: &File, limit: usize, stop: &AtomicBool) -> io::Result<(Names, u8)> {
    let mut buffer = [0u64; 4096];
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut names = Names::new();
    let mut scanned = 0;
    let mut issues = 0;
    'scan: loop {
        check_cancelled(stop)?;
        buffer.fill(0);
        // The buffer is eight-byte aligned for FILE_ID_BOTH_DIR_INFO.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                folder.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer) as u32,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(error);
        }
        class = FileIdBothDirectoryInfo;
        let bytes = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size_of_val(&buffer))
        };
        let mut remaining = bytes;
        loop {
            let item = directory_item(remaining)?;
            scanned += 1;
            if scanned > MAX_SCANNED {
                issues |= 1;
                break 'scan;
            }
            if item.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                issues |= 2;
            } else if item.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                names.insert(item.name, item.id);
                if names.len() > limit {
                    names.pop_last();
                    issues |= 1;
                }
            }
            if item.next == 0 {
                break;
            }
            remaining = &remaining[item.next..];
        }
    }
    Ok((names, issues))
}

struct DirectoryItem {
    next: usize,
    attributes: u32,
    id: u64,
    name: Vec<u16>,
}

fn directory_item(bytes: &[u8]) -> io::Result<DirectoryItem> {
    type Info = FILE_ID_BOTH_DIR_INFO;
    const NAME: usize = offset_of!(Info, FileName);
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let header = bytes.get(..NAME).ok_or_else(invalid)?;
    let word = |at| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    let length = word(offset_of!(Info, FileNameLength)) as usize;
    let next = word(offset_of!(Info, NextEntryOffset)) as usize;
    if length == 0
        || !length.is_multiple_of(2)
        || (next != 0 && !(NAME + length..bytes.len()).contains(&next))
    {
        return Err(invalid());
    }
    let name: Vec<u16> = bytes
        .get(NAME..NAME + length)
        .ok_or_else(invalid)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let id_at = offset_of!(Info, FileId);
    Ok(DirectoryItem {
        next,
        name,
        attributes: word(offset_of!(Info, FileAttributes)),
        id: u64::from_le_bytes(header[id_at..id_at + 8].try_into().unwrap()),
    })
}

fn open_child(folder: &File, mut name: Vec<u16>, expected_id: u64) -> io::Result<Entry> {
    use std::path::Component;
    let leaf = OsString::from_wide(&name);
    let mut parts = Path::new(&leaf).components();
    // The native open is relative to the retained directory. Reject stream syntax too.
    if !matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) || name.iter().any(|c| matches!(c, 0 | 47 | 58 | 92))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let length = u16::try_from(name.len() * 2).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: folder.as_raw_handle(),
        ObjectName: &mut object_name,
        ..Default::default()
    };
    let mut handle = std::ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // The root handle keeps this operation scoped even if the folder is renamed.
    // Opening the reparse point itself lets us reject it before exposing a file.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ,
            &attributes,
            &mut status,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if result < 0 {
        let code = unsafe { RtlNtStatusToDosError(result) };
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let id = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || (expected_id != 0 && id != expected_id)
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    Ok(Entry::folder_file(PathBuf::from(leaf), file, size))
}

fn check_cancelled(stop: &AtomicBool) -> io::Result<()> {
    if stop.load(Ordering::Relaxed) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    Ok(())
}

pub(super) fn pick(owner: HWND) -> windows::core::Result<Option<PathBuf>> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
        SIGDN_FILESYSPATH,
    };

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    // This call and the dialog run on the owning window's thread.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = Apartment;
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(
            dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR,
        )?;
        if let Err(error) = dialog.Show(Some(windows::Win32::Foundation::HWND(owner))) {
            return if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let owned = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0.cast()));
        Ok(Some(PathBuf::from(owned)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_directory_records() {
        let name = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let length = offset_of!(FILE_ID_BOTH_DIR_INFO, FileNameLength);
        let mut record = vec![0u8; name + 2];
        assert!(directory_item(&record).is_err());
        record[length..length + 4].copy_from_slice(&2u32.to_le_bytes());
        record[name..].copy_from_slice(&u16::from(b'x').to_le_bytes());
        assert_eq!(directory_item(&record).unwrap().name, [u16::from(b'x')]);
        record[..4].copy_from_slice(&1u32.to_le_bytes());
        assert!(directory_item(&record).is_err());
    }

    #[test]
    fn folder_files_keep_their_identity_after_names_are_reused() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let root = Scratch(std::env::temp_dir().join(format!(
            "roc-folder-test-{}-{}",
            std::process::id(),
            files::next_token()
        )));
        let selected = root.0.join("selected");
        let moved = root.0.join("moved");
        std::fs::create_dir_all(selected.join("nested")).unwrap();
        std::fs::write(selected.join("a.txt"), "chosen").unwrap();
        std::fs::write(selected.join("b.txt"), "second").unwrap();
        std::fs::write(selected.join("nested").join("inside.txt"), "nested").unwrap();

        let folder = Folder::open(&selected).unwrap();
        std::fs::rename(&selected, &moved).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("a.txt"), "replacement").unwrap();

        let (items, flags) = folder.list(1, &AtomicBool::new(false)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(flags, 1);
        std::fs::rename(moved.join("a.txt"), moved.join("renamed.txt")).unwrap();
        std::fs::write(moved.join("a.txt"), "also a replacement").unwrap();
        assert_eq!(items[0].read_range(0, 6).unwrap(), b"chosen");
        assert!(folder.list(16, &AtomicBool::new(true)).is_err());
    }
}
