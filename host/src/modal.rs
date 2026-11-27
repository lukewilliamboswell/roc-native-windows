//! Native modeless dialogs with an explicit owner block.

use crate::Result;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::UI::Controls::DTM_GETMONTHCAL;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetActiveWindow, GetFocus, IsWindowEnabled, SetActiveWindow, SetFocus, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

const NATIVE_PROC: PCWSTR = w!("RocWindows.NativeDialogProc");

/// A standard dialog template with no embedded menu, class override, title,
/// or child items. Roc mounts its controls on the returned native HWND.
#[repr(C, align(4))]
struct DialogTemplate {
    header: DLGTEMPLATE,
    menu: u16,
    class: u16,
    title: u16,
}

pub(crate) fn create(owner: HWND, route: WNDPROC, title: &str) -> Result<HWND> {
    let route = route.ok_or("native dialog is missing its message procedure")?;
    let template = DialogTemplate {
        header: DLGTEMPLATE {
            style: WS_POPUP
                | WS_CAPTION
                | WS_SYSMENU
                | WS_THICKFRAME
                | WS_CLIPCHILDREN
                | WS_VSCROLL
                | DS_MODALFRAME as u32,
            dwExtendedStyle: WS_EX_CONTROLPARENT | WS_EX_DLGMODALFRAME,
            x: 0,
            y: 0,
            cx: 240,
            cy: 140,
            cdit: 0,
        },
        menu: 0,
        class: 0,
        title: 0,
    };
    let title = crate::view::wide(title);
    // SAFETY: The aligned template and title live across the native calls,
    // and the owner and message procedure are on this UI thread.
    unsafe {
        let dialog =
            CreateDialogIndirectParamW(null_mut(), &template.header, owner, Some(dialog_proc), 0);
        if dialog.is_null() {
            return Err(last_error("create the owned dialog"));
        }
        let previous = GetWindowLongPtrW(dialog, GWLP_WNDPROC);
        if previous == 0 || SetPropW(dialog, NATIVE_PROC, previous as _) == 0 {
            let error = last_error("remember the native dialog procedure");
            DestroyWindow(dialog);
            return Err(error);
        }
        SetLastError(0);
        if SetWindowLongPtrW(dialog, GWLP_WNDPROC, route as usize as isize) == 0
            && GetLastError() != 0
        {
            let error = last_error("connect the native dialog message route");
            RemovePropW(dialog, NATIVE_PROC);
            DestroyWindow(dialog);
            return Err(error);
        }
        if SetWindowTextW(dialog, title.as_ptr()) == 0 {
            let error = last_error("set the native dialog title");
            DestroyWindow(dialog);
            return Err(error);
        }
        Ok(dialog)
    }
}

unsafe extern "system" fn dialog_proc(_: HWND, _: u32, _: WPARAM, _: LPARAM) -> isize {
    // The registry establishes focus after mounting the Roc controls.
    0
}

/// Send an unhandled message through the original Win32 dialog procedure.
pub(crate) unsafe fn dialog_default(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    // SAFETY: Only create sets this property, from the value returned by
    // GWLP_WNDPROC. CallWindowProc understands Win32's opaque proc values.
    unsafe {
        let previous = GetPropW(hwnd, NATIVE_PROC);
        if previous.is_null() {
            return None;
        }
        let native = std::mem::transmute::<HANDLE, WNDPROC>(previous);
        let result = CallWindowProcW(native, hwnd, message, wparam, lparam);
        if message == WM_NCDESTROY {
            RemovePropW(hwnd, NATIVE_PROC);
        }
        Some(result)
    }
}

pub(crate) fn position(dialog: HWND, owner: HWND, desired_client: (i32, i32)) -> Result<()> {
    // SAFETY: Both windows are live on the UI thread. The RECT and monitor
    // information are initialized writable copies.
    unsafe {
        let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err(last_error("read the dialog owner's monitor"));
        }
        let mut owner_bounds = info.rcWork;
        if IsIconic(owner) == 0 && GetWindowRect(owner, &mut owner_bounds) == 0 {
            return Err(last_error("measure the dialog owner"));
        }
        let mut frame = RECT {
            right: desired_client.0.max(1),
            bottom: desired_client.1.max(1),
            ..RECT::default()
        };
        if AdjustWindowRectExForDpi(
            &mut frame,
            GetWindowLongPtrW(dialog, GWL_STYLE) as u32,
            0,
            GetWindowLongPtrW(dialog, GWL_EXSTYLE) as u32,
            GetDpiForWindow(owner).max(96),
        ) == 0
        {
            return Err(last_error("measure the native dialog frame"));
        }
        let (x, y, width, height) = centered(
            owner_bounds,
            info.rcWork,
            (frame.right - frame.left, frame.bottom - frame.top),
        );
        if SetWindowPos(
            dialog,
            null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        ) == 0
        {
            return Err(last_error("place the owned dialog"));
        }
        Ok(())
    }
}

fn centered(owner: RECT, work: RECT, (width, height): (i32, i32)) -> (i32, i32, i32, i32) {
    let width = width.clamp(1, (work.right - work.left).max(1));
    let height = height.clamp(1, (work.bottom - work.top).max(1));
    let x = (owner.left + (owner.right - owner.left - width) / 2)
        .clamp(work.left, (work.right - width).max(work.left));
    let y = (owner.top + (owner.bottom - owner.top - height) / 2)
        .clamp(work.top, (work.bottom - height).max(work.top));
    (x, y, width, height)
}

pub(crate) struct Modal {
    owner: HWND,
    dialog: HWND,
    opener: HWND,
    activate_on_show: bool,
    restore_enabled: bool,
}

impl Modal {
    pub(crate) fn begin(owner: HWND, dialog: HWND) -> Self {
        // SAFETY: Owner and dialog belong to this UI thread. Snapshot their
        // state before EnableWindow can notify either window synchronously.
        unsafe {
            let foreground = GetForegroundWindow();
            let activate_on_show =
                foreground == owner || (foreground.is_null() && GetActiveWindow() == owner);
            let focus = GetFocus();
            let opener = if IsChild(owner, focus) != 0 {
                focus
            } else {
                null_mut()
            };
            let restore_enabled = IsWindowEnabled(owner) != 0;
            if restore_enabled {
                EnableWindow(owner, 0);
            }
            Self {
                owner,
                dialog,
                opener,
                activate_on_show,
                restore_enabled,
            }
        }
    }

    pub(crate) fn show(&self) {
        // SAFETY: The dialog is live and its children have been mounted.
        unsafe {
            if self.activate_on_show {
                ShowWindow(self.dialog, SW_SHOW);
                SetActiveWindow(self.dialog);
                if GetActiveWindow() == self.dialog {
                    focus_first(self.dialog);
                }
            } else {
                ShowWindow(self.dialog, SW_SHOWNOACTIVATE);
            }
        }
    }

    pub(crate) fn can_restore_focus(&self) -> bool {
        // SAFETY: Query the desktop's foreground window and this thread's active window.
        unsafe {
            let foreground = GetForegroundWindow();
            foreground == self.dialog || (foreground.is_null() && GetActiveWindow() == self.dialog)
        }
    }

    /// Reenable the owner before native destruction can choose a new active window.
    pub(crate) fn release_owner(&mut self) {
        if std::mem::take(&mut self.restore_enabled) {
            // SAFETY: The owner belongs to our thread and may already be gone.
            unsafe {
                if IsWindow(self.owner) != 0 {
                    EnableWindow(self.owner, 1);
                }
            }
        }
    }

    pub(crate) fn finish(mut self, restore_focus: bool) {
        self.release_owner();
        // SAFETY: Verify the owner and candidate opener again after layout
        // reconciliation, which may have disposed or disabled the original opener.
        unsafe {
            let foreground = GetForegroundWindow();
            if !restore_focus
                || (!foreground.is_null() && foreground != self.owner)
                || IsWindow(self.owner) == 0
                || IsWindowEnabled(self.owner) == 0
                || IsWindowVisible(self.owner) == 0
            {
                return;
            }
            SetActiveWindow(self.owner);
            if GetActiveWindow() != self.owner {
                return;
            }
            if eligible_opener(self.owner, self.opener) {
                SetFocus(self.opener);
            } else {
                focus_first(self.owner);
            }
        }
    }
}

impl Drop for Modal {
    fn drop(&mut self) {
        self.release_owner();
    }
}

unsafe fn eligible_opener(owner: HWND, opener: HWND) -> bool {
    // SAFETY: Win32 handles may be stale, so check membership and state first.
    unsafe {
        !opener.is_null()
            && IsChild(owner, opener) != 0
            && IsWindowEnabled(opener) != 0
            && IsWindowVisible(opener) != 0
            && GetWindowLongPtrW(opener, GWL_STYLE) & WS_TABSTOP as isize != 0
    }
}

unsafe fn focus_first(window: HWND) {
    // SAFETY: The active window owns the controls and runs on this thread.
    unsafe {
        let first = GetNextDlgTabItem(window, null_mut(), 0);
        SetFocus(if first.is_null() { window } else { first });
    }
}

/// Check Escape before passing keyboard input to the dialog manager.
pub(crate) fn takes_escape(focus: HWND, message: &MSG, composing: bool) -> bool {
    if composing {
        return true;
    }
    if focus.is_null() {
        return false;
    }
    // SAFETY: The focused control belongs to the current UI thread. User32
    // borrows the message and the class buffer only for synchronous calls.
    unsafe {
        let mut class = [0_u16; 32];
        let len = GetClassNameW(focus, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let named = |label: &str| class[..len].iter().copied().eq(label.encode_utf16());
        if (named("ComboBox") && SendMessageW(focus, CB_GETDROPPEDSTATE, 0, 0) != 0)
            || named("ComboLBox")
            || (named("SysDateTimePick32") && SendMessageW(focus, DTM_GETMONTHCAL, 0, 0) != 0)
            || named("SysMonthCal32")
        {
            return true;
        }
        let flags = SendMessageW(
            focus,
            WM_GETDLGCODE,
            usize::from(VK_ESCAPE),
            std::ptr::from_ref(message) as LPARAM,
        ) as u32;
        flags & DLGC_WANTALLKEYS != 0
    }
}

fn last_error(operation: &str) -> String {
    format!("{operation}: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_and_clips_dialog_to_the_owners_work_area() {
        let work = RECT {
            left: -1280,
            top: 40,
            right: 0,
            bottom: 760,
        };
        let owner = RECT {
            left: -380,
            top: 50,
            right: 20,
            bottom: 350,
        };
        assert_eq!(centered(owner, work, (200, 180)), (-280, 110, 200, 180));
        assert_eq!(centered(owner, work, (3000, 900)), (-1280, 40, 1280, 720));
    }

    #[test]
    fn native_dialog_keeps_its_owner_and_restores_prior_enabled_state() {
        // SAFETY: This test owns all windows on this test thread and creates
        // no foreground windows. mock_route preserves native dialog handling.
        unsafe {
            let owner = CreateWindowExW(
                0,
                w!("STATIC"),
                w!("Test owner"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                480,
                320,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!owner.is_null());
            for enabled in [true, false, true] {
                EnableWindow(owner, i32::from(enabled));
                let dialog = create(owner, Some(mock_route), "Owned example").unwrap();
                let mut class = [0_u16; 16];
                let count = GetClassNameW(dialog, class.as_mut_ptr(), class.len() as i32) as usize;
                assert_eq!(String::from_utf16_lossy(&class[..count]), "#32770");
                assert_eq!(GetWindow(dialog, GW_OWNER), owner);
                assert_eq!(IsWindowVisible(dialog), 0);
                position(dialog, owner, (300, 120)).unwrap();
                let mut modal = Modal::begin(owner, dialog);
                assert_eq!(IsWindowEnabled(owner), 0);
                modal.release_owner();
                assert_eq!(IsWindowEnabled(owner) != 0, enabled);
                assert_ne!(DestroyWindow(dialog), 0);
                modal.finish(false);
                assert_eq!(IsWindowEnabled(owner) != 0, enabled);
            }
            let dialog = create(owner, Some(mock_route), "Owner removal").unwrap();
            let modal = Modal::begin(owner, dialog);
            assert_ne!(DestroyWindow(owner), 0);
            assert_eq!(IsWindow(dialog), 0);
            modal.finish(false);
        }
    }

    unsafe extern "system" fn mock_route(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            dialog_default(hwnd, message, wparam, lparam)
                .unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam))
        }
    }
}
