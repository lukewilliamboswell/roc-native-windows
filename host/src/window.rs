use crate::abi::Engine;
use crate::app_resource::embedded_app_data;
use crate::civil_calendar::native::{Action as CalendarAction, Calendar};
use crate::civil_edit::Commit as CivilCommit;
use crate::display_preferences::DisplaySettings;
use crate::effects::{Effects, SpecOptions};
use crate::grid::Intent as GridIntent;
use crate::modal;
use crate::raster_bridge::{Attachment, Bridge as RasterBridge, Update as RasterUpdate};
use crate::taskbar::native::Taskbar;
use crate::tree_view::TreeIntent;
use crate::view::{composing, wide};
use crate::windows::{OwnedWindow, SpecSurface, SurfaceRegistry};
use crate::wire::{Declarations, EFFECT_NODE, Key, Reader, Tree};
use crate::{Event, Result};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{Receiver, channel};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES,
    ICC_TAB_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVIF_STATE,
    LVIS_SELECTED, LVN_COLUMNCLICK, LVN_ENDSCROLL, LVN_ITEMCHANGED, NM_CLICK, NM_CUSTOMDRAW,
    NM_DBLCLK, NM_RETURN, NMCUSTOMDRAW, NMHDR, NMITEMACTIVATE, NMLISTVIEW, NMTREEVIEWW,
    TCN_SELCHANGE, TVN_ITEMEXPANDEDW, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetActiveWindow, GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F2,
    VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

#[path = "spec_run.rs"]
mod spec_run;

#[path = "input_runtime.rs"]
mod input_runtime;
use input_runtime::InputState;
pub(crate) use input_runtime::queue_surface_mouse;

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
    calendars: BTreeMap<Key, Calendar>,
    surface: SurfaceRegistry,
    pump: OwnedWindow,
    close_requests: VecDeque<(Key, usize)>,
    input: InputState,
    quit_posted: bool,
    effects: Effects,
    rasters: RasterBridge,
    incoming: Receiver<Event>,
    private: VecDeque<Event>,
    opportunity: Option<u64>,
    report_selection: BTreeSet<Key>,
    report_actions: VecDeque<(Key, &'static str, String)>,
    grid_intents: VecDeque<GridIntent>,
    focus_departures: BTreeSet<Key>,
    driven: bool,
    display_settings: DisplaySettings,
}

impl App {
    fn start(window: OwnedWindow, options: Option<SpecOptions>) -> Result<Self> {
        let (send, incoming) = channel();
        let driven = options.is_some();
        crate::shader::configure(driven);
        let pump = create_pump()?;
        let title = app_name();
        let display_settings = DisplaySettings::new();
        let declarations = Declarations::decode(&Engine::declarations()?)?;
        let effects = Effects::new(send, declarations.clone())?;
        let mut app = Self {
            engine: Engine::default(),
            tree: Tree::new(),
            calendars: BTreeMap::new(),
            surface: SurfaceRegistry::new(window, title, driven, display_settings.read()),
            pump,
            close_requests: VecDeque::new(),
            input: InputState::default(),
            quit_posted: false,
            effects,
            rasters: RasterBridge::default(),
            incoming,
            private: VecDeque::new(),
            opportunity: None,
            report_selection: BTreeSet::new(),
            report_actions: VecDeque::new(),
            grid_intents: VecDeque::new(),
            focus_departures: BTreeSet::new(),
            driven,
            display_settings,
        };
        if let Some(options) = options {
            app.effects.configure_spec(options)?;
        }
        let facts = app.effects.snapshot(true)?;
        let boot =
            declarations.boot(&facts, |session, key| app.effects.boot_value(session, key))?;
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
        crate::accessibility::disconnect_retired();
        if self.exited() {
            return Ok(());
        }
        self.drain_input()?;
        if self.exited() {
            return Ok(());
        }
        if self.display_settings.changed() {
            let settings = self.display_settings.read();
            self.surface.display_preferences(&self.tree, settings)?;
            for calendar in self.calendars.values_mut() {
                calendar.settings_changed(settings)?;
            }
        }
        self.surface.poll();
        self.automation_actions()?;
        self.effects.poll();
        let (images, resized) = self.rasters.poll()?;
        self.publish_rasters(images)?;
        for update in resized {
            if let Some(owner) = self.surface.for_key(update.key)
                && let Some(surface) = self.surface.at_mut(owner)
            {
                surface.accept_image_resize(update.key, update.source, update.image)?;
            }
        }
        for surface in self.surface.surfaces_mut() {
            surface.reveal_focus(&self.tree, false)?;
            for (key, resize) in surface.image_resizes() {
                self.rasters.resize(key, resize)?;
            }
        }
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
        let mut attachments = Vec::new();
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
                19 => attachments.push(Attachment::read(&mut reader)?),
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
        self.reconcile_calendars()?;
        self.surface.sync(&self.tree)?;
        self.reconcile_input();
        let (bindings, mut images) = self.rasters.sync(&self.tree)?;
        self.private.extend(bindings);
        for attachment in attachments {
            if let Some(update) = self.rasters.attach(&self.tree, attachment)? {
                images.push(update);
            }
        }
        self.publish_rasters(images)?;
        for (op, id) in references {
            if let Some(key) = self.tree.reference(id)
                && let Some(owner) = self.surface.for_key(key)
                && !self.surface.blocked(owner)
                && let Some(surface) = self.surface.at_mut(owner)
            {
                surface.reference(&self.tree, op, key)?;
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

    fn publish_rasters(&mut self, images: Vec<RasterUpdate>) -> Result<()> {
        let mut changed = false;
        for (key, pixels) in images {
            if let Some(owner) = self.surface.for_key(key)
                && let Some(surface) = self.surface.at_mut(owner)
            {
                surface.set_raster(key, pixels);
                changed = true;
            }
        }
        if changed {
            self.surface.sync(&self.tree)?;
        }
        Ok(())
    }

    fn work_done(&self) -> bool {
        self.effects.work_done() && self.rasters.ready() && crate::shader::ready()
    }

    fn command(&mut self, control: HWND, notification: u16) -> Result<()> {
        if u32::from(notification) == BN_CLICKED && self.surface.calendar_origin(control).is_some()
        {
            return self.open_calendar(control);
        }
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
            (Control::Button, BN_CLICKED) => {
                if let Some(text) = self.tree.node(key).attr("host-copy") {
                    if self.surface.spec_disabled(key)?
                        || self.surface.blocked(self.surface.selected())
                    {
                        return Ok(());
                    }
                    let (owner, _) = self.surface.grid_coordinates();
                    let report = self.effects.copy_from_button(owner, text);
                    return self.control_event(key, "host-copy-result", report.into());
                }
                ("click", String::new())
            }
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
                    let (owner, _) = self.surface.grid_coordinates();
                    self.effects.pick(owner, node, listener.handler, multiple)?;
                }
                return Ok(());
            }
            (Control::FolderButton, BN_CLICKED) => {
                if let Some((node, listener)) = self.tree.event(key, "change") {
                    let (owner, _) = self.surface.grid_coordinates();
                    self.effects.pick_folder(owner, node, listener.handler)?;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.control_event(key, event_name, value)
    }

    fn reconcile_calendars(&mut self) -> Result<()> {
        if self.calendars.is_empty() {
            return Ok(());
        }
        let retiring = self.surface.retiring_calendars(
            &self.tree,
            self.calendars
                .iter()
                .map(|(&owner, calendar)| (owner, calendar.origin.field)),
        )?;
        for owner in retiring {
            self.calendars.remove(&owner).unwrap().close(true);
        }
        Ok(())
    }

    fn open_calendar(&mut self, button: HWND) -> Result<()> {
        let Some((origin, _, _)) = self.surface.calendar_origin(button) else {
            return Ok(());
        };
        let surface = self.surface.selected();
        if self.calendars.contains_key(&surface) || self.surface.blocked(surface) {
            return Ok(());
        }
        if !composing(origin.edit) {
            self.commit_civil(origin.edit)?;
        }
        if !self.select_key(origin.field) || self.surface.selected() != surface {
            return Ok(());
        }
        let Some((current, label, date)) = self.surface.calendar_origin(button) else {
            return Ok(());
        };
        if current.edit != origin.edit || current.field != origin.field {
            return Ok(());
        }
        let title = if label.is_empty() {
            "Choose date".into()
        } else {
            format!("Choose date for {label}")
        };
        // SAFETY: Focus the live opener only when its window is active.
        unsafe {
            if GetActiveWindow() == origin.owner {
                SetFocus(button);
            }
        }
        let calendar = Calendar::open(
            origin,
            &title,
            date,
            self.display_settings.read(),
            Some(window_proc),
        )?;
        self.calendars.insert(surface, calendar);
        self.surface.refresh_automation_interaction();
        Ok(())
    }

    fn calendar_owner(&self, handle: HWND) -> Option<Key> {
        // SAFETY: A native message's HWND belongs to the current UI thread.
        let dialog = unsafe { GetAncestor(handle, GA_ROOT) };
        self.calendars
            .iter()
            .find_map(|(&owner, calendar)| (calendar.handle() == dialog).then_some(owner))
    }

    fn calendar_action(&mut self, owner: Key, action: CalendarAction) -> Result<bool> {
        let value = match action {
            CalendarAction::Ignore => return Ok(false),
            CalendarAction::Handled => return Ok(true),
            CalendarAction::Close => None,
            CalendarAction::Choose(value) => Some(value),
        };
        let calendar = self.calendars.remove(&owner).unwrap();
        let origin = calendar.origin;
        calendar.close(true);
        self.surface.refresh_automation_interaction();
        if let Some(value) = value
            && self.select_key(origin.field)
            && self.surface.selected() == owner
            && self.surface.propose_calendar_date(origin, value)
        {
            self.commit_civil(origin.edit)?;
        }
        Ok(true)
    }

    fn commit_civil(&mut self, handle: HWND) -> Result<bool> {
        let Some((key, commit)) = self.surface.civil_commit(handle) else {
            return Ok(false);
        };
        match commit {
            CivilCommit::Clean => return Ok(false),
            CivilCommit::Edited => {}
            CivilCommit::Changed(value) => self.control_event(key, "change", value.to_string())?,
        }
        if self.select_key(key) && self.surface.spec_handle(key) == Some(handle) {
            if matches!(commit, CivilCommit::Changed(_)) {
                // The Roc handler may decline the proposed typed value.
                self.surface.civil_cancel(handle);
            }
            self.surface.arrange(&self.tree)?;
            self.surface.reveal_focus(&self.tree, true)?;
        }
        Ok(true)
    }

    fn cancel_civil(&mut self, handle: HWND) -> Result<bool> {
        let changed = self.surface.civil_cancel(handle);
        if changed {
            self.surface.arrange(&self.tree)?;
            self.surface.reveal_focus(&self.tree, true)?;
        }
        Ok(changed)
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

    fn slider_changed(&mut self, source: HWND) -> Result<()> {
        if let Some(key) = self.surface.key(source)
            && let Some(value) = self.surface.slider_change(key)
        {
            self.drain_input()?;
            if self.select_key(key) && self.surface.spec_handle(key) == Some(source) {
                self.control_event(key, "change", value.to_string())?;
            }
        }
        Ok(())
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

    fn automation_actions(&mut self) -> Result<()> {
        let actions: Vec<_> = self
            .surface
            .surfaces_mut()
            .flat_map(crate::view::Surface::automation_actions)
            .collect();
        let previous = self.surface.selected();
        for (key, action) in actions {
            if !self.select_key(key) {
                continue;
            }
            if let Some((hwnd, grid)) = self.surface.grid_mut(key) {
                if let Some(intent) = grid.automation_action(key, hwnd, action)? {
                    self.deliver_grid(intent)?;
                } else {
                    self.surface.arrange(&self.tree)?;
                }
            }
        }
        self.surface.select(previous);
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
        let (root, font) = self.surface.grid_coordinates();
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
                    if let Some(intent) = grid.paste_native(grid_key, root) {
                        self.grid_intents.push_back(intent);
                        return self.queue_grid().map(|()| true);
                    }
                }
                _ => return Ok(false),
            }
        }
        self.surface.arrange(&self.tree)?;
        Ok(true)
    }

    unsafe fn grid_notify(&mut self, address: LPARAM) -> Result<()> {
        // SAFETY: A nonzero WM_NOTIFY lParam points to an NMHDR.
        let header = unsafe { &*(address as *const NMHDR) };
        let Some(key) = self.surface.key(header.hwndFrom) else {
            return Ok(());
        };
        let (root, font) = self.surface.grid_coordinates();
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
            LVN_ENDSCROLL => {
                grid.position_editor(hwnd, root);
                grid.publish_automation(hwnd);
                return Ok(());
            }
            _ => return Ok(()),
        }
        self.surface.arrange(&self.tree)
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

pub(crate) fn paths_from_drop(drop: HDROP) -> (Option<POINT>, Vec<PathBuf>) {
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

fn scenario_runner() -> Result<bool> {
    const MARKER: &[u8] = b"roc-windows-scenario-1";
    match embedded_app_data(504, MARKER.len())? {
        None => Ok(false),
        Some(bytes) if bytes == MARKER => Ok(true),
        Some(_) => Err("invalid embedded scenario marker".into()),
    }
}

fn app_name() -> String {
    if let Ok(Some(bytes)) = embedded_app_data(501, 64)
        && bytes.iter().all(|&byte| (b' '..=b'~').contains(&byte))
        && let Ok(title) = std::str::from_utf8(bytes)
    {
        return title.to_owned();
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
    if scenario_runner().unwrap_or(false) {
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
    let driven = scenario_runner()?;
    let _accessibility = crate::accessibility::Apartment::new()?;
    // SAFETY: Register the class once on the UI thread, before mounting.
    let title = unsafe {
        let instance = GetModuleHandleW(null());
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES
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
        app_name()
    };
    let window = create_window(&title)?;
    let result = if driven {
        let path = std::env::var_os("ROC_WINDOWS_SPEC")
            .ok_or("launch scenario executables through windows.roc test or record")?;
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
            crate::accessibility::disconnect_retired();
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
        if let Some(owner) = app.calendar_owner(message.hwnd) {
            let calendar = app.calendars.get_mut(&owner).unwrap();
            let dialog = calendar.handle();
            let handled = if message.message == WM_KEYDOWN && modifiers() == 0 {
                let action =
                    calendar.key(message.wParam as u16, message.lParam & (1 << 30) != 0)?;
                app.calendar_action(owner, action)?
            } else {
                false
            };
            return Ok((dialog, false, handled));
        }
        if !app.select_message(message.hwnd) {
            return Ok(unrelated);
        }
        let id = app.surface.selected();
        let owner = app.surface.handle(id).expect("native route is mounted");
        if app.surface.blocked(id) || app.calendars.contains_key(&id) {
            return Ok((owner, false, true));
        }
        if app.surface_key(message)? {
            return Ok((owner, false, true));
        }
        if message.message != WM_KEYDOWN {
            return Ok((owner, false, false));
        }
        let key = message.wParam as u16;
        let modifiers = modifiers();
        let plain = modifiers == 0;
        // SAFETY: Query the focus from this UI thread, before native handlers run.
        let focus = unsafe { GetFocus() };
        if plain
            && matches!(key, VK_RETURN | VK_ESCAPE)
            && (app.surface.editor_composing(focus) || modal::has_open_popup(focus))
        {
            return Ok((owner, true, false));
        }
        if key == VK_RETURN && plain && app.surface.calendar_origin(focus).is_some() {
            if message.lParam & (1 << 30) == 0 {
                app.open_calendar(focus)?;
            }
            return Ok((owner, false, true));
        }
        if app.grid_keyboard(message.hwnd, key)? {
            return Ok((owner, false, true));
        }
        // Commit before the dialog manager chooses the next Tab stop. A date
        // handler can enable a following field or move focus explicitly.
        if key == VK_TAB
            && modifiers <= 1
            && !app.surface.editor_composing(focus)
            && app.commit_civil(focus)?
            && unsafe { GetFocus() } != focus
        {
            return Ok((owner, false, true));
        }
        if plain && matches!(key, VK_RETURN | VK_ESCAPE) {
            let handled = if key == VK_RETURN {
                app.commit_civil(focus)?
            } else {
                app.cancel_civil(focus)?
            };
            if handled {
                return Ok((owner, false, true));
            }
        }
        if key == VK_ESCAPE && app.surface.is_dialog(id) && plain {
            if modal::takes_escape(focus, message) {
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
        let handled = if !native && key == VK_RETURN && plain {
            app.enter(focus, Some(message))?
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
    crate::shader::shutdown();
    crate::accessibility::disconnect_retired();
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
            | WM_HSCROLL
            | WM_MOUSEWHEEL
            | WM_DROPFILES
            | WM_THEMECHANGED
            | WM_SYSCOLORCHANGE
            | WM_SETTINGCHANGE
            | WM_DWMCOLORIZATIONCOLORCHANGED
            | APPLY_PENDING_EDIT
            | APPLY_TREE_INTENT => {
                let paths = (message == WM_DROPFILES)
                    .then(|| crate::effects::take_drop(wparam as HDROP))
                    .flatten();
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
                    if let Some(owner) = app.calendar_owner(hwnd) {
                        let settings = app.display_settings.read();
                        let calendar = app.calendars.get_mut(&owner).unwrap();
                        let action = match message {
                            WM_CLOSE => CalendarAction::Close,
                            WM_COMMAND if (wparam >> 16) as u32 == BN_CLICKED => {
                                calendar.command(lparam as HWND, wparam as u16)?
                            }
                            WM_DPICHANGED | WM_SETTINGCHANGE => {
                                calendar.settings_changed(settings)?;
                                CalendarAction::Ignore
                            }
                            _ => CalendarAction::Ignore,
                        };
                        return app.calendar_action(owner, action);
                    }
                    if theme_change {
                        if message == WM_SETTINGCHANGE {
                            app.surface
                                .display_preferences(&app.tree, app.display_settings.read())?;
                        }
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
                    if (app.surface.blocked(id) || app.calendars.contains_key(&id))
                        && matches!(
                            message,
                            WM_CLOSE
                                | WM_COMMAND
                                | WM_NOTIFY
                                | WM_VSCROLL
                                | WM_HSCROLL
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
                        APPLY_PENDING_EDIT => {
                            let handle = lparam as HWND;
                            app.surface.settle_edit(handle);
                            if GetFocus() != handle && !composing(handle) {
                                app.commit_civil(handle)?;
                            }
                        }
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
                                _ => {
                                    app.report_notify(lparam)?;
                                    app.grid_notify(lparam)?;
                                }
                            }
                        }
                        WM_SIZE => {
                            app.surface.arrange(&app.tree)?;
                            app.surface.reveal_focus(&app.tree, true)?;
                        }
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
                        WM_HSCROLL if lparam != 0 && !app.driven => {
                            app.slider_changed(lparam as HWND)?
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
