use crate::abi::Engine;
use crate::civil::{self, Value as CivilValue};
use crate::effects::{Effects, SpecOptions};
use crate::grid::Intent as GridIntent;
use crate::modal;
use crate::taskbar::native::Taskbar;
use crate::tree_view::TreeIntent;
use crate::view::{composing, wide};
use crate::windows::{OwnedWindow, SpecSurface, SurfaceRegistry};
use crate::wire::{Declarations, EFFECT_NODE, Key, Reader, Tree};
use crate::{Event, Result};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
use windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::System::LibraryLoader::{
    FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
};
use windows_sys::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
use windows_sys::Win32::System::Time::{
    GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
};
use windows_sys::Win32::UI::Controls::{
    DTN_DATETIMECHANGE, ICC_BAR_CLASSES, ICC_DATE_CLASSES, ICC_LISTVIEW_CLASSES,
    ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, ICC_TREEVIEW_CLASSES,
    INITCOMMONCONTROLSEX, InitCommonControlsEx, LVIF_STATE, LVIS_SELECTED, LVN_COLUMNCLICK,
    LVN_ENDSCROLL, LVN_ITEMCHANGED, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_RETURN, NMCUSTOMDRAW,
    NMHDR, NMITEMACTIVATE, NMLISTVIEW, NMTREEVIEWW, TCN_SELCHANGE, TVN_ITEMEXPANDEDW,
    TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F2, VK_LEFT, VK_MENU,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

#[path = "spec_run.rs"]
mod spec_run;

const PUMP: usize = 1;
const REPORT_EVENTS: u32 = WM_APP + 1;
const GRID_EVENTS: u32 = WM_APP + 2;
const CLOSE_EVENTS: u32 = WM_APP + 6;
const WINDOW_CLASS: PCWSTR = w!("RocSignalsWindow");

thread_local! {
    // User32 is allowed to send synchronous child notifications during a
    // property update. A nested window callback must not re-enter the app.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct App {
    engine: Engine,
    tree: Tree,
    surface: SurfaceRegistry,
    pump: OwnedWindow,
    close_requests: VecDeque<(Key, usize)>,
    quit_posted: bool,
    effects: Effects,
    incoming: Receiver<Event>,
    private: VecDeque<Event>,
    opportunity: Option<u64>,
    report_selection: BTreeSet<Key>,
    report_actions: VecDeque<(Key, &'static str, String)>,
    grid_intents: VecDeque<GridIntent>,
    focus_departures: BTreeSet<Key>,
    driven: bool,
}

impl App {
    fn start(window: OwnedWindow, options: Option<SpecOptions>) -> Result<Self> {
        let (send, incoming) = channel();
        let deterministic = options.as_ref().is_some_and(|options| !options.live_time);
        let initial_time = options.as_ref().map_or(0, |options| options.initial_time);
        let driven = options.is_some();
        let pump = create_pump()?;
        let title = app_name(unsafe { GetModuleHandleW(null()) });
        let effects = Effects::new(pump.0, send);
        let mut app = Self {
            engine: Engine::default(),
            tree: Tree::new(),
            surface: SurfaceRegistry::new(window, title, driven),
            pump,
            close_requests: VecDeque::new(),
            quit_posted: false,
            effects,
            incoming,
            private: VecDeque::new(),
            opportunity: None,
            report_selection: BTreeSet::new(),
            report_actions: VecDeque::new(),
            grid_intents: VecDeque::new(),
            focus_departures: BTreeSet::new(),
            driven,
        };
        if let Some(options) = options {
            app.effects.configure_spec(options)?;
        }
        let declarations = Declarations::decode(&Engine::declarations()?)?;
        let (now, seed, locale, zone) = if deterministic {
            (initial_time, [1; 16], "en-US".into(), 0)
        } else {
            live_boot()?
        };
        let boot = declarations.boot(now, seed, &locale, zone, |session, key| {
            app.effects.boot_value(session, key)
        })?;
        let output = app.engine.mount(&boot);
        app.apply(&output)?;
        app.drain_private()?;
        Ok(app)
    }

    fn deliver(&mut self, event: Event) -> Result<()> {
        let output = self.engine.event(event)?;
        self.apply(&output)
    }

    fn drain_private(&mut self) -> Result<()> {
        while !self.exited()
            && let Some(event) = self.private.pop_front()
        {
            self.deliver(event)?;
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<()> {
        if self.exited() {
            return Ok(());
        }
        self.surface.poll();
        self.effects.poll();
        if let Some(generation) = self.opportunity.take() {
            let mut payload = vec![255];
            payload.extend_from_slice(&generation.to_le_bytes());
            self.deliver(effect_event(payload))?;
            self.drain_private()?;
        }
        while !self.exited()
            && let Ok(event) = self.incoming.try_recv()
        {
            self.deliver(event)?;
            self.drain_private()?;
        }
        Ok(())
    }

    fn apply(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut reader = Reader::new(bytes);
        let flags = reader.u8()?;
        if flags > 1 {
            return Err(format!("unknown command flags {flags}"));
        }
        let mut references = Vec::new();
        while !reader.done() {
            let opcode = reader.u8()?;
            if self.tree.command(opcode, &mut reader)? {
                continue;
            }
            match opcode {
                13 => {
                    let id = reader.u32()?;
                    let kind = reader.u8()?;
                    self.effects.start(id, kind, &mut reader)?;
                }
                14 => self.effects.cancel(reader.u32()?),
                15 => {
                    reader.u8()?;
                    reader.str()?;
                }
                16 => {
                    let op = reader.u8()?;
                    let key = reader.string()?;
                    let value = if op == 1 || op == 3 {
                        Some(reader.str()?)
                    } else {
                        None
                    };
                    self.effects.storage(op, key, value)?;
                }
                17 => self.surface.set_legacy_title(reader.str()?),
                18 => {
                    let op = reader.u8()?;
                    let id = reader.u32()?;
                    read_reference(op, &mut reader)?;
                    references.push((op, id));
                }
                20 => {
                    let token = reader.u64()?;
                    let start = reader.u64()?;
                    let length = reader.u64()?;
                    let owner = self
                        .surface
                        .legacy_handle()
                        .ok_or("Save As needs an explicit owner in a managed window collection")?;
                    self.effects
                        .save(owner, token, start, length, reader.string()?)?;
                }
                24 => {
                    for _ in 0..reader.u32()? {
                        reader.str()?;
                    }
                }
                25 => {
                    reader.u8()?; // A browser navigation guard.
                }
                28 => {
                    if reader.u8()? != 0 {
                        return Err("Win32 startup supports fresh mount only".into());
                    }
                    reader.u32()?;
                }
                29 => self.private.push_front(effect_event(vec![254])),
                30 => {}
                31 => {
                    let action = reader.u8()?;
                    let generation = reader.u64()?;
                    match action {
                        0 if self.opportunity == Some(generation) => self.opportunity = None,
                        0 => {}
                        1 => self.opportunity = Some(generation),
                        _ => return Err(format!("unknown opportunity action {action}")),
                    }
                }
                _ => return Err(format!("unsupported command {opcode}")),
            }
        }
        self.surface.sync(&self.tree)?;
        for (op, id) in references {
            if let Some(key) = self.tree.reference(id)
                && let Some(owner) = self.surface.for_key(key)
                && !self.surface.blocked(owner)
                && let Some(surface) = self.surface.at_mut(owner)
            {
                surface.reference(op, key);
            }
        }
        self.finish_exit();
        Ok(())
    }

    fn spec_surfaces(&self) -> Vec<SpecSurface> {
        self.surface.all()
    }

    fn select_surface(&mut self, id: Key) -> Result<()> {
        if self.surface.select(id) {
            Ok(())
        } else {
            Err("the selected window is no longer mounted".into())
        }
    }

    fn surface_for_key(&self, key: Key) -> Option<Key> {
        self.tree
            .contains(key)
            .then(|| self.surface.for_key(key))
            .flatten()
    }

    fn select_key(&mut self, key: Key) -> bool {
        self.surface_for_key(key)
            .is_some_and(|id| !self.surface.blocked(id) && self.surface.select(id))
    }

    fn select_message(&mut self, handle: HWND) -> bool {
        self.surface
            .for_handle(handle)
            .is_some_and(|id| self.select_surface(id).is_ok())
    }

    fn request_close(&mut self, id: Key) -> Result<()> {
        let Some(hwnd) = self
            .surface
            .handle(id)
            .filter(|_| !self.surface.blocked(id))
        else {
            return Ok(());
        };
        let request = (id, hwnd as usize);
        if !self.close_requests.contains(&request) {
            self.close_requests.push_back(request);
        }
        if self.driven {
            self.drain_closes()
        } else {
            self.queue(CLOSE_EVENTS, "queue window close request")
        }
    }

    fn drain_closes(&mut self) -> Result<()> {
        while let Some((id, address)) = self.close_requests.pop_front() {
            if self.surface.handle(id).map(|hwnd| hwnd as usize) != Some(address)
                || self.surface.blocked(id)
                || self.select_surface(id).is_err()
            {
                continue;
            }
            match self.surface.target(id) {
                Some(Some(key)) => {
                    if let Some((node, listener)) = self.tree.mounted_event(key, "close-request") {
                        self.deliver(Event::Turn {
                            node,
                            handler: listener.handler,
                            payload: Vec::new(),
                        })?;
                        self.drain_private()?;
                    }
                }
                Some(None) => {
                    self.surface.shutdown();
                    self.finish_exit();
                }
                None => {}
            }
        }
        Ok(())
    }

    fn exited(&self) -> bool {
        self.surface.exited()
    }

    fn finish_exit(&mut self) {
        if self.exited() && !self.quit_posted {
            self.quit_posted = true;
            // SAFETY: The host owns the UI thread queue, and there are no app surfaces.
            unsafe { PostQuitMessage(0) };
        }
    }

    fn queue(&self, event: u32, trace: &str) -> Result<()> {
        // SAFETY: The process-lifetime message HWND belongs to this UI thread.
        if unsafe { PostMessageW(self.pump.0, event, 0, 0) } == 0 {
            return Err(last_error(trace));
        }
        Ok(())
    }

    fn toolbar_command(&mut self, handle: HWND, command: u16) -> Result<()> {
        let Some(key) = self
            .surface
            .toolbar(handle)
            .and_then(|toolbar| toolbar.dispatch(handle, command))
        else {
            return Ok(());
        };
        if let Some((node, listener)) = self.tree.mounted_event(key, "click") {
            if listener.kind != 10 {
                return Err("toolbar commands require a click listener".into());
            }
            self.deliver(Event::Turn {
                node,
                handler: listener.handler,
                payload: vec![modifiers(), 0],
            })?;
            self.drain_private()?;
        }
        Ok(())
    }

    fn command(&mut self, control: HWND, notification: u16) -> Result<()> {
        if u32::from(notification) == EN_KILLFOCUS
            && let Some(key) = self.surface.grid_key(control)
            && self
                .surface
                .grid(key)
                .is_some_and(|(_, grid)| grid.editing(control))
        {
            self.focus_departures.insert(key);
            return self.queue_grid();
        }
        let Some(key) = self.surface.key(control) else {
            return Ok(());
        };
        let view_kind = self.surface.kind(key);
        use crate::view::Control;
        let (event_name, value) = match (view_kind, u32::from(notification)) {
            (Control::Edit | Control::TextArea, EN_CHANGE) => ("input", self.surface.input(key)),
            (Control::Button, BN_CLICKED) => ("click", String::new()),
            (Control::Checkbox | Control::AggregateCheckbox, BN_CLICKED) => {
                ("change", self.surface.input(key))
            }
            (kind, code) if kind.is_choice() && kind != Control::Tabs => {
                let Some(value) = self.surface.choice_event(key, control, code) else {
                    return Ok(());
                };
                ("change", value)
            }
            (Control::FileButton, BN_CLICKED) => {
                if let Some((node, listener)) = self.tree.event(key, "change") {
                    let multiple = self
                        .tree
                        .node(key)
                        .attr("multiple")
                        .is_some_and(enabled_flag);
                    let (owner, _, _) = self.surface.grid_coordinates();
                    self.effects.pick(owner, node, listener.handler, multiple)?;
                }
                return Ok(());
            }
            (Control::FolderButton, BN_CLICKED) => {
                if let Some((node, listener)) = self.tree.event(key, "change") {
                    let (owner, _, _) = self.surface.grid_coordinates();
                    self.effects.pick_folder(owner, node, listener.handler)?;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.control_event(key, event_name, value)
    }

    fn civil_change(&mut self, key: crate::wire::Key, value: CivilValue) -> Result<()> {
        if self.surface.civil_changed(key, value) {
            self.control_event(key, "change", value.to_string())?;
        }
        Ok(())
    }

    fn notify(&mut self, source: HWND, code: u32) -> Result<()> {
        if code != TCN_SELCHANGE {
            return Ok(());
        }
        if let Some(key) = self.surface.key(source)
            && self.surface.kind(key) == crate::view::Control::Tabs
            && let Some(value) = self.surface.choice_event(key, source, code)
        {
            self.control_event(key, "change", value)?;
        }
        Ok(())
    }

    fn tree_event(&mut self, key: crate::wire::Key, intent: TreeIntent) -> Result<()> {
        let (event, payload) = intent.event();
        self.control_event(key, event, payload)
    }

    fn control_event(
        &mut self,
        key: crate::wire::Key,
        event_name: &str,
        value: String,
    ) -> Result<()> {
        if let Some((node, listener)) = self.tree.event(key, event_name) {
            let payload = match listener.kind {
                1 => value.into_bytes(),
                5 => vec![u8::from(value == "true")],
                10 => vec![modifiers(), 0],
                kind => return Err(format!("unsupported {event_name} listener kind {kind}")),
            };
            self.deliver(Event::Turn {
                node,
                handler: listener.handler,
                payload,
            })?;
            self.drain_private()?;
        }
        if self.select_key(key) {
            self.surface.restore_controlled_value(key)?;
        }
        Ok(())
    }

    fn enter(&mut self, focus: HWND, message: Option<&MSG>) -> Result<bool> {
        let Some(button) = self.surface.default_button(focus, message) else {
            return Ok(false);
        };
        // Suppress auto-repeat; normal key release still goes to the focused control.
        if message.is_none_or(|msg| msg.lParam & (1 << 30) == 0) {
            self.command(button, BN_CLICKED as u16)?;
        }
        Ok(true)
    }

    /// Win32 sends native notifications synchronously. Read report selection
    /// after the input message finishes, when the selected-state pair settles.
    unsafe fn report_notify(&mut self, address: LPARAM) -> Result<()> {
        // SAFETY: A nonzero WM_NOTIFY lParam points to its originating NMHDR.
        let header = unsafe { &*(address as *const NMHDR) };
        let Some(key) = self.surface.key(header.hwndFrom) else {
            return Ok(());
        };
        let Some((hwnd, report)) = self.surface.report(key) else {
            return Ok(());
        };
        if !matches!(
            header.code,
            LVN_ITEMCHANGED | NM_DBLCLK | NM_RETURN | LVN_COLUMNCLICK
        ) {
            return Ok(());
        }
        let was_idle = self.report_selection.is_empty() && self.report_actions.is_empty();
        match header.code {
            LVN_ITEMCHANGED => {
                // SAFETY: This ListView notification provides NMLISTVIEW.
                let details = unsafe { &*(address as *const NMLISTVIEW) };
                if details.uChanged & LVIF_STATE != 0
                    && (details.uNewState ^ details.uOldState) & LVIS_SELECTED != 0
                {
                    self.report_selection.insert(key);
                }
            }
            NM_DBLCLK => {
                // SAFETY: ListView double-click notifications provide NMITEMACTIVATE.
                let details = unsafe { &*(address as *const NMITEMACTIVATE) };
                if let Some(row) = report
                    .row_at(details.iItem)
                    .or_else(|| report.hit_row(hwnd, details.ptAction))
                {
                    self.report_actions.push_back((key, "activate", row.into()));
                }
            }
            NM_RETURN => {
                if let Some(row) = report.focused_or_selected(hwnd) {
                    self.report_actions.push_back((key, "activate", row.into()));
                }
            }
            LVN_COLUMNCLICK => {
                // SAFETY: This ListView notification provides NMLISTVIEW.
                let details = unsafe { &*(address as *const NMLISTVIEW) };
                if let Some(column) = report.column_at(details.iSubItem) {
                    self.report_actions
                        .push_back((key, "column-press", column.into()));
                }
            }
            _ => {}
        }
        if was_idle && !(self.report_selection.is_empty() && self.report_actions.is_empty()) {
            self.queue(REPORT_EVENTS, "queue report input")?;
        }
        Ok(())
    }

    fn drain_reports(&mut self) -> Result<()> {
        for key in std::mem::take(&mut self.report_selection) {
            self.report_selected(key)?;
        }
        while let Some((key, event, value)) = self.report_actions.pop_front() {
            self.report_event(key, event, value)?;
        }
        Ok(())
    }

    pub(crate) fn report_selected(&mut self, key: Key) -> Result<()> {
        if !self.select_key(key) {
            return Ok(());
        }
        let Some((hwnd, report)) = self.surface.report(key) else {
            return Ok(());
        };
        if report.selection_changed(hwnd) {
            let value = report.selection(hwnd).unwrap_or("").to_owned();
            self.report_event(key, "change", value)?;
            if self.select_key(key)
                && let Some((hwnd, report)) = self.surface.report(key)
            {
                report.restore(hwnd)?;
            }
        }
        Ok(())
    }

    pub(crate) fn report_event(&mut self, key: Key, name: &str, value: String) -> Result<()> {
        if !self.select_key(key) || self.surface.report(key).is_none() {
            return Ok(());
        }
        if let Some((node, listener)) = self.tree.event(key, name) {
            if listener.kind != 1 {
                return Err(format!("report event {name} requires a text listener"));
            }
            self.deliver(Event::Turn {
                node,
                handler: listener.handler,
                payload: value.into_bytes(),
            })?;
            self.drain_private()?;
        }
        Ok(())
    }

    fn queue_grid(&self) -> Result<()> {
        self.queue(GRID_EVENTS, "queue grid input")
    }

    fn drain_grid(&mut self) -> Result<()> {
        for key in std::mem::take(&mut self.focus_departures) {
            if self.select_key(key)
                && let Some((hwnd, grid)) = self.surface.grid_mut(key)
            {
                // SAFETY: Query the focus of the calling UI thread.
                if !grid.editing(unsafe { GetFocus() }) {
                    grid.cancel(hwnd, false);
                }
            }
        }
        while let Some(intent) = self.grid_intents.pop_front() {
            self.deliver_grid(intent)?;
        }
        Ok(())
    }

    fn deliver_grid(&mut self, intent: GridIntent) -> Result<()> {
        if !self.select_key(intent.grid) {
            return Ok(());
        }
        let Some((_, grid)) = self.surface.grid(intent.grid) else {
            return Ok(());
        };
        if let Some((node, listener, draft)) = grid.resolve(intent) {
            if listener.kind != 1 {
                return Err("grid edits require a text listener".into());
            }
            self.deliver(Event::Turn {
                node,
                handler: listener.handler,
                payload: draft.into_bytes(),
            })?;
            self.drain_private()?;
        }
        Ok(())
    }

    fn grid_keyboard(&mut self, handle: HWND, key: u16) -> Result<bool> {
        const V_KEY: u16 = b'V' as u16;
        if !matches!(
            key,
            VK_RETURN | VK_ESCAPE | VK_TAB | VK_F2 | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | V_KEY
        ) {
            return Ok(false);
        }
        let Some(grid_key) = self.surface.grid_key(handle) else {
            return Ok(false);
        };
        let (root, font, dpi) = self.surface.grid_coordinates();
        let (hwnd, grid) = self.surface.grid_mut(grid_key).unwrap();
        // SAFETY: The key modifiers refer to this UI thread's input queue.
        let shift = unsafe { GetKeyState(VK_SHIFT as i32) < 0 };
        let editing = grid.editing(handle);
        if key == VK_TAB {
            grid.cancel(hwnd, true);
            // SAFETY: The root and grid are live HWNDs on the UI thread.
            unsafe { SetFocus(GetNextDlgTabItem(root, hwnd, i32::from(shift))) };
        } else if editing {
            match key {
                VK_RETURN | VK_ESCAPE if grid.composition() => return Ok(false),
                VK_RETURN => {
                    if let Some(intent) = grid.commit(grid_key, hwnd) {
                        self.grid_intents.push_back(intent);
                        return self.queue_grid().map(|()| true);
                    }
                }
                VK_ESCAPE => grid.cancel(hwnd, true),
                _ => return Ok(false),
            }
        } else {
            match key {
                VK_RETURN | VK_F2 => grid.begin(hwnd, root, font)?,
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => grid.navigate(hwnd, key, shift)?,
                V_KEY if grid.accepts_paste() => {
                    // SAFETY: The modifiers refer to this UI thread's input queue.
                    let paste = unsafe {
                        GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_MENU as i32) >= 0
                    };
                    if !paste {
                        return Ok(false);
                    }
                    if let Some(intent) = grid.paste(grid_key, crate::grid_clipboard::read(root)) {
                        self.grid_intents.push_back(intent);
                        return self.queue_grid().map(|()| true);
                    }
                }
                _ => return Ok(false),
            }
        }
        grid.arrange(hwnd, root, font, dpi)?;
        Ok(true)
    }

    unsafe fn grid_notify(&mut self, address: LPARAM) -> Result<()> {
        // SAFETY: A nonzero WM_NOTIFY lParam points to an NMHDR.
        let header = unsafe { &*(address as *const NMHDR) };
        let Some(key) = self.surface.key(header.hwndFrom) else {
            return Ok(());
        };
        let (root, font, dpi) = self.surface.grid_coordinates();
        let Some((hwnd, grid)) = self.surface.grid_mut(key) else {
            return Ok(());
        };
        match header.code {
            NM_CLICK | NM_DBLCLK => {
                // SAFETY: These ListView pointer notifications provide NMITEMACTIVATE.
                let details = unsafe { &*(address as *const NMITEMACTIVATE) };
                if grid.click(hwnd, details.ptAction)? && header.code == NM_DBLCLK {
                    grid.begin(hwnd, root, font)?;
                }
            }
            LVN_ENDSCROLL => grid.position_editor(hwnd, root),
            _ => return Ok(()),
        }
        grid.arrange(hwnd, root, font, dpi)?;
        Ok(())
    }

    fn dropped(&mut self, point: Option<POINT>, paths: Vec<PathBuf>) -> Result<()> {
        let target = point
            .and_then(|point| self.surface.drop_target(&self.tree, point))
            .unwrap_or(self.surface.selected());
        if let Some((node, listener)) = self.tree.event(target, "drop") {
            self.effects.deliver_paths(node, listener.handler, paths)?;
        }
        Ok(())
    }
}

pub(crate) const APPLY_PENDING_EDIT: u32 = WM_APP + 3;
const APPLY_TREE_INTENT: u32 = WM_APP + 5;

fn read_reference(op: u8, reader: &mut Reader<'_>) -> Result<()> {
    match op {
        1..=5 => {}
        6 => {
            reader.raw(3)?;
        }
        7 => {
            for _ in 0..2 {
                if reader.u8()? == 3 {
                    reader.u32()?;
                }
            }
            reader.u8()?;
        }
        _ => return Err(format!("unknown reference operation {op}")),
    }
    Ok(())
}

pub(crate) fn enabled_flag(value: &str) -> bool {
    value.is_empty() || value == "true" || value == "1"
}

fn effect_event(payload: Vec<u8>) -> Event {
    Event::Turn {
        node: EFFECT_NODE,
        handler: 0,
        payload,
    }
}

fn modifiers() -> u8 {
    // SAFETY: These are documented virtual-key constants.
    unsafe {
        u8::from(GetKeyState(i32::from(VK_SHIFT)) < 0)
            | u8::from(GetKeyState(i32::from(VK_CONTROL)) < 0) << 1
            | u8::from(GetKeyState(i32::from(VK_MENU)) < 0) << 2
    }
}

fn live_boot() -> Result<(i64, [u8; 16], String, i32)> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?;
    let mut seed = [0; 16];
    // SAFETY: The flag selects the system RNG, and the destination buffer is writable.
    if unsafe {
        BCryptGenRandom(
            null_mut(),
            seed.as_mut_ptr(),
            seed.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } != 0
    {
        return Err("could not obtain random seed from Windows".into());
    }
    Ok((now.as_millis() as i64, seed, language(), zone_offset()))
}

fn language() -> String {
    let mut locale = [0_u16; 85]; // LOCALE_NAME_MAX_LENGTH
    // SAFETY: The OS receives the full length of a writable array.
    let len = unsafe { GetUserDefaultLocaleName(locale.as_mut_ptr(), locale.len() as i32) };
    if len > 1 {
        String::from_utf16_lossy(&locale[..len as usize - 1])
    } else {
        "en-US".into()
    }
}

fn zone_offset() -> i32 {
    let mut info = TIME_ZONE_INFORMATION::default();
    // SAFETY: The function initializes the passed timezone record.
    match unsafe { GetTimeZoneInformation(&mut info) } {
        TIME_ZONE_ID_INVALID => 0,
        TIME_ZONE_ID_DAYLIGHT => -(info.Bias + info.DaylightBias),
        _ => -(info.Bias + info.StandardBias),
    }
}

fn paths_from_drop(drop: HDROP) -> (Option<POINT>, Vec<PathBuf>) {
    let mut paths = Vec::new();
    let mut point = POINT::default();
    let client;
    // SAFETY: The drop handle came from WM_DROPFILES; each UTF-16 buffer
    // includes room for the nul terminator, and DragFinish consumes it.
    unsafe {
        client = DragQueryPoint(drop, &mut point) != 0;
        let count = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
        for index in 0..count {
            let len = DragQueryFileW(drop, index, null_mut(), 0);
            let mut name = vec![0; len as usize + 1];
            let len = DragQueryFileW(drop, index, name.as_mut_ptr(), name.len() as u32);
            if len != 0 {
                paths.push(PathBuf::from(OsString::from_wide(&name[..len as usize])));
            }
        }
        DragFinish(drop);
    }
    (client.then_some(point), paths)
}

fn embedded_display_name(instance: HMODULE) -> Option<String> {
    // Keep the RT_RCDATA identifier in sync with display_name_resource in the CLI.
    const RT_RCDATA: *const u16 = 10usize as *const u16;
    const DISPLAY_NAME: *const u16 = 501usize as *const u16;
    // SAFETY: The module is the current executable and remains loaded.
    // Integer resource identifiers are passed as low-word pointer values.
    unsafe {
        let resource = FindResourceW(instance, DISPLAY_NAME, RT_RCDATA);
        if resource.is_null() {
            return None;
        }
        let len = SizeofResource(instance, resource) as usize;
        if !(1..=64).contains(&len) {
            return None;
        }
        let loaded = LoadResource(instance, resource);
        if loaded.is_null() {
            return None;
        }
        let data = LockResource(loaded);
        if data.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), len);
        if bytes.iter().all(|&byte| (b' '..=b'~').contains(&byte)) {
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        } else {
            None
        }
    }
}

fn app_name(instance: HMODULE) -> String {
    if let Some(title) = embedded_display_name(instance) {
        return title;
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Roc application".into())
}

pub(crate) fn report_error(error: &str) {
    eprintln!("roc-native-windows: {error}");
    if std::env::var_os("ROC_WINDOWS_SPEC").is_some() {
        return;
    }
    let text = wide(error);
    // SAFETY: Both strings are terminated and live for the modal call.
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            w!("Roc application stopped"),
            MB_ICONERROR | MB_OK,
        )
    };
}

fn last_error(operation: &str) -> String {
    format!("{operation}: {}", std::io::Error::last_os_error())
}

pub(crate) fn create_window(title: &str) -> Result<OwnedWindow> {
    let title = wide(title);
    // SAFETY: The class and caption are valid terminated strings on this UI thread.
    let handle = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            WINDOW_CLASS,
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            520,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null::<c_void>(),
        )
    };
    if handle.is_null() {
        Err(last_error("create Roc window"))
    } else {
        Ok(OwnedWindow(handle))
    }
}

fn create_pump() -> Result<OwnedWindow> {
    // SAFETY: The message-only HWND and timer live for the entire app.
    unsafe {
        let pump = CreateWindowExW(
            0,
            WINDOW_CLASS,
            w!(""),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if pump.is_null() {
            return Err(last_error("create the Roc event pump"));
        }
        let pump = OwnedWindow(pump);
        if SetTimer(pump.0, PUMP, 15, None) == 0 {
            return Err(last_error("start the Roc event pump"));
        }
        Ok(pump)
    }
}

fn run() -> Result<()> {
    // SAFETY: Register the class once on the UI thread, before mounting.
    let title = unsafe {
        let instance = GetModuleHandleW(null());
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES
                | ICC_DATE_CLASSES
                | ICC_LISTVIEW_CLASSES
                | ICC_TAB_CLASSES
                | ICC_PROGRESS_CLASS
                | ICC_BAR_CLASSES
                | ICC_TREEVIEW_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err(last_error("initialize Windows common controls"));
        }
        let window_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: WINDOW_CLASS,
            ..WNDCLASSEXW::default()
        };
        if RegisterClassExW(&window_class) == 0 {
            return Err(last_error("register Roc window class"));
        }
        app_name(instance)
    };
    let window = create_window(&title)?;
    let result = if let Some(path) = std::env::var_os("ROC_WINDOWS_SPEC") {
        spec_run::run(window, &PathBuf::from(path))
    } else {
        mount(window, None).and_then(|()| run_message_loop())
    };
    clear_app();
    if let Some(error) = FAILURE.with(|failure| failure.borrow_mut().take()) {
        return Err(error);
    }
    result
}

fn run_message_loop() -> Result<()> {
    let mut message = MSG::default();
    // SAFETY: Translate and dispatch messages from this thread's own queue.
    unsafe {
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err(last_error("read the Windows message queue")),
                0 if message.wParam == 0 => return Ok(()),
                0 => return Err("Windows event loop terminated with an error".into()),
                _ => {}
            }
            let (owner, native, handled) = route_key(&message)?;
            if handled {
                continue;
            }
            if owner.is_null() || native || IsDialogMessageW(owner, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

fn route_key(message: &MSG) -> Result<(HWND, bool, bool)> {
    let unrelated = (null_mut(), false, false);
    if !(WM_KEYFIRST..=WM_KEYLAST).contains(&message.message) {
        return Ok(unrelated);
    }
    APP.with(|state| {
        let mut state = state.borrow_mut();
        let Some(app) = state.as_mut() else {
            return Ok(unrelated);
        };
        if !app.select_message(message.hwnd) {
            return Ok(unrelated);
        }
        let id = app.surface.selected();
        let owner = app.surface.handle(id).expect("native route is mounted");
        if app.surface.blocked(id) {
            return Ok((owner, false, true));
        }
        if message.message != WM_KEYDOWN {
            return Ok((owner, false, false));
        }
        let key = message.wParam as u16;
        if app.grid_keyboard(message.hwnd, key)? {
            return Ok((owner, false, true));
        }
        if key == VK_ESCAPE && app.surface.is_dialog(id) && modifiers() == 0 {
            // SAFETY: The calling UI thread owns the focused edit, if any.
            let focus = unsafe { GetFocus() };
            if modal::takes_escape(focus, message, composing(focus)) {
                return Ok((owner, true, false));
            }
            if message.lParam & (1 << 30) == 0 {
                app.request_close(id)?;
            }
            return Ok((owner, false, true));
        }
        let native = (key == VK_RETURN
            && app
                .surface
                .key(message.hwnd)
                .is_some_and(|view| app.surface.report(view).is_some()))
            || (matches!(
                key,
                VK_RETURN | VK_SPACE | VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN
            ) && app.surface.toolbar(message.hwnd).is_some());
        let handled = if !native && key == VK_RETURN && modifiers() == 0 {
            // SAFETY: Query focus on the target surface's UI thread.
            app.enter(unsafe { GetFocus() }, Some(message))?
        } else {
            false
        };
        Ok((owner, native, handled))
    })
}

fn mount(window: OwnedWindow, options: Option<SpecOptions>) -> Result<()> {
    APP.with(|state| {
        // The borrow suppresses synchronous notifications during mount.
        let mut state = state.borrow_mut();
        *state = Some(App::start(window, options)?);
        Ok(())
    })
}

fn clear_app() {
    APP.with(|state| {
        let mut state = state.borrow_mut();
        drop(state.take());
    });
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
extern "C" fn main(_argc: i32, _argv: *mut *mut u8) -> i32 {
    crate::abi::register_main_thread();
    match run() {
        Ok(()) => 0,
        Err(error) => {
            report_error(&error);
            1
        }
    }
}

pub(crate) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if Taskbar::queue_if_button_created(hwnd, message) {
        return 0;
    }
    // SAFETY: The OS calls this on its window's UI thread with documented
    // WPARAM/LPARAM payloads. Nested callbacks skip app state if busy.
    unsafe {
        let theme_change = matches!(
            message,
            WM_THEMECHANGED | WM_SYSCOLORCHANGE | WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED
        );
        if message == WM_NOTIFY && lparam != 0 {
            let header = &*(lparam as *const NMHDR);
            if header.code == NM_CUSTOMDRAW {
                let drawn = APP.with(|state| {
                    let state = state.try_borrow().ok()?;
                    let app = state.as_ref()?;
                    let id = app.surface.for_handle(hwnd)?;
                    let surface = app.surface.at(id)?;
                    let key = surface.key(header.hwndFrom)?;
                    let (report, grid) = surface.grid(key)?;
                    let details = &*(lparam as *const NMCUSTOMDRAW);
                    Some(grid.draw(report, details))
                });
                if let Some(result) = drawn {
                    return result;
                }
            }
        }
        match message {
            WM_CLOSE
            | WM_COMMAND
            | WM_NOTIFY
            | REPORT_EVENTS
            | GRID_EVENTS
            | CLOSE_EVENTS
            | WM_SIZE
            | WM_TIMER
            | WM_DPICHANGED
            | WM_VSCROLL
            | WM_MOUSEWHEEL
            | WM_DROPFILES
            | WM_THEMECHANGED
            | WM_SYSCOLORCHANGE
            | WM_SETTINGCHANGE
            | WM_DWMCOLORIZATIONCOLORCHANGED
            | APPLY_PENDING_EDIT
            | APPLY_TREE_INTENT => {
                let paths = (message == WM_DROPFILES).then(|| paths_from_drop(wparam as HDROP));
                let handled = APP.with(|state| {
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return Ok(false);
                    };
                    let Some(app) = &mut *state else {
                        return Ok(false);
                    };
                    if hwnd == app.pump.0 {
                        match message {
                            WM_TIMER if wparam == PUMP => app.poll()?,
                            REPORT_EVENTS => app.drain_reports()?,
                            GRID_EVENTS => app.drain_grid()?,
                            CLOSE_EVENTS => app.drain_closes()?,
                            APPLY_TREE_INTENT => {
                                if app.select_key(wparam)
                                    && let Some(intent) = app.surface.tree_intent(wparam)
                                {
                                    app.tree_event(wparam, intent)?;
                                }
                            }
                            _ => return Ok(false),
                        }
                        return Ok(true);
                    }
                    if theme_change {
                        if let Some(surface) = app
                            .surface
                            .for_handle(hwnd)
                            .and_then(|id| app.surface.at_mut(id))
                        {
                            surface.theme_changed(&app.tree)?;
                        }
                        // Let standard controls and dialogs refresh their native theme resources.
                        return Ok(false);
                    }
                    if !app.select_message(hwnd) {
                        return Ok(false);
                    }
                    let id = app.surface.selected();
                    if app.surface.blocked(id)
                        && matches!(
                            message,
                            WM_CLOSE
                                | WM_COMMAND
                                | WM_NOTIFY
                                | WM_VSCROLL
                                | WM_MOUSEWHEEL
                                | WM_DROPFILES
                        )
                    {
                        return Ok(true);
                    }
                    match message {
                        WM_CLOSE => app.request_close(id)?,
                        WM_COMMAND if lparam != 0 && !app.driven => {
                            let source = lparam as HWND;
                            let notification = (wparam >> 16) as u16;
                            if u32::from(notification) == BN_CLICKED
                                && app.surface.toolbar(source).is_some()
                            {
                                app.toolbar_command(source, wparam as u16)?;
                            } else {
                                app.command(source, notification)?;
                            }
                        }
                        APPLY_PENDING_EDIT => app.surface.settle_edit(lparam as HWND),
                        WM_NOTIFY if lparam != 0 && !app.driven => {
                            let header = &*(lparam as *const NMHDR);
                            match header.code {
                                TVN_SELCHANGEDW | TVN_ITEMEXPANDINGW | TVN_ITEMEXPANDEDW => {
                                    if let Some(key) = app.surface.key(header.hwndFrom)
                                        && app.surface.kind(key) == crate::view::Control::TreeView
                                    {
                                        let native = &*(lparam as *const NMTREEVIEWW);
                                        if app.surface.tree_notification(
                                            key,
                                            header.code,
                                            native.itemNew.hItem,
                                            native.action,
                                        ) && PostMessageW(app.pump.0, APPLY_TREE_INTENT, key, 0)
                                            == 0
                                        {
                                            return Err(last_error("dispatch native tree intent"));
                                        }
                                    }
                                }
                                TCN_SELCHANGE => app.notify(header.hwndFrom, header.code)?,
                                DTN_DATETIMECHANGE => {
                                    if let Some(key) = app.surface.key(header.hwndFrom)
                                        && let Some(mode) = app.surface.kind(key).civil()
                                    {
                                        let value = civil::native::notification(lparam, mode)?;
                                        app.civil_change(key, value)?;
                                    }
                                }
                                _ => {
                                    app.report_notify(lparam)?;
                                    app.grid_notify(lparam)?;
                                }
                            }
                        }
                        WM_SIZE => app.surface.arrange(&app.tree)?,
                        WM_DPICHANGED => {
                            let rect = &*(lparam as *const RECT);
                            SetWindowPos(
                                hwnd,
                                null_mut(),
                                rect.left,
                                rect.top,
                                rect.right - rect.left,
                                rect.bottom - rect.top,
                                SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                            app.surface.dpi_changed(&app.tree)?;
                            app.surface.sync_icons(id);
                        }
                        WM_VSCROLL => app.surface.scroll(&app.tree, wparam as u16, None)?,
                        WM_MOUSEWHEEL => {
                            app.surface
                                .scroll(&app.tree, 0, Some((wparam >> 16) as i16))?
                        }
                        WM_DROPFILES if !app.driven => {
                            let (point, files) = paths.unwrap_or_default();
                            app.dropped(point, files)?;
                        }
                        _ => return Ok::<_, String>(false),
                    }
                    Ok(true)
                });
                match handled {
                    Ok(true) => return 0,
                    Ok(false) if message == WM_CLOSE => return 0,
                    Ok(false) => {}
                    Err(error) => {
                        fail_thread(error);
                        return 0;
                    }
                }
            }
            WM_NCDESTROY => {
                let unexpected = APP.with(|state| {
                    state
                        .try_borrow_mut()
                        .ok()
                        .and_then(|mut state| {
                            state.as_mut().map(|app| app.surface.forget_destroyed(hwnd))
                        })
                        .unwrap_or(false)
                });
                if unexpected {
                    fail_thread(
                        "a native window was destroyed outside the mounted lifecycle".into(),
                    );
                }
            }
            _ => {}
        }
        modal::dialog_default(hwnd, message, wparam, lparam)
            .unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam))
    }
}

fn fail_thread(error: String) {
    FAILURE.with(|failure| {
        let mut failure = failure.borrow_mut();
        if failure.is_none() {
            *failure = Some(error);
        }
    });
    // SAFETY: Quit is posted only on the host's UI thread.
    unsafe { PostQuitMessage(1) };
}
