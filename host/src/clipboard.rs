//! Clipboard request framing and plain text conversion.

use crate::{Result, wire::Reader};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Plain,
    Rich,
}

pub(crate) fn request<'a>(reader: &mut Reader<'a>) -> Result<(&'a str, Format)> {
    let text = reader.string()?;
    let format = match reader.u8()? {
        0 => Format::Plain,
        1 => {
            reader.string()?;
            Format::Rich
        }
        mode => return Err(format!("unknown clipboard format {mode}")),
    };
    Ok((text, format))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CopyRefusal {
    InvalidText,
    TooLarge,
}

pub(crate) fn validate_copy(text: &str) -> std::result::Result<(), CopyRefusal> {
    // Count UTF-16, including the carriage returns added for lone LFs and
    // the terminator, before the clipboard or simulator receives the text.
    let mut size = 1;
    let mut previous = 0;
    for unit in text.encode_utf16() {
        if unit == 0 {
            return Err(CopyRefusal::InvalidText);
        }
        size += if unit == 10 && previous != 13 { 2 } else { 1 };
        if size > 32_768 {
            return Err(CopyRefusal::TooLarge);
        }
        previous = unit;
    }
    Ok(())
}

fn utf16(text: &str) -> Option<Vec<u16>> {
    if text.contains('\0') {
        return None;
    }
    let mut units = Vec::new();
    let mut previous = 0;
    for unit in text.encode_utf16() {
        if unit == 10 && previous != 13 {
            units.push(13);
        }
        units.push(unit);
        previous = unit;
    }
    units.push(0);
    Some(units)
}

#[cfg(windows)]
pub(crate) fn write(window: windows_sys::Win32::Foundation::HWND, text: &str) -> Option<()> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    let units = utf16(text)?;
    let size = units.len().checked_mul(std::mem::size_of::<u16>())?;
    // SAFETY: Called on the owner window's UI thread. The buffer is filled
    // while locked, then either transferred to Windows or freed here.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, size);
        if memory.is_null() {
            return None;
        }
        let buffer = GlobalLock(memory).cast::<u16>();
        if buffer.is_null() {
            GlobalFree(memory);
            return None;
        }
        buffer.copy_from_nonoverlapping(units.as_ptr(), units.len());
        GlobalUnlock(memory);
        if OpenClipboard(window) == 0 {
            GlobalFree(memory);
            return None;
        }
        let transferred =
            EmptyClipboard() != 0 && !SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null();
        CloseClipboard();
        if !transferred {
            GlobalFree(memory);
        }
        transferred.then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_the_full_clipboard_request() {
        let mut plain = Reader::new(b"\x01\0\0\0a\x00\x55");
        assert_eq!(request(&mut plain).unwrap(), ("a", Format::Plain));
        assert_eq!(plain.u8().unwrap(), 0x55);
        let mut rich = Reader::new(b"\x01\0\0\0a\x01\x03\0\0\0<b>\x55");
        assert_eq!(request(&mut rich).unwrap(), ("a", Format::Rich));
        assert_eq!(rich.u8().unwrap(), 0x55);
        assert!(request(&mut Reader::new(b"\x01\0\0\0a\x01")).is_err());
        assert!(request(&mut Reader::new(b"\x01\0\0\0a\x02")).is_err());
    }

    #[test]
    fn native_copy_checks_the_normalized_size_and_rejects_nul() {
        assert!(validate_copy(&"a".repeat(32_767)).is_ok());
        assert_eq!(
            validate_copy(&"a".repeat(32_768)),
            Err(CopyRefusal::TooLarge)
        );
        assert_eq!(
            validate_copy(&"\n".repeat(16_384)),
            Err(CopyRefusal::TooLarge)
        );
        assert_eq!(validate_copy("a\0b"), Err(CopyRefusal::InvalidText));
    }

    #[test]
    fn preserves_unicode_and_tabs_with_windows_line_endings() {
        let expected: Vec<_> = "café\t🦀\r\nnext\r\nend\0".encode_utf16().collect();
        assert_eq!(utf16("café\t🦀\nnext\r\nend"), Some(expected));
        assert!(utf16("a\0b").is_none());
    }
}
