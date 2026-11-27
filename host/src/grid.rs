use crate::Result;
use crate::list_view::{ListView, Report};
use crate::view::{child, position, scale, set_text, window_text};
use crate::wire::{Key, Listener, Tree};
use std::collections::BTreeMap;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{DrawFocusRect, HFONT, InvalidateRect, MapWindowPoints};
use windows_sys::Win32::System::SystemServices::{SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::Ime::{
    GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

const TEXT_LIMIT: usize = 4096;

#[derive(Clone, PartialEq, Eq)]
struct Cell {
    key: Key,
    listener: (u32, Listener),
    value: String,
}

struct Metadata {
    position: (usize, usize),
    commit: Option<(u32, Listener)>,
    feedback: String,
}

pub(crate) struct Intent {
    pub(crate) grid: Key,
    cell: Cell,
    draft: String,
}

struct Editing {
    handle: HWND,
    cell: Cell,
}

impl Drop for Editing {
    fn drop(&mut self) {
        // SAFETY: The grid owns this live EDIT HWND on the UI thread.
        unsafe { DestroyWindow(self.handle) };
    }
}

#[derive(Default)]
pub(crate) struct Grid {
    pub(crate) report: ListView,
    cells: BTreeMap<Key, Metadata>,
    active: Option<Key>,
    editing: Option<Editing>,
    status: HWND,
    enabled: bool,
}

impl Grid {
    pub(crate) fn sync(
        &mut self,
        tree: &Tree,
        key: Key,
        hwnd: HWND,
        font: HFONT,
        dpi: u32,
        enabled: bool,
    ) -> Result<()> {
        let desired = Report::read_grid(tree, key)?;
        self.cells.clear();
        for (row, data) in desired.rows.iter().enumerate() {
            for (column, &cell) in data.mounted.iter().enumerate() {
                let feedback = tree.node(cell).attr("feedback").unwrap_or("");
                if feedback.contains('\0') {
                    return Err("grid feedback contains a NUL".into());
                }
                self.cells.insert(
                    cell,
                    Metadata {
                        position: (row, column),
                        commit: tree.mounted_event(cell, "grid-commit"),
                        feedback: feedback.into(),
                    },
                );
            }
        }
        self.active = self
            .active
            .filter(|key| self.cells.contains_key(key))
            .or_else(|| desired.rows.first()?.mounted.first().copied());
        let selected = self
            .active
            .and_then(|key| self.cells.get(&key))
            .map(|meta| desired.rows[meta.position.0].key.clone());
        self.report
            .sync(hwnd, font, dpi, desired, selected.as_deref())?;
        self.enabled = enabled;
        if let Some(editing) = &self.editing
            && self.editable(editing.cell.key).as_ref() != Some(&editing.cell)
        {
            self.cancel(hwnd, true);
        }
        Ok(())
    }

    fn editable(&self, key: Key) -> Option<Cell> {
        if !self.enabled {
            return None;
        }
        let meta = self.cells.get(&key)?;
        Some(Cell {
            key,
            listener: meta.commit?,
            value: self.text(meta.position.0, meta.position.1).into(),
        })
    }

    pub(crate) fn resolve(&self, intent: Intent) -> Option<(u32, Listener, String)> {
        if self.editable(intent.cell.key).as_ref() != Some(&intent.cell) {
            return None;
        }
        let (node, listener) = intent.cell.listener;
        Some((node, listener, intent.draft))
    }

    pub(crate) fn editing(&self, hwnd: HWND) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|editing| editing.handle == hwnd)
    }

    pub(crate) fn cancel(&mut self, hwnd: HWND, refocus: bool) {
        if self.editing.take().is_some() && refocus && self.enabled {
            // SAFETY: The report is on the UI thread and outlives the editor.
            unsafe { SetFocus(hwnd) };
        }
    }

    pub(crate) fn font(&self, font: HFONT) {
        // SAFETY: These optional surface windows live on the UI thread.
        unsafe {
            if let Some(editing) = &self.editing {
                SendMessageW(editing.handle, WM_SETFONT, font as usize, 1);
            }
            if !self.status.is_null() {
                SendMessageW(self.status, WM_SETFONT, font as usize, 1);
            }
        }
    }

    pub(crate) fn activate(&mut self, hwnd: HWND, row: usize, column: usize) -> Result<()> {
        let record = self
            .report
            .projection()
            .rows
            .get(row)
            .ok_or("grid row is not mounted")?;
        self.active = Some(*record.mounted.get(column).ok_or("unknown grid column")?);
        self.report.select(hwnd, Some(&record.key))?;
        // SAFETY: The live report is owned by this grid on the UI thread.
        unsafe {
            SendMessageW(hwnd, LVM_ENSUREVISIBLE, row, 0);
            if let Some(rect) = subitem_rect(hwnd, row, column) {
                let mut client = RECT::default();
                GetClientRect(hwnd, &mut client);
                let scroll = if rect.left < 0 {
                    rect.left
                } else {
                    (rect.right - client.right).clamp(0, rect.left)
                };
                if scroll != 0 {
                    SendMessageW(hwnd, LVM_SCROLL, scroll as usize, 0);
                }
            }
            InvalidateRect(hwnd, null_mut(), 0);
        }
        Ok(())
    }

    pub(crate) fn navigate(&mut self, hwnd: HWND, key: u16) -> Result<()> {
        let report = self.report.projection();
        let Some(meta) = self.active.and_then(|id| self.cells.get(&id)) else {
            return Ok(());
        };
        let (row, column) = meta.position;
        let (row, column) = match key {
            VK_LEFT => (row, column.saturating_sub(1)),
            VK_RIGHT => (row, (column + 1).min(report.columns.len() - 1)),
            VK_UP => (row.saturating_sub(1), column),
            VK_DOWN => ((row + 1).min(report.rows.len() - 1), column),
            _ => return Ok(()),
        };
        self.activate(hwnd, row, column)
    }

    pub(crate) fn click(&mut self, hwnd: HWND, point: POINT) -> Result<bool> {
        let mut hit = LVHITTESTINFO {
            pt: point,
            ..LVHITTESTINFO::default()
        };
        // SAFETY: The report reads the point and writes the provided hit record.
        let index = unsafe { SendMessageW(hwnd, LVM_SUBITEMHITTEST, 0, address(&mut hit)) };
        if index < 0 || hit.iSubItem < 0 {
            return Ok(false);
        }
        self.activate(hwnd, index as usize, hit.iSubItem as usize)?;
        Ok(true)
    }

    pub(crate) fn begin(&mut self, hwnd: HWND, window: HWND, font: HFONT) -> Result<()> {
        if self.editing.is_some() {
            return Ok(());
        }
        let Some(cell) = self.active.and_then(|key| self.editable(key)) else {
            return Ok(());
        };
        let handle = child(
            window,
            font,
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            ES_AUTOHSCROLL as u32,
        )?;
        set_text(handle, &cell.value);
        // SAFETY: The EDIT lives on the UI thread and receives only scalar payloads.
        unsafe {
            SendMessageW(handle, EM_LIMITTEXT, TEXT_LIMIT, 0);
            SendMessageW(handle, EM_SETSEL, 0, -1);
        }
        self.editing = Some(Editing { handle, cell });
        self.position_editor(hwnd, window);
        if self.editing.is_some() {
            // SAFETY: The transient editor is live and arranged.
            unsafe { SetFocus(handle) };
        }
        Ok(())
    }

    pub(crate) fn composition(&self) -> bool {
        let Some(editing) = &self.editing else {
            return false;
        };
        // SAFETY: The EDIT owns the IME context, which is released before return.
        unsafe {
            let context = ImmGetContext(editing.handle);
            if context.is_null() {
                return false;
            }
            let composing = ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0) > 0;
            ImmReleaseContext(editing.handle, context);
            composing
        }
    }

    pub(crate) fn commit(&mut self, key: Key, hwnd: HWND) -> Option<Intent> {
        let editing = self.editing.as_ref()?;
        let intent = Intent {
            grid: key,
            cell: editing.cell.clone(),
            draft: window_text(editing.handle),
        };
        self.cancel(hwnd, true);
        Some(intent)
    }

    pub(crate) fn fill(&self, text: &str) -> Result<()> {
        let editing = self.editing.as_ref().ok_or("no grid editor is open")?;
        if text.contains('\0') || text.encode_utf16().count() > TEXT_LIMIT {
            return Err("grid drafts allow at most 4096 UTF-16 units and no NUL".into());
        }
        set_text(editing.handle, text);
        Ok(())
    }

    pub(crate) fn location(&self, row: usize, column: &str) -> Result<(usize, usize)> {
        let report = self.report.projection();
        if row >= report.rows.len() {
            return Err(format!("grid row {} is not mounted", row + 1));
        }
        let column = report
            .columns
            .iter()
            .position(|item| item.key == column)
            .ok_or_else(|| format!("unknown grid column {column:?}"))?;
        Ok((row, column))
    }

    pub(crate) fn text(&self, row: usize, column: usize) -> &str {
        &self.report.projection().rows[row].cells[column]
    }

    pub(crate) fn native_text(&self, hwnd: HWND, row: usize, column: usize) -> String {
        let size = self
            .text(row, column)
            .encode_utf16()
            .count()
            .saturating_add(2);
        let mut utf16 = vec![0; size];
        let mut item = LVITEMW {
            iSubItem: column as i32,
            pszText: utf16.as_mut_ptr(),
            cchTextMax: size as i32,
            ..LVITEMW::default()
        };
        // SAFETY: The report writes within the buffer described by this LVITEMW.
        let used =
            unsafe { SendMessageW(hwnd, LVM_GETITEMTEXTW, row, address(&mut item)) } as usize;
        String::from_utf16_lossy(&utf16[..used.min(utf16.len())])
    }

    pub(crate) fn error(&self, row: usize, column: usize) -> &str {
        let key = self.report.projection().rows[row].mounted[column];
        &self.cells[&key].feedback
    }

    pub(crate) fn active(&self, row: usize, column: usize) -> bool {
        self.active == Some(self.report.projection().rows[row].mounted[column])
    }

    pub(crate) fn arrange(
        &mut self,
        hwnd: HWND,
        window: HWND,
        font: HFONT,
        dpi: u32,
    ) -> Result<HWND> {
        if self.status.is_null() {
            self.status = child(window, font, 0, w!("STATIC"), SS_LEFT | SS_NOPREFIX)?;
        }
        let mut rect = win_rect(hwnd);
        // SAFETY: A RECT contains two adjacent POINT values.
        unsafe { MapWindowPoints(null_mut(), window, (&mut rect as *mut RECT).cast(), 2) };
        let status = if let Some(meta) = self.active.and_then(|key| self.cells.get(&key)) {
            let (row, column) = meta.position;
            let heading = &self.report.projection().columns[column].heading;
            if meta.feedback.is_empty() {
                format!("Row {} · {heading}", row + 1)
            } else {
                format!("Row {} · {heading}: {}", row + 1, meta.feedback)
            }
        } else {
            "No rows".into()
        };
        if window_text(self.status) != status {
            set_text(self.status, &status);
        }
        let after = if self.editing.is_some() {
            HWND_TOP
        } else {
            hwnd
        };
        position(
            self.status,
            after,
            rect.left,
            rect.bottom,
            rect.right - rect.left,
            scale(dpi, 22),
        );
        self.position_editor(hwnd, window);
        Ok(self.status)
    }

    pub(crate) fn position_editor(&mut self, hwnd: HWND, window: HWND) {
        let Some(editing) = &self.editing else {
            return;
        };
        let rect = self
            .cells
            .get(&editing.cell.key)
            .and_then(|meta| visible_rect(hwnd, meta.position.0, meta.position.1));
        let Some(mut rect) = rect else {
            self.cancel(hwnd, true);
            return;
        };
        // SAFETY: Both windows are on this thread, and the RECT contains two POINTs.
        unsafe { MapWindowPoints(hwnd, window, (&mut rect as *mut RECT).cast(), 2) };
        position(
            editing.handle,
            self.status,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        );
    }

    pub(crate) fn draw(&self, hwnd: HWND, details: &NMCUSTOMDRAW) -> isize {
        match details.dwDrawStage {
            CDDS_PREPAINT => CDRF_NOTIFYPOSTPAINT as isize,
            CDDS_POSTPAINT => {
                if let Some(meta) = self.active.and_then(|key| self.cells.get(&key))
                    && let Some(mut rect) = visible_rect(hwnd, meta.position.0, meta.position.1)
                {
                    rect.left += 1;
                    rect.right -= 1;
                    // SAFETY: The WM_NOTIFY payload provides a live paint DC.
                    unsafe { DrawFocusRect(details.hdc, &rect) };
                }
                CDRF_DODEFAULT as isize
            }
            _ => CDRF_DODEFAULT as isize,
        }
    }
}

impl Drop for Grid {
    fn drop(&mut self) {
        self.editing.take();
        if !self.status.is_null() {
            // SAFETY: The status label belongs to the grid on this thread.
            unsafe { DestroyWindow(self.status) };
        }
    }
}

fn win_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: The RECT is writable and the HWND is owned by this thread.
    unsafe { GetWindowRect(hwnd, &mut rect) };
    rect
}

fn subitem_rect(hwnd: HWND, row: usize, column: usize) -> Option<RECT> {
    let mut rect = RECT {
        top: column as i32,
        left: LVIR_BOUNDS as i32,
        ..RECT::default()
    };
    // SAFETY: The report takes a row index and writable RECT with input members.
    unsafe {
        if SendMessageW(hwnd, LVM_GETSUBITEMRECT, row, address(&mut rect)) == 0 {
            return None;
        }
        if column == 0 {
            rect.right = rect.left + SendMessageW(hwnd, LVM_GETCOLUMNWIDTH, 0, 0) as i32;
        }
    }
    Some(rect)
}

fn visible_rect(hwnd: HWND, row: usize, column: usize) -> Option<RECT> {
    let mut rect = subitem_rect(hwnd, row, column)?;
    let mut client = RECT::default();
    // SAFETY: The report is a live HWND and the RECT is writable.
    let header = unsafe {
        GetClientRect(hwnd, &mut client);
        SendMessageW(hwnd, LVM_GETHEADER, 0, 0) as HWND
    };
    let header = win_rect(header);
    rect.left = rect.left.max(client.left);
    rect.right = rect.right.min(client.right);
    (rect.top >= header.bottom - header.top
        && rect.bottom <= client.bottom
        && rect.right > rect.left)
        .then_some(rect)
}

fn address<T>(value: &mut T) -> isize {
    std::ptr::from_mut(value) as isize
}
