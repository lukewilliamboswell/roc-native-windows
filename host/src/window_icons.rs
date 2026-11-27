use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, IsWindow, LoadImageW, SM_CXICON,
    SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
};

pub(crate) struct WindowIcons {
    window: HWND,
    dpi: u32,
    large: Option<Icon>,
    small: Option<Icon>,
}

impl WindowIcons {
    pub(crate) fn new(window: HWND) -> Self {
        let mut icons = Self {
            window,
            dpi: 0,
            large: None,
            small: None,
        };
        icons.sync();
        icons
    }

    pub(crate) fn sync(&mut self) {
        // The window belongs to the calling thread.
        let reported = unsafe { GetDpiForWindow(self.window) };
        let dpi = if reported == 0 { 96 } else { reported };
        if self.dpi == dpi {
            return;
        }
        self.dpi = dpi;
        let large = Icon::load(dpi, SM_CXICON, SM_CYICON);
        let small = Icon::load(dpi, SM_CXSMICON, SM_CYSMICON);
        replace(self.window, ICON_BIG, &mut self.large, large);
        replace(self.window, ICON_SMALL, &mut self.small, small);
    }
}

impl Drop for WindowIcons {
    fn drop(&mut self) {
        if unsafe { IsWindow(self.window) } != 0 {
            replace(self.window, ICON_BIG, &mut self.large, None);
            replace(self.window, ICON_SMALL, &mut self.small, None);
        }
    }
}

struct Icon(HICON);

impl Icon {
    fn load(dpi: u32, width: i32, height: i32) -> Option<Self> {
        // Resource 1 is the application icon embedded by the launcher. No
        // LR_SHARED flag means the returned handle belongs to this window.
        let handle = unsafe {
            LoadImageW(
                GetModuleHandleW(null()),
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetricsForDpi(width, dpi),
                GetSystemMetricsForDpi(height, dpi),
                0,
            )
        };
        (!handle.is_null()).then(|| Self(handle))
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) };
    }
}

fn replace(window: HWND, size: u32, slot: &mut Option<Icon>, replacement: Option<Icon>) {
    if slot.is_none() && replacement.is_none() {
        return;
    }
    let handle = replacement.as_ref().map_or(null_mut(), |icon| icon.0);
    // Update the HWND before releasing the previous owned icon.
    unsafe { SendMessageW(window, WM_SETICON, size as usize, handle as isize) };
    *slot = replacement;
}
