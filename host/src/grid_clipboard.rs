use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

const LIMIT_BYTES: usize = 65_536;

pub(crate) enum ClipboardText {
    Text(String),
    Oversized,
    Unavailable,
}

impl ClipboardText {
    pub(crate) fn from_text(text: &str) -> Self {
        if text.encode_utf16().count() >= LIMIT_BYTES / 2 {
            Self::Oversized
        } else if text.contains('\0') {
            Self::Unavailable
        } else {
            Self::Text(text.into())
        }
    }
}

pub(crate) fn read(hwnd: HWND) -> ClipboardText {
    // SAFETY: The owner HWND is on this UI thread. Closing the clipboard
    // and releasing the allocation happen before this method returns.
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return ClipboardText::Unavailable;
        }
        let clipboard = Clipboard;
        let memory = GetClipboardData(u32::from(CF_UNICODETEXT));
        if memory.is_null() {
            return ClipboardText::Unavailable;
        }
        let bytes = GlobalSize(memory);
        if bytes > LIMIT_BYTES {
            return ClipboardText::Oversized;
        }
        if bytes < 2 || !bytes.is_multiple_of(2) {
            return ClipboardText::Unavailable;
        }
        let buffer = GlobalLock(memory).cast::<u16>();
        if buffer.is_null() {
            return ClipboardText::Unavailable;
        }
        let text = std::slice::from_raw_parts(buffer, bytes / 2);
        let owned = text
            .iter()
            .position(|&value| value == 0)
            .and_then(|end| String::from_utf16(&text[..end]).ok());
        GlobalUnlock(memory);
        drop(clipboard);
        owned.map_or(ClipboardText::Unavailable, ClipboardText::Text)
    }
}

struct Clipboard;

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: A successful OpenClipboard created this UI-thread guard.
        unsafe { CloseClipboard() };
    }
}
