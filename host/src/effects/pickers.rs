//! Bind Common Item Dialog choices to file handles before returning to Roc.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use windows::Win32::Foundation::{HWND as ComWindow, S_FALSE};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::UI::Shell::{
    FDE_OVERWRITE_RESPONSE, FDE_SHAREVIOLATION_RESPONSE, FDEOR_DEFAULT, FDESVR_DEFAULT,
    FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
    FOS_NODEREFERENCELINKS, FOS_NOTESTFILECREATE, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS,
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileDialogEvents, IFileDialogEvents_Impl,
    IFileOpenDialog, IShellItem, SIGDN_FILESYSPATH,
};
use windows::core::{Error, HRESULT, Interface, PCWSTR, Ref, implement};
use windows_sys::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_YESNO, MessageBoxW,
};

use super::files::{Entry, MAX_HANDLES, Selection, remaining_handles};
use crate::rooted::{Directory, single_link};

pub(super) fn open(owner: HWND, multiple: bool) -> crate::Result<Option<Selection>> {
    let flags = if multiple {
        FOS_ALLOWMULTISELECT
    } else {
        FILEOPENDIALOGOPTIONS::default()
    };
    match show_open(owner, Kind::Open, flags)? {
        Some(Choice::Open(selection)) => Ok(Some(selection)),
        None => Ok(None),
        _ => unreachable!("an Open dialog always binds an open selection"),
    }
}

pub(super) enum FolderChoice {
    Chosen(Entry),
    Refused,
    Canceled,
}

pub(super) fn folder(owner: HWND) -> FolderChoice {
    match show_open(owner, Kind::Folder, FOS_PICKFOLDERS) {
        Ok(Some(Choice::Folder(Some(entry)))) => FolderChoice::Chosen(entry),
        Ok(Some(Choice::Folder(None))) | Err(_) => FolderChoice::Refused,
        Ok(None) => FolderChoice::Canceled,
        _ => unreachable!("a folder dialog always binds a folder selection"),
    }
}

fn show_open(
    owner: HWND,
    kind: Kind,
    flags: FILEOPENDIALOGOPTIONS,
) -> crate::Result<Option<Choice>> {
    let _apartment = Apartment::enter().map_err(dialog_error)?;
    // SAFETY: The owning UI thread has entered its COM apartment.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).map_err(dialog_error)?;
        let options = dialog.GetOptions().map_err(dialog_error)? | COMMON_OPTIONS | flags;
        dialog.SetOptions(options).map_err(dialog_error)?;
        show(owner, &dialog, kind)
    }
}

pub(super) fn save(owner: HWND, suggested_name: &str) -> crate::Result<Option<Destination>> {
    let _apartment = Apartment::enter().map_err(dialog_error)?;
    // Keep the same suggested-name limit as the previous Win32 dialog.
    let name = suggested_name
        .encode_utf16()
        .take_while(|&unit| unit != 0)
        .take(260)
        .chain([0])
        .collect::<Vec<_>>();
    // SAFETY: COM is initialized and the name remains live through SetFileName.
    let result = unsafe {
        let dialog: IFileDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).map_err(dialog_error)?;
        let options = dialog.GetOptions().map_err(dialog_error)?;
        dialog
            .SetOptions(
                (options | COMMON_OPTIONS | FOS_NOTESTFILECREATE | FOS_NODEREFERENCELINKS)
                    & !FOS_OVERWRITEPROMPT,
            )
            .map_err(dialog_error)?;
        dialog
            .SetFileName(PCWSTR(name.as_ptr()))
            .map_err(dialog_error)?;
        show(owner, &dialog, Kind::Save)
    }?;
    match result {
        Some(Choice::Save(destination)) => Ok(Some(destination)),
        None => Ok(None),
        _ => unreachable!("a Save As dialog always binds an export destination"),
    }
}

const COMMON_OPTIONS: FILEOPENDIALOGOPTIONS =
    FILEOPENDIALOGOPTIONS(FOS_FORCEFILESYSTEM.0 | FOS_NOCHANGEDIR.0);

struct Apartment;

impl Apartment {
    fn enter() -> windows::core::Result<Self> {
        // SAFETY: Balanced by Drop on the same UI thread, including S_FALSE
        // when the existing apartment already uses this threading model.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: This guard owns one successful CoInitializeEx call.
        unsafe { CoUninitialize() };
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Open,
    Folder,
    Save,
}

enum Choice {
    Open(Selection),
    Folder(Option<Entry>),
    Save(Destination),
}

fn show(owner: HWND, dialog: &IFileDialog, kind: Kind) -> crate::Result<Option<Choice>> {
    let selection = Rc::new(RefCell::new(None));
    let events: IFileDialogEvents = DialogEvents {
        owner,
        kind,
        selection: Rc::clone(&selection),
    }
    .into();
    // SAFETY: The dialog and sink live through Show. Unadvise runs both on
    // success and when the user cancels or the dialog fails.
    let shown = unsafe {
        let cookie = dialog.Advise(&events).map_err(dialog_error)?;
        let shown = dialog.Show(Some(ComWindow(owner)));
        let unadvised = dialog.Unadvise(cookie);
        if shown.is_ok() {
            unadvised.map_err(dialog_error)?;
        }
        shown
    };
    match shown {
        Ok(()) => selection
            .borrow_mut()
            .take()
            .map(Some)
            .ok_or_else(|| "Windows dialog returned without confirming a file choice".into()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED) => Ok(None),
        Err(error) => Err(dialog_error(error)),
    }
}

fn bound_open(dialog: &IFileDialog) -> windows::core::Result<Selection> {
    // SAFETY: The Shell documents GetResults during OnFileOk.
    unsafe {
        let result = dialog.cast::<IFileOpenDialog>()?.GetResults()?;
        let requested = result.GetCount()?;
        let available = remaining_handles();
        let entries = (0..requested.min(MAX_HANDLES as u32))
            .filter_map(|index| {
                let path = shell_path(&result.GetItemAt(index).ok()?).ok()?;
                Entry::open_picked(&path).ok()
            })
            .take(available)
            .collect();
        Ok(Selection { requested, entries })
    }
}

fn bound_folder(dialog: &IFileDialog) -> Option<Entry> {
    // SAFETY: The Shell documents GetResult during OnFileOk.
    let item = unsafe { dialog.GetResult() }.ok()?;
    let path = shell_path(&item).ok()?;
    Entry::for_folder(path).ok()
}

pub(super) struct Destination {
    parent: Directory,
    parent_path: PathBuf,
    target: Target,
}

enum Target {
    Existing(File),
    New(OsString),
}

impl Destination {
    fn bind(path: &Path) -> io::Result<Self> {
        let parent_path = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let leaf = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let parent = Directory::open(parent_path)?;
        let target = match parent.open_for_export(leaf) {
            Ok(file) => {
                single_link(&file)?;
                Target::Existing(file)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Target::New(leaf.to_owned()),
            Err(error) => return Err(error),
        };
        Ok(Self {
            parent,
            parent_path: parent_path.to_owned(),
            target,
        })
    }

    pub(super) fn open(&self) -> io::Result<File> {
        // This comparison is only an extra refusal. Opening a new file still
        // uses the held parent, never the directory reached by this lookup.
        if Directory::open(&self.parent_path)?.id()? != self.parent.id()? {
            return Err(io::Error::other(
                "the destination folder changed; choose Save As again to confirm it",
            ));
        }
        match &self.target {
            Target::Existing(file) => {
                single_link(file)?;
                file.try_clone()
            }
            Target::New(leaf) => self.parent.create_file(leaf).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    io::Error::new(
                        error.kind(),
                        "a file appeared at this destination; choose Save As again to confirm it",
                    )
                } else {
                    error
                }
            }),
        }
    }
}

fn bound_save(dialog: &IFileDialog, owner: HWND) -> crate::Result<Option<Destination>> {
    // SAFETY: The Shell documents GetResult during OnFileOk.
    let path =
        shell_path(&unsafe { dialog.GetResult() }.map_err(dialog_error)?).map_err(dialog_error)?;
    let choice = Destination::bind(&path).map_err(|error| format!("Cannot save here: {error}"))?;
    if matches!(choice.target, Target::Existing(_)) {
        let name = path.file_name().unwrap().to_string_lossy();
        let accepted = message_box(
            owner,
            "Confirm Save As",
            &format!("A file named “{name}” already exists. Do you want to replace its contents?"),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        );
        if accepted != IDYES {
            return Ok(None);
        }
    }
    Ok(Some(choice))
}

fn shell_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: The Shell allocates the display name in COM task memory. Copy
    // the complete UTF-16 path before freeing the allocation.
    unsafe {
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(OsString::from_wide(raw.as_wide()));
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(path)
    }
}

fn dialog_owner(dialog: &IFileDialog, fallback: HWND) -> HWND {
    // SAFETY: GetWindow borrows a handle owned by the live Shell dialog.
    let window = dialog
        .cast::<IOleWindow>()
        .and_then(|window| unsafe { window.GetWindow() });
    window.map_or(fallback, |window| window.0)
}

fn dialog_error(error: Error) -> String {
    format!("Windows file dialog failed: {error}")
}

fn message_box(owner: HWND, title: &str, text: &str, style: u32) -> i32 {
    let title = title.encode_utf16().chain([0]).collect::<Vec<_>>();
    let text = text.encode_utf16().chain([0]).collect::<Vec<_>>();
    // SAFETY: Both UTF-16 strings stay live through the owned modal dialog.
    unsafe { MessageBoxW(owner, text.as_ptr(), title.as_ptr(), style) }
}

pub(super) fn save_error(owner: HWND, error: &str) {
    message_box(owner, "Save As", error, MB_OK | MB_ICONERROR);
}

#[implement(IFileDialogEvents)]
struct DialogEvents {
    owner: HWND,
    kind: Kind,
    selection: Rc<RefCell<Option<Choice>>>,
}

#[allow(non_snake_case)]
impl IFileDialogEvents_Impl for DialogEvents_Impl {
    fn OnFileOk(&self, dialog: Ref<IFileDialog>) -> windows::core::Result<()> {
        self.selection.borrow_mut().take();
        let dialog = dialog.ok()?;
        let owner = dialog_owner(dialog, self.owner);
        let result = match self.kind {
            Kind::Open => bound_open(dialog)
                .map(Choice::Open)
                .map(Some)
                .map_err(dialog_error),
            Kind::Folder => Ok(Some(Choice::Folder(bound_folder(dialog)))),
            Kind::Save => bound_save(dialog, owner).map(|choice| choice.map(Choice::Save)),
        };
        match result {
            Ok(Some(choice)) => {
                self.selection.replace(Some(choice));
                Ok(())
            }
            Ok(None) => Err(Error::from_hresult(S_FALSE)),
            Err(error) => {
                match self.kind {
                    Kind::Open => {
                        message_box(owner, "Open", &error, MB_OK | MB_ICONERROR);
                    }
                    Kind::Save => save_error(owner, &error),
                    Kind::Folder => unreachable!("a folder refusal is delivered to the app"),
                }
                Err(Error::from_hresult(S_FALSE))
            }
        }
    }

    fn OnFolderChanging(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnFolderChange(&self, _: Ref<IFileDialog>) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnSelectionChange(&self, _: Ref<IFileDialog>) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnShareViolation(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<FDE_SHAREVIOLATION_RESPONSE> {
        Ok(FDESVR_DEFAULT)
    }

    fn OnTypeChange(&self, _: Ref<IFileDialog>) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnOverwrite(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<FDE_OVERWRITE_RESPONSE> {
        Ok(FDEOR_DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "roc-picker-tests-{}-{}",
                std::process::id(),
                super::super::files::next_token()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn confirming_open_keeps_the_object_after_its_name_is_reused() {
        let scratch = Scratch::new();
        let path = scratch.0.join("chosen.txt");
        std::fs::write(&path, b"chosen").unwrap();
        let entry = Entry::open_picked(&path).unwrap();
        std::fs::rename(&path, scratch.0.join("moved.txt")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();

        assert_eq!(entry.read_range(0, 6).unwrap(), b"chosen");
    }

    #[test]
    fn declining_overwrite_or_cancelling_save_leaves_no_changes() {
        let scratch = Scratch::new();
        let existing = scratch.0.join("existing.txt");
        let missing = scratch.0.join("missing.txt");
        std::fs::write(&existing, b"existing").unwrap();
        drop(Destination::bind(&existing).unwrap());
        drop(Destination::bind(&missing).unwrap());
        assert_eq!(std::fs::read(existing).unwrap(), b"existing");
        assert!(!missing.exists());
    }

    #[test]
    fn overwrite_uses_the_file_held_before_confirmation() {
        use std::io::Write;

        let scratch = Scratch::new();
        let path = scratch.0.join("chosen.txt");
        std::fs::write(&path, b"chosen").unwrap();
        let target = Destination::bind(&path).unwrap();
        assert!(std::fs::rename(&path, scratch.0.join("other.txt")).is_err());
        assert!(std::fs::write(&path, b"unconfirmed").is_err());

        let mut file = target.open().unwrap();
        file.set_len(0).unwrap();
        file.write_all(b"exported").unwrap();
        drop(file);
        drop(target);
        assert_eq!(std::fs::read(path).unwrap(), b"exported");
    }

    #[test]
    fn new_destination_uses_exclusive_create_and_leaves_a_late_arrival_intact() {
        use std::io::Write;

        let scratch = Scratch::new();
        let collision = scratch.0.join("late.txt");
        let target = Destination::bind(&collision).unwrap();
        assert!(!collision.exists());
        std::fs::write(&collision, b"late arrival").unwrap();
        assert_eq!(
            target.open().unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&collision).unwrap(), b"late arrival");

        let fresh = scratch.0.join("fresh.txt");
        let target = Destination::bind(&fresh).unwrap();
        target.open().unwrap().write_all(b"exported").unwrap();
        drop(target);
        assert_eq!(std::fs::read(&fresh).unwrap(), b"exported");
    }

    #[test]
    fn new_destination_refuses_a_replaced_parent() {
        let scratch = Scratch::new();
        let earlier = scratch.0.join("selected");
        let moved = scratch.0.join("previous");
        std::fs::create_dir(&earlier).unwrap();
        let target = Destination::bind(&earlier.join("new.txt")).unwrap();
        std::fs::rename(&earlier, &moved).unwrap();
        std::fs::create_dir(&earlier).unwrap();
        assert!(target.open().is_err());
        assert!(!moved.join("new.txt").exists());
        assert!(!earlier.join("new.txt").exists());
    }

    #[test]
    fn refuses_existing_hardlinks_and_ambiguous_leaf_names() {
        let scratch = Scratch::new();
        let real = scratch.0.join("real.txt");
        let other = scratch.0.join("hard-link.txt");
        std::fs::write(&real, b"original").unwrap();
        std::fs::hard_link(&real, &other).unwrap();

        assert!(Destination::bind(&other).is_err());
        for name in ["real.txt:stream", "NUL.txt", "file.txt.", "file.txt "] {
            assert!(Destination::bind(&scratch.0.join(name)).is_err(), "{name}");
        }
        assert_eq!(std::fs::read(real).unwrap(), b"original");
    }

    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn common_item_dialog_confirms_and_cancels_through_the_shell() {
        use std::io::Write;
        use std::os::windows::ffi::OsStrExt;
        use std::time::{Duration, Instant};
        use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
        use windows::core::w;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, IDCANCEL, IDNO, IDOK, IsWindow, PostMessageW, WM_CLOSE, WM_COMMAND,
        };

        fn wait_for_window(title: PCWSTR) -> HWND {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                // SAFETY: Title points at a static NUL-terminated string.
                let window = unsafe { FindWindowW(std::ptr::null(), title.0) };
                if !window.is_null() || Instant::now() >= deadline {
                    return window;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn press(title: &'static [u16], command: i32, followup: Option<i32>) {
            let title = PCWSTR(title.as_ptr());
            let window = wait_for_window(title);
            assert!(!window.is_null(), "common item dialog did not appear");
            // SAFETY: Only standard command IDs are posted to the test dialog.
            assert_ne!(
                unsafe { PostMessageW(window, WM_COMMAND, command as usize, 0) },
                0
            );
            if let Some(answer) = followup {
                let confirm = wait_for_window(w!("Confirm Save As"));
                if confirm.is_null() {
                    // SAFETY: Close the test dialog so failed automation cannot hang Show.
                    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
                    panic!("owned overwrite confirmation did not appear");
                }
                // SAFETY: The confirm window belongs to this test's Save As flow.
                assert_ne!(
                    unsafe { PostMessageW(confirm, WM_COMMAND, answer as usize, 0) },
                    0
                );
                if answer == IDNO {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    // SAFETY: IsWindow only checks the handle obtained above.
                    while unsafe { IsWindow(confirm) } != 0 && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    // SAFETY: No should leave the original Save As dialog open.
                    assert_ne!(
                        unsafe { PostMessageW(window, WM_COMMAND, IDCANCEL as usize, 0) },
                        0
                    );
                }
            }
        }

        const TITLE: PCWSTR = w!("Roc native picker contract");
        let title = unsafe { TITLE.as_wide() };
        let owner = std::ptr::null_mut();
        let scratch = Scratch::new();
        let file = scratch.0.join("chosen.txt");
        std::fs::write(&file, b"chosen").unwrap();
        let directory = scratch
            .0
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();

        let _apartment = Apartment::enter().unwrap();
        // SAFETY: The test owns the COM apartment and all string parameters.
        unsafe {
            let folder: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(directory.as_ptr()), None).unwrap();
            let native: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).unwrap();
            native
                .SetOptions(native.GetOptions().unwrap() | COMMON_OPTIONS)
                .unwrap();
            native.SetTitle(TITLE).unwrap();
            native.SetFolder(&folder).unwrap();
            native.SetFileName(w!("chosen.txt")).unwrap();
            let click = std::thread::spawn(move || press(title, IDOK, None));
            let choice = show(owner, &native, Kind::Open).unwrap();
            click.join().unwrap();
            let Some(Choice::Open(opened)) = choice else {
                panic!("Open did not return the confirmed selection");
            };
            assert_eq!(opened.requested, 1);
            assert_eq!(opened.entries[0].read_range(0, 6).unwrap(), b"chosen");
            drop(opened);

            let selected = scratch.0.join("selected-folder");
            let moved = scratch.0.join("moved-folder");
            std::fs::create_dir(&selected).unwrap();
            std::fs::write(selected.join("proof.txt"), b"confirmed").unwrap();
            let native: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).unwrap();
            native
                .SetOptions(native.GetOptions().unwrap() | COMMON_OPTIONS | FOS_PICKFOLDERS)
                .unwrap();
            const FOLDER_TITLE: PCWSTR = w!("Roc native folder picker contract");
            let folder_title = FOLDER_TITLE.as_wide();
            native.SetTitle(FOLDER_TITLE).unwrap();
            native.SetFolder(&folder).unwrap();
            native.SetFileName(w!("selected-folder")).unwrap();
            let click = std::thread::spawn(move || press(folder_title, IDCANCEL, None));
            assert!(show(owner, &native, Kind::Folder).unwrap().is_none());
            click.join().unwrap();

            let click = std::thread::spawn(move || press(folder_title, IDOK, None));
            let choice = show(owner, &native, Kind::Folder).unwrap();
            click.join().unwrap();
            let Some(Choice::Folder(Some(entry))) = choice else {
                panic!("Folder picker did not retain the confirmed directory");
            };
            drop(native);
            std::fs::rename(&selected, &moved).unwrap();
            std::fs::create_dir(&selected).unwrap();
            std::fs::write(selected.join("proof.txt"), b"substitute").unwrap();
            // Bind the app token only after the confirmed path has been reused.
            let (record, tokens) = super::super::files::folder_record(Some(entry));
            assert_eq!(&record[..8], &[1, 0, 0, 0, 0, 0, 0, 0]);
            let entry = super::super::files::resolve(tokens[0]).unwrap();
            super::super::files::discard_entries(tokens);
            let (files, flags) = entry
                .folder
                .unwrap()
                .list(16, &std::sync::atomic::AtomicBool::new(false))
                .unwrap();
            assert_eq!(flags, 0);
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].read_range(0, 9).unwrap(), b"confirmed");

            let native: IFileDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).unwrap();
            native
                .SetOptions(COMMON_OPTIONS | FOS_NOTESTFILECREATE)
                .unwrap();
            native.SetTitle(TITLE).unwrap();
            native.SetFolder(&folder).unwrap();
            native.SetFileName(w!("chosen.txt")).unwrap();
            let click = std::thread::spawn(move || press(title, IDCANCEL, None));
            assert!(show(owner, &native, Kind::Save).unwrap().is_none());
            click.join().unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), b"chosen");

            let click = std::thread::spawn(move || press(title, IDOK, Some(IDNO)));
            assert!(show(owner, &native, Kind::Save).unwrap().is_none());
            click.join().unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), b"chosen");

            let click = std::thread::spawn(move || press(title, IDOK, Some(IDYES)));
            let choice = show(owner, &native, Kind::Save).unwrap();
            click.join().unwrap();
            let Some(Choice::Save(destination)) = choice else {
                panic!("Save As did not return the confirmed destination");
            };
            let mut output = destination.open().unwrap();
            output.set_len(0).unwrap();
            output.write_all(b"updated").unwrap();
            drop(output);
            drop(destination);
            assert_eq!(std::fs::read(file).unwrap(), b"updated");
            native.SetFileName(w!("new.txt")).unwrap();
            let click = std::thread::spawn(move || press(title, IDOK, None));
            let choice = show(owner, &native, Kind::Save).unwrap();
            click.join().unwrap();
            let Some(Choice::Save(destination)) = choice else {
                panic!("Save As did not return the confirmed destination");
            };
            let new_file = scratch.0.join("new.txt");
            assert!(!new_file.exists());
            destination.open().unwrap().write_all(b"new").unwrap();
            assert_eq!(std::fs::read(new_file).unwrap(), b"new");
        }
    }

    #[test]
    fn native_callback_preserves_the_hresult_that_refuses_a_choice() {
        let error = Error::from_hresult(S_FALSE);
        let result: windows::core::Result<()> = Err(error);
        assert_eq!(HRESULT::from(result), S_FALSE);
    }
}
