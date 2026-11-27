use crate::Result;
use crate::accessibility::{Mount, NativeName};
use crate::accessibility_grid::{self as automation, Action, Operation, Projection, Snapshot};
use crate::grid_clipboard::ClipboardText;
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

#[derive(PartialEq, Eq)]
struct Metadata {
    position: (usize, usize),
    commit: Option<(u32, Listener)>,
    feedback: String,
}

pub(crate) struct Intent {
    pub(crate) grid: Key,
    source: IntentSource,
    text: String,
}

enum IntentSource {
    Edit(Cell),
    Paste {
        listener: (u32, Listener),
        revision: u64,
    },
}

struct Editing {
    handle: HWND,
    cell: Cell,
    generation: u64,
    name: Option<NativeName>,
    automation: Option<Mount>,
}

impl Editing {
    fn automation(&self, focus: HWND) -> automation::Editor {
        automation::Editor {
            cell: self.cell.key,
            hwnd: self.handle as usize,
            generation: self.generation,
            focused: self.handle == focus,
        }
    }
}

impl Drop for Editing {
    fn drop(&mut self) {
        drop(self.automation.take());
        drop(self.name.take());
        // SAFETY: The grid owns this live EDIT HWND on the UI thread.
        unsafe { DestroyWindow(self.handle) };
    }
}

#[derive(Default)]
pub(crate) struct Grid {
    pub(crate) report: ListView,
    cells: BTreeMap<Key, Metadata>,
    active: Option<Key>,
    anchor: Option<Key>,
    paste: Option<(u32, Listener)>,
    revision: u64,
    editing: Option<Editing>,
    edit_generation: u64,
    status: HWND,
    enabled: bool,
    automation: Option<Projection>,
    automation_name: String,
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
        let mut cells = BTreeMap::new();
        let paste = tree.mounted_event(key, "grid-paste");
        for (row, data) in desired.rows.iter().enumerate() {
            for (column, &cell) in data.mounted.iter().enumerate() {
                let feedback = tree.node(cell).attr("feedback").unwrap_or("");
                if feedback.contains('\0') {
                    return Err("grid feedback contains a NUL".into());
                }
                cells.insert(
                    cell,
                    Metadata {
                        position: (row, column),
                        commit: tree.mounted_event(cell, "grid-commit"),
                        feedback: feedback.into(),
                    },
                );
            }
        }
        if self.report.projection() != &desired
            || self.cells != cells
            || self.paste != paste
            || self.enabled != enabled
        {
            self.revision = self.revision.wrapping_add(1);
        }
        self.cells = cells;
        self.paste = paste;
        let surviving_active = self.active.filter(|key| self.cells.contains_key(key));
        self.anchor = self
            .anchor
            .filter(|key| surviving_active.is_some() && self.cells.contains_key(key));
        self.active = surviving_active.or_else(|| desired.rows.first()?.mounted.first().copied());
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
        let (node, listener) = match intent.source {
            IntentSource::Edit(cell) if self.editable(cell.key).as_ref() == Some(&cell) => {
                cell.listener
            }
            IntentSource::Paste { listener, revision }
                if revision == self.revision && self.paste == Some(listener) =>
            {
                listener
            }
            _ => return None,
        };
        Some((node, listener, intent.text))
    }

    pub(crate) fn accepts_paste(&self) -> bool {
        self.enabled && self.paste.is_some() && self.active.is_some() && self.editing.is_none()
    }

    pub(crate) fn paste(&self, key: Key, clipboard: ClipboardText) -> Option<Intent> {
        let listener = self.paste.filter(|_| self.accepts_paste())?;
        let (top, left, bottom, right) = self.range()?;
        let report = self.report.projection();
        let height = (report.rows.len() - top).min(64);
        let width = (report.columns.len() - left).min(32);
        let text = match clipboard {
            ClipboardText::Oversized => "oversized".into(),
            ClipboardText::Unavailable => "unavailable".into(),
            ClipboardText::Text(raw) => {
                let mut parts = vec![
                    (top + 1).to_string(),
                    (bottom - top + 1).to_string(),
                    (right - left + 1).to_string(),
                    height.to_string(),
                    width.to_string(),
                ];
                parts.extend(
                    report.columns[left..left + width]
                        .iter()
                        .map(|column| column.key.clone()),
                );
                let mask = report.rows[top..top + height]
                    .iter()
                    .flat_map(|row| row.mounted[left..left + width].iter())
                    .map(|cell| {
                        if self.cells[cell].commit.is_some() {
                            '1'
                        } else {
                            '0'
                        }
                    })
                    .collect();
                parts.push(mask);
                parts.push(raw);
                parts.join("\0")
            }
        };
        Some(Intent {
            grid: key,
            source: IntentSource::Paste {
                listener,
                revision: self.revision,
            },
            text,
        })
    }

    pub(crate) fn editing(&self, hwnd: HWND) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|editing| editing.handle == hwnd)
    }

    pub(crate) fn cancel(&mut self, hwnd: HWND, refocus: bool) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        if let Some(automation) = &self.automation {
            automation.retire_editor();
        }
        drop(editing);
        if refocus && self.interaction_enabled(hwnd) {
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
        self.anchor = None;
        self.move_to(hwnd, row, column)
    }

    pub(crate) fn select_range(
        &mut self,
        hwnd: HWND,
        start: (usize, usize),
        end: (usize, usize),
    ) -> Result<()> {
        self.anchor = Some(self.report.projection().rows[start.0].mounted[start.1]);
        self.move_to(hwnd, end.0, end.1)
    }

    fn range(&self) -> Option<(usize, usize, usize, usize)> {
        let active = self.active?;
        let (row, column) = self.cells[&active].position;
        let (anchor_row, anchor_column) = self.cells[&self.anchor.unwrap_or(active)].position;
        Some((
            row.min(anchor_row),
            column.min(anchor_column),
            row.max(anchor_row),
            column.max(anchor_column),
        ))
    }

    fn move_to(&mut self, hwnd: HWND, row: usize, column: usize) -> Result<()> {
        let record = self
            .report
            .projection()
            .rows
            .get(row)
            .ok_or("grid row is not mounted")?;
        self.revision = self.revision.wrapping_add(1);
        self.active = Some(*record.mounted.get(column).ok_or("unknown grid column")?);
        self.report.select(hwnd, Some(&record.key))?;
        reveal_cell(hwnd, row, column);
        Ok(())
    }

    pub(crate) fn mount_automation(&mut self, hwnd: HWND, name: &str) -> Result<()> {
        if self.automation.is_none() {
            self.automation = Some(Projection::new(hwnd)?);
        }
        if self.automation_name != name {
            self.automation_name = name.to_owned();
        }
        Ok(())
    }

    pub(crate) fn take_automation(&mut self) -> impl Iterator<Item = Action> + use<> {
        self.automation
            .as_mut()
            .map_or_else(Default::default, |automation| {
                automation.announce();
                automation.drain()
            })
            .into_iter()
    }

    pub(crate) fn automation_action(
        &mut self,
        key: Key,
        hwnd: HWND,
        action: Action,
    ) -> Result<Option<Intent>> {
        let Some(cell) = self
            .cells
            .get(&action.cell)
            .filter(|_| self.interaction_enabled(hwnd))
        else {
            return Ok(None);
        };
        let (row, column) = cell.position;
        match action.operation {
            Operation::SetValue(text) => {
                return Ok(self
                    .editable(action.cell)
                    .filter(|_| self.editing.is_none())
                    .map(|cell| Intent {
                        grid: key,
                        source: IntentSource::Edit(cell),
                        text,
                    }));
            }
            Operation::Select => self.activate(hwnd, row, column)?,
            Operation::ExtendSelection => {
                if self.anchor.is_none() {
                    self.anchor = self.active;
                }
                self.move_to(hwnd, row, column)?;
            }
            Operation::Reveal => reveal_cell(hwnd, row, column),
            Operation::Focus => {
                self.activate(hwnd, row, column)?;
                // SAFETY: The grid is live, enabled, and on this UI thread.
                unsafe { SetFocus(hwnd) };
            }
            Operation::FocusEditor(generation) => {
                if let Some(editing) = self
                    .editing
                    .as_ref()
                    .filter(|edit| edit.generation == generation && edit.cell.key == action.cell)
                {
                    // SAFETY: This EDIT still belongs to the enabled grid.
                    unsafe { SetFocus(editing.handle) };
                }
            }
            Operation::Scroll([x, y]) => {
                let horizontal = scroll_axis(hwnd, SB_HORZ);
                let vertical = scroll_axis(hwnd, SB_VERT);
                let row_height =
                    subitem_rect(hwnd, 0, 0).map_or(1, |rect| (rect.bottom - rect.top).max(1));
                // SAFETY: Report-mode horizontal scrolling uses device pixels and vertical scrolling uses row heights.
                unsafe {
                    SendMessageW(
                        hwnd,
                        LVM_SCROLL,
                        (x - horizontal.position) as usize,
                        ((y - vertical.position) * row_height) as isize,
                    )
                };
            }
        }
        Ok(None)
    }

    pub(crate) fn publish_automation(&mut self, hwnd: HWND) {
        let Some(automation) = &self.automation else {
            return;
        };
        let report = self.report.projection();
        // SAFETY: This native control and its header live on our UI thread.
        let (header, first, limit, focus) = unsafe {
            let first = SendMessageW(hwnd, LVM_GETTOPINDEX, 0, 0).max(0) as usize;
            let count = SendMessageW(hwnd, LVM_GETCOUNTPERPAGE, 0, 0).max(0) as usize;
            (
                SendMessageW(hwnd, LVM_GETHEADER, 0, 0) as HWND,
                first,
                first.saturating_add(count).saturating_add(1),
                GetFocus(),
            )
        };
        let headers = report
            .columns
            .iter()
            .enumerate()
            .map(|(column, info)| {
                let mut rect = RECT::default();
                // SAFETY: The header fills this RECT for its current column.
                unsafe {
                    if SendMessageW(header, HDM_GETITEMRECT, column, address(&mut rect)) != 0 {
                        MapWindowPoints(header, null_mut(), (&mut rect as *mut RECT).cast(), 2);
                    }
                }
                (info.heading.clone(), rect_to_uia(rect))
            })
            .collect();
        let mut order = Vec::new();
        let mut cells = BTreeMap::new();
        for (row, rendered) in report.rows.iter().enumerate() {
            for (column, &key) in rendered.mounted.iter().enumerate() {
                let meta = &self.cells[&key];
                let mut bounds = windows::Win32::UI::Accessibility::UiaRect::default();
                if (first..limit).contains(&row)
                    && let Some(mut rect) = visible_rect(hwnd, row, column)
                {
                    // SAFETY: Map the live cell's client RECT to screen coordinates.
                    unsafe {
                        MapWindowPoints(hwnd, null_mut(), (&mut rect as *mut RECT).cast(), 2)
                    };
                    bounds = rect_to_uia(rect);
                }
                order.push(key);
                cells.insert(
                    key,
                    automation::Cell {
                        row,
                        column,
                        value: rendered.cells[column].clone(),
                        feedback: meta.feedback.clone(),
                        read_only: meta.commit.is_none(),
                        bounds,
                    },
                );
            }
        }
        automation.update(Snapshot {
            name: self.automation_name.clone(),
            headers,
            order,
            cells,
            bounds: rect_to_uia(win_rect(hwnd)),
            active: self.active,
            range: self.range(),
            focused: focus == hwnd,
            enabled: self.interaction_enabled(hwnd),
            editor: self
                .editing
                .as_ref()
                .map(|editing| editing.automation(focus)),
            scroll: [scroll_axis(hwnd, SB_HORZ), scroll_axis(hwnd, SB_VERT)],
        });
    }

    pub(crate) fn refresh_automation_interaction(&self, hwnd: HWND) {
        if let Some(automation) = &self.automation {
            // SAFETY: GetFocus refers to the UI thread that owns the live grid.
            let focus = unsafe { GetFocus() };
            automation.set_interaction(
                self.interaction_enabled(hwnd),
                focus == hwnd,
                self.editing(focus),
            );
        }
    }

    fn interaction_enabled(&self, hwnd: HWND) -> bool {
        // SAFETY: These checks run on the UI thread with the mounted grid HWND.
        self.enabled
            && unsafe {
                IsWindowVisible(hwnd) != 0
                    && IsWindowEnabled(hwnd) != 0
                    && IsWindowEnabled(GetAncestor(hwnd, GA_ROOT)) != 0
            }
    }
}

fn reveal_cell(hwnd: HWND, row: usize, column: usize) {
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
}

fn rect_to_uia(rect: RECT) -> windows::Win32::UI::Accessibility::UiaRect {
    windows::Win32::UI::Accessibility::UiaRect {
        left: f64::from(rect.left),
        top: f64::from(rect.top),
        width: f64::from((rect.right - rect.left).max(0)),
        height: f64::from((rect.bottom - rect.top).max(0)),
    }
}

fn scroll_axis(hwnd: HWND, axis: i32) -> automation::ScrollAxis {
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_PAGE | SIF_POS | SIF_RANGE,
        ..SCROLLINFO::default()
    };
    // SAFETY: The report lives on our UI thread and supplies the scrollbar state.
    if unsafe { GetScrollInfo(hwnd, axis, &mut info) } == 0 {
        return automation::ScrollAxis::default();
    }
    let length = (info.nMax - info.nMin + 1).max(0);
    let page = (info.nPage as i32).clamp(0, length);
    automation::ScrollAxis {
        position: (info.nPos - info.nMin).clamp(0, length - page),
        length,
        page,
    }
}

impl Grid {
    pub(crate) fn navigate(&mut self, hwnd: HWND, key: u16, extend: bool) -> Result<()> {
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
        self.anchor = if extend {
            self.anchor.or(self.active)
        } else {
            None
        };
        self.move_to(hwnd, row, column)
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
        let (_, column) = self.cells[&cell.key].position;
        let mut name = None;
        NativeName::annotate(
            handle,
            &self.report.projection().columns[column].heading,
            &mut name,
        );
        if let Some(annotation) = &mut name {
            annotation.help(&self.cells[&cell.key].feedback);
        }
        self.edit_generation = self.edit_generation.wrapping_add(1);
        let mut editing = Editing {
            handle,
            cell,
            generation: self.edit_generation,
            name,
            automation: None,
        };
        if let Some(automation) = &self.automation {
            editing.automation =
                Some(automation.mount_editor(handle, editing.automation(null_mut()))?);
        }
        self.editing = Some(editing);
        self.position_editor(hwnd, window);
        self.publish_automation(hwnd);
        if self.editing.is_some() {
            // SAFETY: The transient editor is live and arranged.
            unsafe { SetFocus(handle) };
            self.refresh_automation_interaction(hwnd);
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
            source: IntentSource::Edit(editing.cell.clone()),
            text: window_text(editing.handle),
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
        let mut status = if let Some(meta) = self.active.and_then(|key| self.cells.get(&key)) {
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
        if let Some((top, left, bottom, right)) = self.range()
            && (top != bottom || left != right)
        {
            status.push_str(&format!(
                " · {} × {} cells selected",
                bottom - top + 1,
                right - left + 1
            ));
        }
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
        self.publish_automation(hwnd);
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
                if let Some((top, left, bottom, right)) = self.range()
                    && let Some(mut rect) = range_rect(hwnd, top, left, bottom, right)
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

fn range_rect(hwnd: HWND, top: usize, left: usize, bottom: usize, right: usize) -> Option<RECT> {
    let first = subitem_rect(hwnd, top, left)?;
    let last = subitem_rect(hwnd, bottom, right)?;
    let mut client = RECT::default();
    // SAFETY: The report HWND is live and provides a writable client RECT.
    let header = unsafe {
        GetClientRect(hwnd, &mut client);
        SendMessageW(hwnd, LVM_GETHEADER, 0, 0) as HWND
    };
    let header = win_rect(header);
    let rect = RECT {
        left: first.left.max(client.left),
        top: first.top.max(header.bottom - header.top),
        right: last.right.min(client.right),
        bottom: last.bottom.min(client.bottom),
    };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

fn address<T>(value: &mut T) -> isize {
    std::ptr::from_mut(value) as isize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::OwnedWindow;
    use std::ptr::null;

    #[test]
    fn native_owner_must_be_enabled_for_grid_automation() {
        // SAFETY: Both native windows belong to this test thread. The owner
        // stays offscreen and does not activate the test process.
        unsafe {
            let owner = OwnedWindow(CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Grid automation test"),
                WS_POPUP,
                -400,
                -400,
                100,
                100,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            ));
            assert!(!owner.0.is_null());
            let handle = child(owner.0, null_mut(), 0, w!("STATIC"), WS_VISIBLE).unwrap();
            let mut grid = Grid::default();
            grid.enabled = true;
            assert!(!grid.interaction_enabled(handle));
            ShowWindow(owner.0, SW_SHOWNOACTIVATE);
            assert!(grid.interaction_enabled(handle));
            EnableWindow(owner.0, 0);
            assert_ne!(IsWindowEnabled(handle), 0);
            assert!(!grid.interaction_enabled(handle));
            EnableWindow(owner.0, 1);
            assert!(grid.interaction_enabled(handle));
        }
    }
}
