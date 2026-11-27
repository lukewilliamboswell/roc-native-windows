use crate::Result;
use crate::progress::Progress;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Clear,
    Indeterminate,
    Normal,
    Value { completed: u64, total: u64 },
}

/// Coalesces requests made before a button exists and replays the latest one
/// when Explorer creates a replacement button.
#[derive(Default)]
struct Replay {
    requested: Progress,
    applied: Option<Progress>,
    ready: bool,
}

impl Replay {
    fn button_created(&mut self) {
        self.ready = true;
        self.applied = None;
    }

    fn button_unavailable(&mut self) {
        self.ready = false;
        self.applied = None;
    }

    fn reconcile(&mut self, mut send: impl FnMut(Command) -> Result<()>) -> Result<()> {
        if !self.ready || self.applied == Some(self.requested) {
            return Ok(());
        }
        match self.requested {
            Progress::Idle => send(Command::Clear)?,
            Progress::Indeterminate => send(Command::Indeterminate)?,
            Progress::Determinate { completed, total } => {
                if !matches!(self.applied, Some(Progress::Determinate { .. })) {
                    send(Command::Normal)?;
                }
                send(Command::Value { completed, total })?;
            }
        }
        self.applied = Some(self.requested);
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) mod native {
    use super::{Command, Replay};
    use crate::Result;
    use crate::progress::Progress;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HWND as ShellWindow;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        ITaskbarList3, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TaskbarList,
    };
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    use windows_sys::core::w;

    static BUTTON_CREATED: OnceLock<u32> = OnceLock::new();
    thread_local! {
        static REPLAY_PENDING: RefCell<BTreeSet<usize>> = const { RefCell::new(BTreeSet::new()) };
    }

    pub(crate) struct Taskbar {
        window: ShellWindow,
        playback: Replay,
        shell: Option<ITaskbarList3>,
        apartment: Option<Apartment>,
        warned: bool,
    }

    impl Drop for Taskbar {
        fn drop(&mut self) {
            REPLAY_PENDING.with(|pending| pending.borrow_mut().remove(&(self.window.0 as usize)));
        }
    }

    impl Taskbar {
        pub(crate) fn new(window: HWND) -> Self {
            // SAFETY: User32 reads the static, terminated message name.
            BUTTON_CREATED
                .get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) });
            Self {
                window: ShellWindow(window),
                playback: Replay::default(),
                shell: None,
                apartment: None,
                warned: false,
            }
        }

        pub(crate) fn queue_if_button_created(hwnd: HWND, message: u32) -> bool {
            if message != 0 && BUTTON_CREATED.get() == Some(&message) {
                // A registered message can arrive during a shell callback or
                // another UI update. Reconcile when the event pump owns App.
                REPLAY_PENDING.with(|pending| pending.borrow_mut().insert(hwnd as usize));
                true
            } else {
                false
            }
        }

        pub(crate) fn poll(&mut self) {
            if REPLAY_PENDING.with(|pending| pending.borrow_mut().remove(&(self.window.0 as usize)))
            {
                self.shell = None;
                self.playback.button_created();
                self.reconcile();
            }
        }

        pub(crate) fn update(&mut self, requested: Progress) {
            self.playback.requested = requested;
            if requested != Progress::Idle && BUTTON_CREATED.get() == Some(&0) {
                self.unavailable("could not register TaskbarButtonCreated");
            } else {
                self.reconcile();
            }
        }

        pub(crate) fn clear(&mut self) {
            self.update(Progress::Idle);
        }

        fn reconcile(&mut self) {
            let result = self.playback.reconcile(|command| {
                // A newly created button has no progress state before the
                // first active request, so an idle-only app needs no COM object.
                if self.shell.is_none() && command == Command::Clear {
                    return Ok(());
                }
                if self.apartment.is_none() {
                    self.apartment = Some(Apartment::enter()?);
                }
                if self.shell.is_none() {
                    // SAFETY: COM is initialized on this UI thread.
                    let shell: ITaskbarList3 =
                        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
                            .map_err(|error| format!("create the taskbar interface: {error}"))?;
                    // SAFETY: The registered message confirms that the button exists.
                    unsafe { shell.HrInit() }
                        .map_err(|error| format!("initialize the taskbar interface: {error}"))?;
                    self.shell = Some(shell);
                }
                let shell = self.shell.as_ref().unwrap();
                // SAFETY: The interface and window belong to the same UI thread.
                let result = unsafe {
                    match command {
                        Command::Clear => shell.SetProgressState(self.window, TBPF_NOPROGRESS),
                        Command::Indeterminate => {
                            shell.SetProgressState(self.window, TBPF_INDETERMINATE)
                        }
                        Command::Normal => shell.SetProgressState(self.window, TBPF_NORMAL),
                        Command::Value { completed, total } => {
                            shell.SetProgressValue(self.window, completed, total)
                        }
                    }
                };
                result.map_err(|error| format!("update the taskbar progress: {error}"))
            });
            if let Err(error) = result {
                self.unavailable(&error);
            }
        }

        fn unavailable(&mut self, error: &str) {
            self.shell = None;
            self.playback.button_unavailable();
            if !self.warned {
                eprintln!("roc-native-windows: taskbar progress unavailable: {error}");
                self.warned = true;
            }
        }
    }

    struct Apartment;

    impl Apartment {
        fn enter() -> Result<Self> {
            // SAFETY: The taskbar runs on the host's UI thread.
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .map_err(|error| format!("initialize COM for taskbar progress: {error}"))?;
            Ok(Self)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: Balance the successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(replay: &mut Replay) -> Vec<Command> {
        let mut commands = Vec::new();
        replay
            .reconcile(|command| {
                commands.push(command);
                Ok(())
            })
            .unwrap();
        commands
    }

    #[test]
    fn applies_only_the_latest_request_after_a_button_is_created() {
        let mut replay = Replay {
            requested: Progress::Indeterminate,
            ..Replay::default()
        };
        assert!(collect(&mut replay).is_empty());
        replay.requested = Progress::Determinate {
            completed: 10,
            total: 20,
        };
        assert!(collect(&mut replay).is_empty());
        replay.button_created();
        assert_eq!(
            collect(&mut replay),
            [
                Command::Normal,
                Command::Value {
                    completed: 10,
                    total: 20
                }
            ]
        );
        assert!(collect(&mut replay).is_empty());

        replay.requested = Progress::Determinate {
            completed: 15,
            total: 20,
        };
        assert_eq!(
            collect(&mut replay),
            [Command::Value {
                completed: 15,
                total: 20
            }]
        );
    }

    #[test]
    fn replays_after_shell_recovery_and_clears_removed_declarations() {
        let mut replay = Replay::default();
        replay.button_created();
        replay.requested = Progress::Indeterminate;
        assert_eq!(collect(&mut replay), [Command::Indeterminate]);
        replay.button_unavailable();
        assert!(collect(&mut replay).is_empty());
        replay.button_created();
        assert_eq!(collect(&mut replay), [Command::Indeterminate]);
        replay.requested = Progress::parse("").unwrap();
        assert_eq!(collect(&mut replay), [Command::Clear]);
        assert!(collect(&mut replay).is_empty());
    }

    #[test]
    fn retains_desired_progress_if_a_shell_command_fails() {
        let mut replay = Replay::default();
        replay.button_created();
        replay.requested = Progress::Determinate {
            completed: 1,
            total: 2,
        };
        assert_eq!(
            replay.reconcile(|_| Err("shell disappeared".into())),
            Err("shell disappeared".into())
        );
        replay.button_unavailable();
        replay.button_created();
        assert_eq!(
            collect(&mut replay),
            [
                Command::Normal,
                Command::Value {
                    completed: 1,
                    total: 2
                }
            ]
        );
    }
}
