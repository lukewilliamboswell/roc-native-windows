use crate::Result;
use crate::window_chrome::{WindowDeclaration, elements};
use crate::wire::{Key, Kind, ROOT, Tree};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Title {
    App,
    Text(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartupPosition {
    #[default]
    System,
    Centered,
    At(i32, i32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartupState {
    #[default]
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Startup {
    position: StartupPosition,
    state: StartupState,
}

impl Startup {
    fn read(position: Option<&str>, state: Option<&str>) -> Result<Self> {
        let position = match position {
            None | Some("system") => StartupPosition::System,
            Some("centered") => StartupPosition::Centered,
            Some(point) => {
                let invalid = || "invalid startup window position".to_owned();
                let (x, y) = point.split_once(',').ok_or_else(invalid)?;
                StartupPosition::At(
                    x.parse().map_err(|_| invalid())?,
                    y.parse().map_err(|_| invalid())?,
                )
            }
        };
        let state = match state {
            None | Some("normal") => StartupState::Normal,
            Some("minimized") => StartupState::Minimized,
            Some("maximized") => StartupState::Maximized,
            Some("fullscreen") => StartupState::Fullscreen,
            Some(_) => return Err("invalid startup window state".into()),
        };
        Ok(Self { position, state })
    }
}

#[derive(Clone)]
struct Declaration {
    id: Key,
    wrapper: Option<Key>,
    content: Key,
    owner: Option<Key>,
    title: Title,
    request: Option<Key>,
    startup: Startup,
}

struct WindowPlan {
    managed: bool,
    declarations: Vec<Declaration>,
    scopes: BTreeMap<Key, Key>,
}

impl WindowPlan {
    fn read(tree: &Tree, previous_mode: Option<bool>) -> Result<Self> {
        let roots = elements(tree, ROOT).collect::<Vec<_>>();
        let collection = roots
            .iter()
            .find_map(|&(key, tag)| (tag == "ui-windows").then_some(key));
        let managed = collection.is_some();
        if previous_mode.is_some_and(|old| old != managed) {
            return Err("an app's outermost window mode cannot change while it runs".into());
        }

        let mut plan = Self {
            managed,
            declarations: Vec::new(),
            scopes: BTreeMap::new(),
        };
        if let Some(collection) = collection {
            if roots.len() != 1 {
                return Err("Ui.windows must be the single outermost element".into());
            }
            for (key, tag) in elements(tree, collection) {
                if tag != "ui-window" {
                    return Err(format!(
                        "Ui.windows accepts only Ui.window declarations, found {tag:?}"
                    ));
                }
                plan.window(tree, key, Some(key), true)?;
            }
            if previous_mode.is_none() && plan.declarations.is_empty() {
                return Err("Ui.windows must mount at least one window initially".into());
            }
        } else {
            let wrapper = roots
                .iter()
                .find_map(|&(key, tag)| (tag == "ui-window").then_some(key));
            if wrapper.is_some() && roots.len() != 1 {
                return Err("Ui.window must be the single outermost element".into());
            }
            plan.window(tree, ROOT, wrapper, false)?;
        }

        let mut owners = BTreeSet::new();
        for window in plan.declarations.clone() {
            plan.visit(tree, window.content, window.id, false, &mut owners)?;
            if let Some(wrapper) = window.wrapper {
                plan.scopes.insert(wrapper, window.id);
            }
        }
        Ok(plan)
    }

    fn window(&mut self, tree: &Tree, id: Key, wrapper: Option<Key>, managed: bool) -> Result<()> {
        let content = WindowDeclaration::read(tree, wrapper, ROOT)?.content;
        let title = match wrapper.and_then(|key| tree.node(key).attr("window-title")) {
            None | Some("app") => Title::App,
            Some(raw) if raw.starts_with("text:") => Title::Text(raw[5..].into()),
            Some(_) => return Err("invalid window title declaration".into()),
        };
        let request = wrapper
            .map(|key| close_request(tree, key, managed))
            .transpose()?
            .flatten();
        let startup = Startup::read(
            wrapper.and_then(|key| tree.node(key).attr("startup-position")),
            wrapper.and_then(|key| tree.node(key).attr("startup-state")),
        )?;
        self.declarations.push(Declaration {
            id,
            wrapper,
            content,
            owner: None,
            title,
            request,
            startup,
        });
        Ok(())
    }

    fn visit(
        &mut self,
        tree: &Tree,
        key: Key,
        owner: Key,
        in_dialog: bool,
        dialogs: &mut BTreeSet<Key>,
    ) -> Result<()> {
        let node = tree.node(key);
        match &node.kind {
            Kind::Element(tag) if tag == "ui-window" || tag == "ui-windows" => {
                return Err(format!("{tag} cannot be nested within window content"));
            }
            Kind::Element(tag) if tag == "ui-dialog" => {
                if in_dialog || !dialogs.insert(owner) {
                    return Err(
                        "only one open dialog per window is supported, with no nested dialogs"
                            .into(),
                    );
                }
                let mut children = elements(tree, key);
                let Some((body, "ui-dialog-content")) = children.next() else {
                    return Err("Ui.dialog needs a content element".into());
                };
                if children.next().is_some() {
                    return Err("Ui.dialog accepts one content element".into());
                }
                let title = Title::Text(
                    node.attr("dialog-title")
                        .ok_or("Ui.dialog needs a title")?
                        .into(),
                );
                let request = close_request(tree, key, true)?;
                self.declarations.push(Declaration {
                    id: key,
                    wrapper: None,
                    content: body,
                    owner: Some(owner),
                    title,
                    request,
                    startup: Startup::default(),
                });
                self.scopes.insert(key, key);
                return self.visit(tree, body, key, true, dialogs);
            }
            _ => {}
        }
        self.scopes.insert(key, owner);
        for &child in &node.children {
            self.visit(tree, child, owner, in_dialog, dialogs)?;
        }
        Ok(())
    }
}

fn close_request(tree: &Tree, key: Key, required: bool) -> Result<Option<Key>> {
    match tree.mounted_event(key, "close-request") {
        Some((_, listener)) if listener.kind == 1 => Ok(Some(key)),
        Some(_) => Err("a close-request needs a typed unit listener".into()),
        None if required => Err("managed windows and dialogs need a close-request handler".into()),
        None => Ok(None),
    }
}

#[cfg(windows)]
mod native {
    use super::{Declaration, Startup, StartupPosition, StartupState, Title, WindowPlan};
    use crate::Result;
    use crate::display_preferences::DisplayPreferences;
    use crate::modal::{self, Modal};
    use crate::taskbar::native::Taskbar;
    use crate::typeface::FontRegistry;
    use crate::view::Surface;
    use crate::window::{create_window, window_proc};
    use crate::window_chrome::WindowDeclaration;
    use crate::window_icons::WindowIcons;
    use crate::wire::{Key, ROOT, Tree};
    use std::collections::{BTreeMap, BTreeSet};
    use std::ops::{Deref, DerefMut};
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, GA_ROOT, GWL_STYLE, GetAncestor, GetWindowLongPtrW, GetWindowRect, IsWindow,
        SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_OVERLAPPEDWINDOW, WS_POPUP,
    };

    fn place_startup(hwnd: HWND, startup: Startup) -> Result<()> {
        if startup.position == StartupPosition::System && startup.state != StartupState::Fullscreen
        {
            return Ok(());
        }
        // SAFETY: The validated HWND belongs to this thread and has yet to be shown.
        unsafe {
            let monitor = match startup.position {
                StartupPosition::At(x, y) => {
                    MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST)
                }
                _ => MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            };
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..MONITORINFO::default()
            };
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return Err("could not read the startup window's monitor".into());
            }
            let mut bounds = info.rcMonitor;
            let mut flags = SWP_NOACTIVATE | SWP_NOZORDER;
            if startup.state == StartupState::Fullscreen {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                if SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    ((style & !WS_OVERLAPPEDWINDOW) | WS_POPUP) as i32 as isize,
                ) == 0
                {
                    return Err("could not set the startup window's fullscreen style".into());
                }
                flags |= SWP_FRAMECHANGED;
            } else {
                if GetWindowRect(hwnd, &mut bounds) == 0 {
                    return Err("could not measure the startup window".into());
                }
                let work = info.rcWork;
                let wide = work.right - work.left;
                let high = work.bottom - work.top;
                let width = (bounds.right - bounds.left).min(wide);
                let height = (bounds.bottom - bounds.top).min(high);
                let (x, y) = match startup.position {
                    StartupPosition::At(x, y) => (x, y),
                    _ => (
                        work.left + (wide - width) / 2,
                        work.top + (high - height) / 2,
                    ),
                };
                bounds.left = x.clamp(work.left, work.right - width);
                bounds.top = y.clamp(work.top, work.bottom - height);
                bounds.right = bounds.left + width;
                bounds.bottom = bounds.top + height;
            }
            if SetWindowPos(
                hwnd,
                null_mut(),
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                flags,
            ) == 0
            {
                return Err("could not place the startup window".into());
            }
        }
        Ok(())
    }

    #[cfg(test)]
    #[test]
    fn startup_placement_respects_work_area_and_fullscreen_bounds() {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, IsWindowVisible};
        use windows_sys::core::w;

        // SAFETY: The test creates and checks a hidden HWND on this thread.
        unsafe {
            let window = OwnedWindow(CreateWindowExW(
                0,
                w!("STATIC"),
                w!(""),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                360,
                280,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            ));
            let hwnd = window.0;
            assert!(!hwnd.is_null());
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..MONITORINFO::default()
            };
            assert_ne!(GetMonitorInfoW(monitor, &mut info), 0);
            let work = info.rcWork;
            let full = info.rcMonitor;
            let rect = || {
                let mut bounds = RECT::default();
                assert_ne!(GetWindowRect(hwnd, &mut bounds), 0);
                (bounds.left, bounds.top, bounds.right, bounds.bottom)
            };
            let initial = rect();
            place_startup(hwnd, Startup::default()).unwrap();
            assert_eq!(rect(), initial);

            place_startup(
                hwnd,
                Startup {
                    position: StartupPosition::Centered,
                    ..Startup::default()
                },
            )
            .unwrap();
            let width = 360.min(work.right - work.left);
            let height = 280.min(work.bottom - work.top);
            let x = work.left + (work.right - work.left - width) / 2;
            let y = work.top + (work.bottom - work.top - height) / 2;
            assert_eq!(rect(), (x, y, x + width, y + height));

            place_startup(
                hwnd,
                Startup {
                    position: StartupPosition::At(work.right - 1, work.bottom - 1),
                    ..Startup::default()
                },
            )
            .unwrap();
            assert_eq!(
                rect(),
                (
                    work.right - width,
                    work.bottom - height,
                    work.right,
                    work.bottom
                )
            );

            place_startup(
                hwnd,
                Startup {
                    state: StartupState::Fullscreen,
                    ..Startup::default()
                },
            )
            .unwrap();
            assert_eq!(rect(), (full.left, full.top, full.right, full.bottom));
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            assert_ne!(style & WS_POPUP, 0);
            assert_eq!(style & WS_OVERLAPPEDWINDOW, 0);
            assert_eq!(IsWindowVisible(hwnd), 0);
        }
    }

    pub(crate) struct OwnedWindow(pub(crate) HWND);
    impl Drop for OwnedWindow {
        fn drop(&mut self) {
            // SAFETY: This owner belongs to the UI thread.
            if !self.0.is_null() {
                unsafe { DestroyWindow(self.0) };
            }
        }
    }

    pub(crate) struct SpecSurface {
        pub(crate) id: Key,
        pub(crate) handle: HWND,
        pub(crate) root: Key,
        pub(crate) title: String,
        pub(crate) is_dialog: bool,
        pub(crate) blocked: bool,
        pub(crate) owner: Option<Key>,
    }

    struct Mounted {
        declaration: Declaration,
        surface: Surface,
        icons: Option<WindowIcons>,
        taskbar: Option<Taskbar>,
        modal: Option<Modal>,
        title: String,
        window: OwnedWindow,
    }

    impl Drop for Mounted {
        fn drop(&mut self) {
            if let Some(taskbar) = &mut self.taskbar {
                taskbar.clear();
            }
        }
    }

    pub(crate) struct SurfaceRegistry {
        mounted: BTreeMap<Key, Mounted>,
        handles: BTreeMap<usize, Key>,
        scope: BTreeMap<Key, Key>,
        order: Vec<Key>,
        pending_legacy: Option<OwnedWindow>,
        selected: Key,
        managed: Option<bool>,
        driven: bool,
        app_title: String,
        preferences: DisplayPreferences,
        legacy_title: Option<String>,
        exited: bool,
        font_registry: FontRegistry,
    }

    impl SurfaceRegistry {
        pub(crate) fn new(
            window: OwnedWindow,
            title: String,
            driven: bool,
            preferences: DisplayPreferences,
        ) -> Self {
            Self {
                mounted: BTreeMap::new(),
                handles: BTreeMap::new(),
                scope: BTreeMap::new(),
                order: Vec::new(),
                pending_legacy: Some(window),
                selected: ROOT,
                managed: None,
                driven,
                app_title: title,
                preferences,
                legacy_title: None,
                exited: false,
                font_registry: FontRegistry::default(),
            }
        }

        pub(crate) fn display_preferences(
            &mut self,
            tree: &Tree,
            preferences: DisplayPreferences,
        ) -> Result<()> {
            if self.preferences != preferences {
                self.preferences = preferences;
                for surface in self.surfaces_mut() {
                    surface.display_preferences(tree, preferences)?;
                }
            }
            Ok(())
        }

        pub(crate) fn retiring_calendars(
            &self,
            tree: &Tree,
            open: impl Iterator<Item = (Key, Key)>,
        ) -> Result<Vec<Key>> {
            let plan = WindowPlan::read(tree, self.managed)?;
            let blocked: BTreeSet<_> = plan
                .declarations
                .iter()
                .filter_map(|wanted| wanted.owner)
                .collect();
            Ok(open
                .filter_map(|(owner, field)| {
                    let mounted = plan.scopes.get(&field) == Some(&owner);
                    let enabled = mounted
                        && tree
                            .node(field)
                            .attr("enabled")
                            .is_none_or(crate::window::enabled_flag);
                    (!enabled || blocked.contains(&owner)).then_some(owner)
                })
                .collect())
        }

        pub(crate) fn sync(&mut self, tree: &Tree) -> Result<()> {
            if self.exited {
                return Ok(());
            }
            let plan = WindowPlan::read(tree, self.managed)?;
            self.managed = Some(plan.managed);
            if plan.managed {
                self.pending_legacy.take();
            }
            let live: BTreeSet<_> = plan.declarations.iter().map(|wanted| wanted.id).collect();
            let modal_owners: BTreeSet<_> = plan
                .declarations
                .iter()
                .filter_map(|wanted| wanted.owner)
                .collect();
            let mut removed = self
                .order
                .iter()
                .rev()
                .copied()
                .filter(|id| !live.contains(id))
                .collect::<Vec<_>>();
            // Owned panels precede owners even when the engine moves a declaration.
            removed.sort_by_key(|id| self.mounted[id].declaration.owner.is_none());
            let mut restore = Vec::new();
            for id in removed {
                let mut dead = self
                    .mounted
                    .remove(&id)
                    .expect("mounted window order is authoritative");
                self.handles.remove(&(dead.window.0 as usize));
                if let Some(mut modal) = dead.modal.take() {
                    let owner = dead.declaration.owner.expect("owned dialog has a guard");
                    let surviving = live.contains(&owner);
                    let focus =
                        surviving && !modal_owners.contains(&owner) && modal.can_restore_focus();
                    modal.release_owner();
                    restore.push((modal, focus));
                }
                drop(dead);
            }
            self.scope = plan.scopes;
            let mut opened = Vec::new();
            self.order.clear();
            for wanted in plan.declarations {
                let id = wanted.id;
                let inherited_accent = wanted
                    .owner
                    .map(|owner| self.mounted[&owner].surface.accent());
                let title = self.title_for(&wanted).to_owned();
                if !self.mounted.contains_key(&id) {
                    let window = if let Some(owner) = wanted.owner {
                        let parent = self.mounted[&owner].window.0;
                        OwnedWindow(modal::create(parent, Some(window_proc), &title)?)
                    } else if id == ROOT {
                        match self.pending_legacy.take() {
                            Some(window) => window,
                            None => create_window(&title)?,
                        }
                    } else {
                        create_window(&title)?
                    };
                    let hwnd = window.0;
                    if wanted.owner.is_none() && !self.driven {
                        place_startup(hwnd, wanted.startup)?;
                    }
                    let surface = Surface::new(hwnd, self.preferences, self.font_registry.clone())?;
                    let icons = wanted.owner.is_none().then(|| WindowIcons::new(hwnd));
                    let taskbar = if !self.driven && wanted.owner.is_none() {
                        Taskbar::new(hwnd)
                    } else {
                        None
                    };
                    self.handles.insert(hwnd as usize, id);
                    self.mounted.insert(
                        id,
                        Mounted {
                            declaration: wanted.clone(),
                            surface,
                            icons,
                            taskbar,
                            modal: None,
                            title: String::new(),
                            window,
                        },
                    );
                    opened.push(id);
                }
                let window = self.mounted.get_mut(&id).unwrap();
                if window.title != title {
                    window.surface.title(&title);
                    window.title = title;
                }
                window.declaration = wanted;
                let chrome = WindowDeclaration::read(
                    tree,
                    window.declaration.wrapper,
                    window.declaration.content,
                )?;
                let accent = inherited_accent.unwrap_or(chrome.accent);
                window.surface.sync(tree, chrome, &self.scope, id, accent)?;
                if let Some(taskbar) = &mut window.taskbar {
                    taskbar.update(window.surface.taskbar_progress());
                }
                let scope = window
                    .declaration
                    .wrapper
                    .unwrap_or(window.declaration.content);
                window
                    .surface
                    .register_file_drop(tree.event(scope, "drop").is_some());
                self.order.push(id);
            }
            if self.order.is_empty() {
                self.exited = true;
            } else if !self.mounted.contains_key(&self.selected) {
                self.selected = self.order[0];
            }

            for (guard, focus) in restore {
                guard.finish(focus && !self.driven);
            }
            for id in opened {
                let entry = &self.mounted[&id];
                let hwnd = entry.window.0;
                if let Some(owner) = entry.declaration.owner {
                    let owner_hwnd = self.mounted[&owner].window.0;
                    let measure = entry.surface.dialog_size(tree);
                    modal::position(hwnd, owner_hwnd, measure)?;
                    self.mounted.get_mut(&id).unwrap().surface.arrange(tree)?;
                    let guard = Modal::begin(owner_hwnd, hwnd);
                    if !self.driven {
                        guard.show();
                    }
                    self.mounted.get_mut(&id).unwrap().modal = Some(guard);
                } else if !self.driven {
                    // SAFETY: The validated surface already owns all its native children.
                    let show = match entry.declaration.startup.state {
                        StartupState::Minimized => SW_SHOWMINNOACTIVE,
                        StartupState::Maximized => SW_SHOWMAXIMIZED,
                        StartupState::Normal | StartupState::Fullscreen => SW_SHOWNORMAL,
                    };
                    unsafe { ShowWindow(hwnd, show) };
                    if show == SW_SHOWMAXIMIZED {
                        // The synchronous WM_SIZE is suppressed while the registry is borrowed.
                        self.mounted.get_mut(&id).unwrap().surface.arrange(tree)?;
                    }
                }
            }
            self.refresh_automation_interaction();
            Ok(())
        }

        pub(crate) fn refresh_automation_interaction(&self) {
            for entry in self.mounted.values() {
                entry.surface.refresh_automation_interaction();
            }
        }

        fn title_for<'a>(&'a self, wanted: &'a Declaration) -> &'a str {
            match &wanted.title {
                Title::Text(title) => title,
                Title::App if wanted.id == ROOT => {
                    self.legacy_title.as_deref().unwrap_or(&self.app_title)
                }
                Title::App => &self.app_title,
            }
        }

        pub(crate) fn set_legacy_title(&mut self, title: String) {
            self.legacy_title = Some(title);
        }

        pub(crate) fn legacy_handle(&self) -> Option<HWND> {
            if self.managed == Some(true) {
                return None;
            }
            self.handle(ROOT)
                .or_else(|| self.pending_legacy.as_ref().map(|window| window.0))
        }

        pub(crate) fn all(&self) -> Vec<SpecSurface> {
            self.order
                .iter()
                .map(|&id| {
                    let entry = &self.mounted[&id];
                    SpecSurface {
                        id,
                        handle: entry.window.0,
                        root: entry.declaration.content,
                        title: entry.title.clone(),
                        is_dialog: entry.declaration.owner.is_some(),
                        blocked: self.blocked(id),
                        owner: entry.declaration.owner,
                    }
                })
                .collect()
        }

        pub(crate) fn selected(&self) -> Key {
            self.selected
        }

        pub(crate) fn select(&mut self, id: Key) -> bool {
            if self.mounted.contains_key(&id) {
                self.selected = id;
                true
            } else {
                false
            }
        }

        pub(crate) fn blocked(&self, id: Key) -> bool {
            self.mounted
                .values()
                .any(|entry| entry.declaration.owner == Some(id))
        }

        pub(crate) fn target(&self, id: Key) -> Option<Option<Key>> {
            self.mounted.get(&id).map(|entry| entry.declaration.request)
        }

        pub(crate) fn handle(&self, id: Key) -> Option<HWND> {
            self.mounted.get(&id).map(|entry| entry.window.0)
        }

        pub(crate) fn for_key(&self, key: Key) -> Option<Key> {
            self.scope
                .get(&key)
                .copied()
                .filter(|id| self.mounted.contains_key(id))
        }

        pub(crate) fn for_handle(&self, handle: HWND) -> Option<Key> {
            if handle.is_null() {
                return None;
            }
            // SAFETY: GetAncestor returns a live top-level root, without following ownership.
            let root = unsafe { GetAncestor(handle, GA_ROOT) };
            self.handles.get(&(root as usize)).copied()
        }

        pub(crate) fn at(&self, id: Key) -> Option<&Surface> {
            Some(&self.mounted.get(&id)?.surface)
        }

        pub(crate) fn at_mut(&mut self, id: Key) -> Option<&mut Surface> {
            Some(&mut self.mounted.get_mut(&id)?.surface)
        }

        pub(crate) fn surfaces_mut(&mut self) -> impl Iterator<Item = &mut Surface> {
            self.mounted.values_mut().map(|entry| &mut entry.surface)
        }

        pub(crate) fn is_dialog(&self, id: Key) -> bool {
            self.mounted
                .get(&id)
                .is_some_and(|entry| entry.declaration.owner.is_some())
        }

        pub(crate) fn is_fullscreen(&self, id: Key) -> bool {
            self.mounted.get(&id).is_some_and(|entry| {
                // SAFETY: The registry owns this live HWND on the UI thread.
                entry.declaration.owner.is_none()
                    && unsafe { GetWindowLongPtrW(entry.window.0, GWL_STYLE) } as u32 & WS_POPUP
                        != 0
            })
        }

        pub(crate) fn poll(&mut self) {
            for entry in self.mounted.values_mut() {
                entry.surface.poll_shaders();
                if let Some(taskbar) = &mut entry.taskbar {
                    taskbar.poll();
                }
            }
        }

        pub(crate) fn sync_icons(&mut self, id: Key) {
            if let Some(icons) = self
                .mounted
                .get_mut(&id)
                .and_then(|entry| entry.icons.as_mut())
            {
                icons.sync();
            }
        }

        pub(crate) fn shutdown(&mut self) {
            let mut ids = self.mounted.keys().copied().collect::<Vec<_>>();
            ids.sort_by_key(|id| self.mounted[id].declaration.owner.is_none());
            for id in ids {
                if let Some(mut entry) = self.mounted.remove(&id) {
                    self.handles.remove(&(entry.window.0 as usize));
                    let guard = entry.modal.take();
                    drop(entry);
                    if let Some(guard) = guard {
                        guard.finish(false);
                    }
                }
            }
            self.order.clear();
            self.scope.clear();
            self.exited = true;
        }

        pub(crate) fn exited(&self) -> bool {
            self.exited
        }

        pub(crate) fn forget_destroyed(&mut self, hwnd: HWND) -> bool {
            let Some(id) = self.handles.remove(&(hwnd as usize)) else {
                return false;
            };
            // An unexpected native deletion must fail the process rather than dispatch with stale HWNDs.
            self.order.retain(|&other| other != id);
            if let Some(mut entry) = self.mounted.remove(&id) {
                if unsafe { IsWindow(entry.window.0) } != 0 {
                    // WM_NCDESTROY is already destroying this HWND. Avoid re-entry.
                    entry.window.0 = std::ptr::null_mut();
                }
                drop(entry);
            }
            true
        }
    }

    impl Drop for SurfaceRegistry {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    impl Deref for SurfaceRegistry {
        type Target = Surface;
        fn deref(&self) -> &Self::Target {
            &self.mounted[&self.selected].surface
        }
    }

    impl DerefMut for SurfaceRegistry {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self
                .mounted
                .get_mut(&self.selected)
                .expect("scope a live window first")
                .surface
        }
    }
}

#[cfg(windows)]
pub(crate) use native::{OwnedWindow, SpecSurface, SurfaceRegistry};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Reader, put_str};

    fn apply(tree: &mut Tree, code: u8, buffer: Vec<u8>) {
        assert!(tree.command(code, &mut Reader::new(&buffer)).unwrap());
    }

    fn words(numbers: &[u32]) -> Vec<u8> {
        numbers
            .iter()
            .flat_map(|number| number.to_le_bytes())
            .collect()
    }

    fn name(buffer: &mut Vec<u8>, value: &str) {
        buffer.push(0);
        put_str(buffer, "");
        put_str(buffer, value);
    }

    fn element(tree: &mut Tree, id: u32, parent: u32, tag: &str) -> Key {
        let mut buffer = words(&[id, parent, 0]);
        name(&mut buffer, tag);
        buffer.push(0);
        apply(tree, 8, buffer);
        // These fixtures allocate element nodes in their wire ID order.
        let key = id as Key;
        assert!(tree.contains(key));
        key
    }

    fn listen(tree: &mut Tree, id: u32) {
        let mut buffer = words(&[id, id]);
        put_str(&mut buffer, "close-request");
        buffer.push(1);
        buffer.extend_from_slice(&0u32.to_le_bytes());
        apply(tree, 11, buffer);
    }

    fn attr(tree: &mut Tree, id: u32, attribute: &str, value: &str) {
        let mut buffer = words(&[id]);
        name(&mut buffer, attribute);
        buffer.push(1);
        put_str(&mut buffer, value);
        apply(tree, 9, buffer);
    }

    #[test]
    fn collection_validates_controlled_windows_before_honoring_last_close() {
        let mut tree = Tree::new();
        let collection = element(&mut tree, 1, 0, "ui-windows");
        assert_eq!(collection, 1);
        assert!(WindowPlan::read(&tree, None).is_err());

        let first = element(&mut tree, 2, 1, "ui-window");
        let content = element(&mut tree, 3, 2, "ui-window-content");
        assert!(WindowPlan::read(&tree, None).is_err());
        listen(&mut tree, 2);
        let plan = WindowPlan::read(&tree, None).unwrap();
        assert_eq!(plan.declarations[0].request, Some(first));
        assert_eq!(plan.scopes[&content], first);

        apply(&mut tree, 5, words(&[2]));
        let replacement_plan = WindowPlan::read(&tree, Some(true)).unwrap();
        assert!(replacement_plan.managed);
        assert!(replacement_plan.declarations.is_empty());

        let mut buffer = words(&[4, 1, 0]);
        name(&mut buffer, "ui-window");
        buffer.push(0);
        apply(&mut tree, 8, buffer);
        let replacement = *tree.node(collection).children.last().unwrap();
        let mut buffer = words(&[5, 4, 0]);
        name(&mut buffer, "ui-window-content");
        buffer.push(0);
        apply(&mut tree, 8, buffer);
        listen(&mut tree, 4);
        attr(&mut tree, 4, "window-title", "text:Replacement");
        let plan = WindowPlan::read(&tree, Some(true)).unwrap();
        assert_eq!(plan.declarations[0].id, replacement);
        assert_ne!(first, replacement);
        assert_eq!(
            plan.declarations[0].title,
            Title::Text("Replacement".into())
        );
    }

    #[test]
    fn window_decorations_must_be_outermost() {
        let mut tree = Tree::new();
        element(&mut tree, 1, 0, "ui-window");
        element(&mut tree, 2, 1, "ui-window-content");
        assert!(WindowPlan::read(&tree, None).is_ok());
        element(&mut tree, 3, 0, "ui-text");
        assert!(WindowPlan::read(&tree, None).is_err());
    }

    #[test]
    fn startup_options_are_validated_at_the_window_boundary() {
        let mut tree = Tree::new();
        element(&mut tree, 1, 0, "ui-window");
        element(&mut tree, 2, 1, "ui-window-content");
        let read = |tree: &Tree| WindowPlan::read(tree, None).unwrap().declarations[0].startup;
        assert_eq!(read(&tree), Startup::default());

        attr(&mut tree, 1, "startup-position", "-720,18");
        attr(&mut tree, 1, "startup-state", "maximized");
        assert_eq!(
            read(&tree),
            Startup {
                position: StartupPosition::At(-720, 18),
                state: StartupState::Maximized
            }
        );

        attr(&mut tree, 1, "startup-position", "2147483648,18");
        assert!(WindowPlan::read(&tree, None).is_err());
        attr(&mut tree, 1, "startup-position", "centered");
        attr(&mut tree, 1, "startup-state", "unknown");
        assert!(WindowPlan::read(&tree, None).is_err());
    }

    #[test]
    fn dialog_controls_have_their_own_scope_and_owner() {
        let mut tree = Tree::new();
        let _stack = element(&mut tree, 1, 0, "ui-vstack");
        let owner_control = element(&mut tree, 2, 1, "ui-button");
        let dialog = element(&mut tree, 3, 1, "ui-dialog");
        let content = element(&mut tree, 4, 3, "ui-dialog-content");
        let dialog_control = element(&mut tree, 5, 4, "ui-button");
        listen(&mut tree, 3);
        attr(&mut tree, 3, "dialog-title", "Notes");
        let plan = WindowPlan::read(&tree, None).unwrap();
        assert_eq!(plan.scopes[&owner_control], ROOT);
        assert_eq!(plan.scopes[&dialog_control], dialog);
        assert_eq!(plan.declarations[1].owner, Some(ROOT));
        assert_eq!(plan.declarations[1].content, content);

        element(&mut tree, 6, 4, "ui-dialog");
        assert!(WindowPlan::read(&tree, Some(false)).is_err());
    }
}
