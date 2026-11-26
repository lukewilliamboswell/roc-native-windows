use crate::abi::{self, Engine};
use crate::effects::Effects;
use crate::view::{Surface, wide};
use crate::wire::{Declarations, EFFECT_NODE, Reader, Tree};
use crate::{Event, Result};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
use windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
use windows_sys::Win32::System::Time::{
    GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
};
use windows_sys::Win32::UI::Controls::{
    ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

const PUMP: usize = 1;

thread_local! {
    // User32 is allowed to send synchronous child notifications during a
    // property update. A nested window callback must not re-enter the app.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct App {
    engine: Engine,
    tree: Tree,
    surface: Surface,
    effects: Effects,
    incoming: Receiver<Event>,
    private: VecDeque<Event>,
    opportunity: Option<u64>,
}

impl App {
    fn start(window: HWND) -> Result<Self> {
        let (send, incoming) = channel();
        let mut app = Self {
            engine: Engine::default(),
            tree: Tree::new(),
            surface: Surface::new(window)?,
            effects: Effects::new(window, send),
            incoming,
            private: VecDeque::new(),
            opportunity: None,
        };
        let declarations = Declarations::decode(&Engine::declarations()?)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| err.to_string())?;
        let mut seed = [0; 16];
        // SAFETY: The buffer is writable for exactly 16 bytes and the flag
        // selects the system RNG with no algorithm handle.
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
        let boot = declarations.boot(
            now.as_millis() as i64,
            seed,
            &language(),
            zone_offset(),
            |session, key| app.effects.boot_value(session, key),
        )?;
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
        while let Some(event) = self.private.pop_front() {
            self.deliver(event)?;
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<()> {
        self.effects.poll();
        if let Some(generation) = self.opportunity.take() {
            let mut payload = vec![255];
            payload.extend_from_slice(&generation.to_le_bytes());
            self.deliver(effect_event(payload))?;
            self.drain_private()?;
        }
        while let Ok(event) = self.incoming.try_recv() {
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
                17 => self.surface.title(reader.string()?),
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
                    self.effects.save(token, start, length, reader.string()?)?;
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
            if let Some(key) = self.tree.reference(id) {
                self.surface.reference(op, key);
            }
        }
        Ok(())
    }

    fn command(&mut self, control: HWND, notification: u16) -> Result<()> {
        let Some(key) = self.surface.key(control) else {
            return Ok(());
        };
        let view_kind = self.surface.kind(key);
        use crate::view::Control;
        let (event_name, value) = match (view_kind, u32::from(notification)) {
            (Control::Edit, EN_CHANGE) => ("input", self.surface.input(key)),
            (Control::Button, BN_CLICKED) => ("click", String::new()),
            (Control::Checkbox, BN_CLICKED) => ("change", self.surface.input(key)),
            (Control::FileButton, BN_CLICKED) => {
                if let Some((node, listener)) = self.tree.event(key, "change") {
                    let multiple = self
                        .tree
                        .node(key)
                        .attr("multiple")
                        .is_some_and(enabled_flag);
                    self.effects.pick(node, listener.handler, multiple)?;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
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
            self.surface.restore_controlled_value(key);
        }
        Ok(())
    }

    fn dropped(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        if let Some((node, listener)) = self.tree.event(crate::wire::ROOT, "drop") {
            self.effects.deliver_paths(node, listener.handler, paths)?;
        }
        Ok(())
    }
}

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

fn paths_from_drop(drop: HDROP) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // SAFETY: The drop handle came from WM_DROPFILES; each UTF-16 buffer
    // includes room for the nul terminator, and DragFinish consumes it.
    unsafe {
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
    paths
}

fn app_name() -> String {
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

fn run() -> Result<()> {
    // SAFETY: This entry point runs once on the process's UI thread. All
    // Win32 input/output pointers below are valid for the called function.
    unsafe {
        let instance = GetModuleHandleW(null());
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err(last_error("initialize Windows common controls"));
        }
        let class = w!("RocSignalsWindow");
        let window_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: class,
            ..WNDCLASSEXW::default()
        };
        if RegisterClassExW(&window_class) == 0 {
            return Err(last_error("register Roc window class"));
        }
        let title = wide(&app_name());
        let window = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            520,
            null_mut(),
            null_mut(),
            instance,
            null::<c_void>(),
        );
        if window.is_null() {
            return Err(last_error("create Roc window"));
        }

        APP.with(|state| -> Result<()> {
            // The borrow suppresses synchronous notifications during mount.
            let mut state = state.borrow_mut();
            *state = Some(App::start(window)?);
            Ok(())
        })?;
        DragAcceptFiles(window, 1);
        if SetTimer(window, PUMP, 15, None) == 0 {
            return Err(last_error("start Roc event pump"));
        }
        ShowWindow(window, SW_SHOWNORMAL);

        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err(last_error("read the Windows message queue")),
                0 => break,
                _ => {
                    if !(WM_KEYFIRST..=WM_KEYLAST).contains(&message.message)
                        || IsDialogMessageW(window, &message) == 0
                    {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
        }
        clear_app();
        if let Some(error) = FAILURE.with(|failure| failure.borrow_mut().take()) {
            return Err(error);
        }
        if message.wParam != 0 {
            return Err("Windows event loop terminated with an error".into());
        }
        Ok(())
    }
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
    abi::register_main_thread();
    match run() {
        Ok(()) => 0,
        Err(error) => {
            report_error(&error);
            1
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: The OS calls this on its window's UI thread with documented
    // WPARAM/LPARAM payloads. Nested callbacks skip app state if busy.
    unsafe {
        match message {
            WM_COMMAND | WM_SIZE | WM_TIMER | WM_DPICHANGED | WM_VSCROLL | WM_MOUSEWHEEL
            | WM_DROPFILES => {
                let paths = (message == WM_DROPFILES).then(|| paths_from_drop(wparam as HDROP));
                let handled = APP.with(|state| {
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return Ok(false);
                    };
                    let Some(app) = &mut *state else {
                        return Ok(false);
                    };
                    match message {
                        WM_COMMAND if lparam != 0 => {
                            app.command(lparam as HWND, (wparam >> 16) as u16)?
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
                        }
                        WM_TIMER if wparam == PUMP => app.poll()?,
                        WM_VSCROLL => app.surface.scroll(&app.tree, wparam as u16, None)?,
                        WM_MOUSEWHEEL => {
                            app.surface
                                .scroll(&app.tree, 0, Some((wparam >> 16) as i16))?
                        }
                        WM_DROPFILES => app.dropped(paths.unwrap_or_default())?,
                        _ => return Ok::<_, String>(false),
                    }
                    Ok(true)
                });
                match handled {
                    Ok(true) => return 0,
                    Ok(false) => {}
                    Err(error) => {
                        FAILURE.with(|failure| failure.borrow_mut().replace(error));
                        clear_app();
                        DestroyWindow(hwnd);
                        return 0;
                    }
                }
            }
            WM_DESTROY => {
                KillTimer(hwnd, PUMP);
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
