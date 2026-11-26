use super::{APP, App};
use crate::surface_input::{self, CAPTURE, Packet, Phase, Queue, Target, native};
use crate::view::Control;
use crate::wire::Key;
use crate::{Event, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT,
    VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const KEYS: [(u16, &str); 8] = [
    (VK_LEFT, "ArrowLeft"),
    (VK_RIGHT, "ArrowRight"),
    (VK_UP, "ArrowUp"),
    (VK_DOWN, "ArrowDown"),
    (VK_HOME, "Home"),
    (VK_END, "End"),
    (VK_PRIOR, "PageUp"),
    (VK_NEXT, "PageDown"),
];

#[derive(Clone, Copy)]
struct Drag {
    key: Key,
    hwnd: HWND,
    native: bool,
}

#[derive(Default)]
pub(super) struct InputState {
    queue: Queue,
    captured: Option<Drag>,
    spec_pressed: bool,
}

impl App {
    fn input_handle(&self, key: Key) -> Option<HWND> {
        self.surface
            .at(self.surface.for_key(key)?)?
            .spec_handle(key)
    }

    fn input_target(&self, key: Key, phase: Phase) -> Option<Target> {
        let hwnd = self.input_handle(key)? as usize;
        let (node, listener) = self.tree.mounted_event(key, phase.event())?;
        let kind = if matches!(phase, Phase::KeyDown | Phase::KeyUp) {
            4
        } else {
            3
        };
        (listener.kind == kind).then_some(Target {
            key,
            hwnd,
            node,
            listener,
        })
    }

    fn input_open(&self, key: Key) -> bool {
        self.surface
            .for_key(key)
            .is_some_and(|owner| !self.surface.blocked(owner))
    }

    pub(super) fn reconcile_input(&mut self) {
        if let Some(drag) = self.input.captured
            && (self.input_handle(drag.key) != Some(drag.hwnd) || !self.input_open(drag.key))
        {
            self.cancel_drag();
        }
    }

    pub(super) fn drain_input(&mut self) -> Result<()> {
        self.input.queue.frame();
        while !self.exited()
            && let Some(packet) = self.input.queue.pop()
        {
            if !self.input_open(packet.target.key) && packet.phase != Phase::Cancel {
                continue;
            }
            if self.input_target(packet.target.key, packet.phase) == Some(packet.target) {
                self.deliver(Event::Turn {
                    node: packet.target.node,
                    handler: packet.target.listener.handler,
                    payload: packet.payload,
                })?;
                self.drain_private()?;
            }
        }
        Ok(())
    }

    fn release_drag(&mut self) -> Option<Drag> {
        let drag = self.input.captured.take()?;
        if !drag.native {
            self.input.spec_pressed = false;
        }
        // SAFETY: Capture APIs belong to the app's UI thread.
        if drag.native && unsafe { GetCapture() } == drag.hwnd {
            unsafe { ReleaseCapture() };
        }
        Some(drag)
    }

    fn cancel_drag(&mut self) {
        if let Some(drag) = self.release_drag()
            && let Some(target) = self.input_target(drag.key, Phase::Cancel)
            && target.hwnd == drag.hwnd as usize
        {
            let modifiers = if drag.native { native::modifiers() } else { 0 };
            self.input.queue.push(Packet {
                target,
                phase: Phase::Cancel,
                payload: surface_input::pointer_payload(Phase::Cancel, None, 0, modifiers),
            });
        }
    }

    fn pointer(
        &mut self,
        key: Key,
        phase: Phase,
        location: Option<(Key, u16, u16)>,
        buttons: u16,
        modifiers: u8,
        real: bool,
    ) {
        let Some(hwnd) = self.input_handle(key) else {
            return;
        };
        if phase == Phase::Cancel {
            if self.input.captured.is_some_and(|drag| drag.hwnd == hwnd) {
                self.cancel_drag();
            }
            return;
        }
        if !self.input_open(key) {
            self.reconcile_input();
            return;
        }
        let target = self.input_target(key, phase);
        if phase == Phase::Down {
            self.cancel_drag();
            let node = self.tree.node(key);
            if real && (node.listens("keydown") || node.listens("keyup")) {
                // SAFETY: hwnd is the live opt-in control on this UI thread.
                unsafe { SetFocus(hwnd) };
            }
            if target.is_some_and(|target| target.listener.policies & CAPTURE != 0) {
                let captured = if real {
                    // SAFETY: SetCapture reports the previous HWND; GetCapture confirms success.
                    unsafe {
                        SetCapture(hwnd);
                        GetCapture() == hwnd
                    }
                } else {
                    true
                };
                if captured {
                    self.input.captured = Some(Drag {
                        key,
                        hwnd,
                        native: real,
                    });
                }
            }
        } else if phase == Phase::Up && self.input.captured.is_some_and(|drag| drag.hwnd == hwnd) {
            self.release_drag();
        }
        if let Some(target) = target {
            let located = location.and_then(|(hit, x, y)| {
                let zone = self
                    .tree
                    .node(hit)
                    .attr("pointer-zone")
                    .filter(|zone| !zone.is_empty())?;
                Some((zone, x, y))
            });
            self.input.queue.push(Packet {
                target,
                phase,
                payload: surface_input::pointer_payload(phase, located, buttons, modifiers),
            });
        } else if phase != Phase::Move {
            self.input.queue.frame(); // Gesture boundaries also order listeners without an up handler.
        }
    }

    pub(super) fn spec_pointer(
        &mut self,
        key: Key,
        phase: Phase,
        position: Option<(u16, u16)>,
    ) -> Result<()> {
        if self.input_handle(key).is_none() || self.tree.node(key).attr("pointer-zone").is_none() {
            return Err("pointer input requires a mounted pointer-enabled surface".into());
        }
        let buttons =
            u16::from(phase == Phase::Down || (phase == Phase::Move && self.input.spec_pressed));
        self.pointer(
            key,
            phase,
            position.map(|(x, y)| (key, x, y)),
            buttons,
            0,
            false,
        );
        match phase {
            Phase::Down => self.input.spec_pressed = true,
            Phase::Up | Phase::Cancel => self.input.spec_pressed = false,
            _ => {}
        }
        Ok(())
    }

    fn key_input(
        &mut self,
        key: Key,
        phase: Phase,
        name: &str,
        modifiers: u8,
        repeat: bool,
    ) -> Result<bool> {
        let Some(target) = self.input_target(key, phase) else {
            return Ok(false);
        };
        let prop = if phase == Phase::KeyDown {
            "keymap-down"
        } else {
            "keymap-up"
        };
        let map = self.tree.node(key).attr(prop).unwrap_or("");
        let Some((prevent, payload)) = surface_input::key_payload(map, name, modifiers, repeat)
        else {
            return Ok(false);
        };
        self.input.queue.push(Packet {
            target,
            phase,
            payload,
        });
        self.drain_input()?;
        Ok(prevent || !self.input_open(key) || self.input_target(key, phase) != Some(target))
    }

    pub(super) fn spec_key(&mut self, key: Key, name: &str) -> Result<()> {
        let &(code, _) = KEYS
            .iter()
            .find(|(_, key_name)| *key_name == name)
            .ok_or("unsupported navigation key")?;
        let hwnd = self
            .input_handle(key)
            .ok_or("key input needs a mounted control")?;
        if self.surface.kind(key) == Control::Slider {
            if self.surface.spec_disabled(key)? {
                return Err("control is disabled".into());
            }
            // SAFETY: The live native trackbar handles the normal key messages.
            unsafe {
                SendMessageW(hwnd, WM_KEYDOWN, usize::from(code), 1);
                SendMessageW(
                    hwnd,
                    WM_KEYUP,
                    usize::from(code),
                    (1 | (3u32 << 30)) as isize,
                );
            }
            return self.slider_changed(hwnd);
        }
        if self.input_target(key, Phase::KeyDown).is_none()
            && self.input_target(key, Phase::KeyUp).is_none()
        {
            return Err("key input requires an opted-in custom surface".into());
        }
        self.key_input(key, Phase::KeyDown, name, 0, false)?;
        self.key_input(key, Phase::KeyUp, name, 0, false)?;
        Ok(())
    }

    pub(super) fn surface_key(&mut self, message: &MSG) -> Result<bool> {
        let phase = match message.message {
            WM_KEYDOWN | WM_SYSKEYDOWN => Phase::KeyDown,
            WM_KEYUP | WM_SYSKEYUP => Phase::KeyUp,
            _ => return Ok(false),
        };
        let Some(&(_, name)) = KEYS.iter().find(|(code, _)| *code == message.wParam as u16) else {
            return Ok(false);
        };
        let Some(key) = self.surface.key(message.hwnd) else {
            return Ok(false);
        };
        self.key_input(
            key,
            phase,
            name,
            native::modifiers(),
            phase == Phase::KeyDown && message.lParam & (1 << 30) != 0,
        )
    }
}

pub(crate) fn queue_surface_mouse(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) {
    APP.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let Some(app) = state.as_mut() else { return };
        if app.driven || !app.select_message(hwnd) {
            return;
        }
        let Some(key) = app.surface.key(hwnd) else {
            return;
        };
        let phase = native::phase(message);
        let location = if phase == Phase::Cancel {
            None
        } else {
            native::hit(hwnd, lparam).and_then(|(hit, x, y)| {
                let owner = app.surface.for_handle(hit)?;
                Some((app.surface.at(owner)?.key(hit)?, x, y))
            })
        };
        app.pointer(
            key,
            phase,
            location,
            native::mouse_buttons(wparam),
            native::modifiers(),
            true,
        );
    });
}
