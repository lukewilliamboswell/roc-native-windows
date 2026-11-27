//! Open Windows assets and directories relative to held filesystem objects.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, Prefix};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION, FILE_NON_DIRECTORY_FILE,
    FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_SYNCHRONOUS_IO_NONALERT, FileDispositionInformation, FileRenameInformation, NtCreateFile,
    NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, UNICODE_STRING};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK,
    GetFileType, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

const NAME_LIMIT: usize = 255;
const DIRECTORY_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const SHARE_FILE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const SHARE_ALL: u32 = SHARE_FILE | FILE_SHARE_DELETE;

pub(crate) struct Directory(File);

impl Directory {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return Err(invalid_name());
        };
        let drive = match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => char::from(drive),
            _ => return Err(invalid_name()),
        };
        if components.next() != Some(Component::RootDir) {
            return Err(invalid_name());
        }
        let root = OpenOptions::new()
            .access_mode(DIRECTORY_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(format!("{drive}:\\"))?;
        checked(&root, true)?;
        let mut directory = Self(root);
        for part in components {
            let Component::Normal(name) = part else {
                return Err(invalid_name());
            };
            directory = directory.subdir(name)?;
        }
        Ok(directory)
    }

    pub(crate) fn id(&self) -> io::Result<Identity> {
        identity(&self.0)
    }

    pub(crate) fn subdir(&self, name: &OsStr) -> io::Result<Self> {
        self.open_relative(name, true, DIRECTORY_ACCESS, SHARE_ALL, FILE_OPEN)
            .map(Self)
    }

    pub(crate) fn create_subdir(&self, name: &OsStr) -> io::Result<Self> {
        self.open_relative(name, true, DIRECTORY_ACCESS, SHARE_ALL, FILE_OPEN_IF)
            .map(Self)
    }

    pub(crate) fn read(&self, name: &OsStr) -> io::Result<File> {
        self.open_relative(name, false, FILE_GENERIC_READ, SHARE_ALL, FILE_OPEN)
    }

    pub(crate) fn open_for_export(&self, name: &OsStr) -> io::Result<File> {
        self.open_relative(name, false, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_OPEN)
    }

    pub(crate) fn create_file(&self, name: &OsStr) -> io::Result<File> {
        self.open_relative(
            name,
            false,
            FILE_GENERIC_WRITE | DELETE,
            FILE_SHARE_READ,
            FILE_CREATE,
        )
    }

    pub(crate) fn open_for_removal(&self, name: &OsStr) -> io::Result<File> {
        self.open_relative(
            name,
            false,
            DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE_FILE,
            FILE_OPEN,
        )
    }

    pub(crate) fn replace(&self, staged: &File, name: &OsStr) -> io::Result<()> {
        const HEADER: usize = std::mem::size_of::<FILE_RENAME_INFORMATION>();
        const NAME_OFFSET: usize = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
        const BUFFER: usize = NAME_LIMIT + (HEADER - NAME_OFFSET).div_ceil(2);
        #[repr(C)]
        struct Rename {
            replace: u32,
            root: windows_sys::Win32::Foundation::HANDLE,
            length: u32,
            name: [u16; BUFFER],
        }

        let name = checked_name(name)?;
        let mut request = Rename {
            replace: 1,
            root: self.0.as_raw_handle(),
            length: (name.len() * 2) as u32,
            name: [0; BUFFER],
        };
        request.name[..name.len()].copy_from_slice(&name);
        const _: () = assert!(NAME_OFFSET == std::mem::offset_of!(Rename, name));
        let length = (HEADER + name.len() * 2) as u32;
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: Both handles and the aligned rename buffer are live for this
        // synchronous call. The destination is one validated child of root.
        checked_status(unsafe {
            NtSetInformationFile(
                staged.as_raw_handle(),
                &mut status,
                std::ptr::from_ref(&request).cast(),
                length,
                FileRenameInformation,
            )
        })
    }

    fn open_relative(
        &self,
        name: &OsStr,
        is_dir: bool,
        access: u32,
        sharing: u32,
        disposition: u32,
    ) -> io::Result<File> {
        let mut name = checked_name(name)?;
        let length = (name.len() * 2) as u16;
        let mut object_name = UNICODE_STRING {
            Length: length,
            MaximumLength: length,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: self.0.as_raw_handle(),
            ObjectName: &mut object_name,
            Attributes: 0x40, // OBJ_CASE_INSENSITIVE
            ..Default::default()
        };
        let mut handle = std::ptr::null_mut();
        let mut status = IO_STATUS_BLOCK::default();
        let kind = if is_dir {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
        // SAFETY: The root, bounded object name, and output storage remain live
        // throughout this synchronous call. NT does not consume the root handle.
        let result = unsafe {
            NtCreateFile(
                &mut handle,
                access,
                &attributes,
                &mut status,
                std::ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                sharing,
                disposition,
                kind | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null(),
                0,
            )
        };
        checked_status(result)?;
        // SAFETY: Successful NtCreateFile transferred a unique handle.
        let file = unsafe { File::from_raw_handle(handle) };
        checked(&file, is_dir)?;
        Ok(file)
    }
}

pub(crate) fn single_link(file: &File) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
    };

    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: Query the live regular-file handle into a correctly sized record.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.NumberOfLinks != 1 || info.DeletePending {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file with one link",
        ));
    }
    Ok(())
}

pub(crate) fn remove(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: The file was opened for deletion, and the input and output
    // storage stay live for this synchronous call.
    checked_status(unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of_val(&disposition) as u32,
            FileDispositionInformation,
        )
    })
}

fn checked_status(status: i32) -> io::Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: Convert the NTSTATUS returned by the native file operation.
    Err(io::Error::from_raw_os_error(
        unsafe { RtlNtStatusToDosError(status) } as i32,
    ))
}

pub(crate) fn read_asset(root: &Path, name: &str) -> crate::Result<File> {
    let open = || {
        if name.len() > 32_767 {
            return Err(invalid_name());
        }
        let parts = name.split(['/', '\\']).collect::<Vec<_>>();
        for part in &parts {
            checked_name(OsStr::new(part))?;
        }
        let (file, folders) = parts.split_last().ok_or_else(invalid_name)?;
        let mut directory = Directory::open(root)?;
        for folder in folders {
            directory = directory.subdir(OsStr::new(folder))?;
        }
        directory.read(OsStr::new(file))
    };
    open().map_err(|err| format!("cannot open asset {name:?}: {err}"))
}

pub(crate) type Identity = (u64, [u8; 16]);

pub(crate) fn identity(file: &File) -> io::Result<Identity> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };

    let mut info = FILE_ID_INFO::default();
    // SAFETY: The file and output storage remain live throughout the query.
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

fn checked_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let units = name.encode_wide().take(NAME_LIMIT + 1).collect::<Vec<_>>();
    if !(1..=NAME_LIMIT).contains(&units.len())
        || matches!(units.last(), Some(32 | 46))
        || units
            .iter()
            .any(|&c| c < 32 || matches!(c, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
    {
        return Err(invalid_name());
    }
    let stem = units.split(|&u| u == u16::from(b'.')).next().unwrap();
    let stem = String::from_utf16_lossy(stem).to_ascii_uppercase();
    let stem = stem.trim_end_matches(' ');
    if matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
    {
        return Err(invalid_name());
    }
    Ok(units)
}

fn checked(file: &File, is_dir: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    // SAFETY: The metadata and file type are queried using the same held handle.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || if is_dir {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular filesystem object with no reparse point",
        ));
    }
    Ok(())
}

fn invalid_name() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "unsupported Windows filesystem path",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn relative_opens_stay_with_the_open_directory() {
        let fixture =
            Fixture(std::env::temp_dir().join(format!("roc-rooted-test-{}", std::process::id())));
        std::fs::create_dir(&fixture.0).unwrap();
        let original = fixture.0.join("assets");
        let renamed = fixture.0.join("renamed");
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("file.bin"), b"original").unwrap();
        let held = Directory::open(&original).unwrap();

        std::fs::rename(&original, &renamed).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("file.bin"), b"replacement").unwrap();
        let mut bytes = Vec::new();
        held.read(OsStr::new("file.bin"))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"original");
    }

    #[test]
    fn rejects_ambiguous_devices_and_non_leaf_names() {
        for name in [
            "",
            ".",
            "..",
            "CON.txt",
            "com¹.doc",
            "LPT2:foo",
            "foo/bar",
            "foo\\bar",
            "file.txt ",
            "file.",
        ] {
            assert!(checked_name(OsStr::new(name)).is_err(), "{name:?}");
        }
        assert!(checked_name(OsStr::new("café.png")).is_ok());
    }
}
