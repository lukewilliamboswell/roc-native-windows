use crate::wire::{Key, Listener, put_str};
use std::collections::VecDeque;

pub(crate) const CAPTURE: u32 = 4;
pub(crate) const PACED: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Down,
    Move,
    Up,
    Cancel,
    KeyDown,
    KeyUp,
}

impl Phase {
    pub(crate) fn event(self) -> &'static str {
        match self {
            Self::Down => "pointerdown",
            Self::Move => "pointermove",
            Self::Up => "pointerup",
            Self::Cancel => "pointercancel",
            Self::KeyDown => "keydown",
            Self::KeyUp => "keyup",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) key: Key,
    pub(crate) hwnd: usize,
    pub(crate) node: u32,
    pub(crate) listener: Listener,
}

pub(crate) struct Packet {
    pub(crate) target: Target,
    pub(crate) phase: Phase,
    pub(crate) payload: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Queue {
    ready: VecDeque<Packet>,
    paced: VecDeque<Packet>,
}

impl Queue {
    pub(crate) fn push(&mut self, packet: Packet) {
        if packet.phase == Phase::Move && packet.target.listener.policies & PACED != 0 {
            // Mouse uses one stable pointer ID. The target includes both
            // mount identity and the listener observed with this sample.
            self.paced.retain(|prior| prior.target != packet.target);
            self.paced.push_back(packet);
        } else {
            self.frame();
            self.ready.push_back(packet);
        }
    }

    pub(crate) fn frame(&mut self) {
        self.ready.append(&mut self.paced);
    }

    pub(crate) fn pop(&mut self) -> Option<Packet> {
        self.ready.pop_front()
    }
}

/// Normalize against the live client pixels. The last drawable pixel reaches
/// the endpoint while the first pixel beyond the client remains outside.
pub(crate) fn normalized(x: i32, y: i32, width: i32, height: i32) -> Option<(u16, u16)> {
    if x < 0 || y < 0 || x >= width || y >= height {
        return None;
    }
    let axis = |at: i32, size: i32| {
        let span = i64::from((size - 1).max(1));
        ((i64::from(at) * 10_000 + span / 2) / span) as u16
    };
    Some((axis(x, width), axis(y, height)))
}

pub(crate) fn pointer_payload(
    phase: Phase,
    location: Option<(&str, u16, u16)>,
    buttons: u16,
    modifiers: u8,
) -> Vec<u8> {
    let mut bytes = 1u32.to_le_bytes().to_vec(); // One stable mouse pointer.
    bytes.push(0); // Mouse.
    bytes.push(u8::from(matches!(phase, Phase::Down | Phase::Up))); // Main or None.
    bytes.extend_from_slice(&buttons.to_le_bytes());
    // Pointer orders modifiers Shift, Ctrl, Alt, Meta. Key starts Ctrl, Alt, Shift, Meta.
    bytes.push((modifiers & 8) | (modifiers & 3) << 1 | (modifiers & 4) >> 2);
    bytes.push(1); // Primary pointer.
    if let Some((zone, x, y)) = location.filter(|_| phase != Phase::Cancel) {
        bytes.push(1);
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        put_str(&mut bytes, zone);
    } else {
        bytes.push(0);
    }
    bytes
}

pub(crate) fn navigation(key: &str) -> bool {
    matches!(
        key,
        "ArrowLeft"
            | "ArrowRight"
            | "ArrowUp"
            | "ArrowDown"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
    )
}

pub(crate) fn key_payload(
    mut map: &str,
    key: &str,
    modifiers: u8,
    repeat: bool,
) -> Option<(bool, Vec<u8>)> {
    if !navigation(key) {
        return None;
    }
    while !map.is_empty() {
        if matches!(map, "a*" | "o*") {
            return Some((false, encode_key(key, key, modifiers, repeat)));
        }
        let (header, rest) = map.split_at_checked(6)?;
        let [prevent, site, ctrl, alt, shift, meta] = *header.as_bytes() else {
            return None;
        };
        if !matches!(prevent, b'p' | b'k') || !matches!(site, b'a' | b'o') {
            return None;
        }
        let mask = [(ctrl, b'C'), (alt, b'A'), (shift, b'S'), (meta, b'M')]
            .into_iter()
            .enumerate()
            .try_fold(0, |bits, (bit, (mark, expected))| match mark {
                b'-' => Some(bits),
                actual if actual == expected => Some(bits | 1 << bit),
                _ => None,
            })?;
        let colon = rest.find(':')?;
        let length = rest[..colon]
            .parse::<usize>()
            .ok()
            .filter(|&len| len != 0)?;
        let (chord, following) = rest[colon + 1..].split_at_checked(length)?;
        if (chord == key || chord.strip_prefix("code:") == Some(key)) && mask == modifiers {
            return Some((prevent == b'p', encode_key(key, chord, modifiers, repeat)));
        }
        map = following;
    }
    None
}

fn encode_key(key: &str, chord: &str, modifiers: u8, repeat: bool) -> Vec<u8> {
    let mut payload = vec![modifiers | u8::from(repeat) << 4];
    put_str(&mut payload, key);
    put_str(&mut payload, key); // Navigation key code and key have the same name.
    payload.push(modifiers);
    put_str(&mut payload, chord);
    payload
}

#[cfg(windows)]
pub(crate) mod native {
    use super::Phase;
    use crate::Result;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, DrawFocusRect, GetDC, InvalidateRect, ReleaseDC, ScreenToClient,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetFocus, GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const SUBCLASS: usize = 0x5243_494e;

    pub(crate) fn attach(hwnd: HWND, keys: bool) -> Result<()> {
        // SAFETY: The custom surface HWND and subclass live on the app UI thread.
        unsafe {
            if SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, usize::from(keys)) == 0 {
                return Err("could not attach native surface input".into());
            }
            if keys {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_TABSTOP as isize);
            }
        }
        Ok(())
    }

    unsafe extern "system" fn subclass(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        id: usize,
        keys: usize,
    ) -> LRESULT {
        // SAFETY: The window and all messages refer to this UI thread's subclass.
        unsafe {
            match message {
                WM_NCHITTEST => return HTCLIENT as isize,
                WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE | WM_CAPTURECHANGED
                | WM_CANCELMODE => {
                    crate::window::queue_surface_mouse(hwnd, message, wparam, lparam);
                }
                WM_SETFOCUS | WM_KILLFOCUS if keys != 0 => {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                WM_NCDESTROY => {
                    RemoveWindowSubclass(hwnd, Some(subclass), id);
                }
                _ => {}
            }
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            if message == WM_PAINT && keys != 0 && GetFocus() == hwnd {
                let dc = GetDC(hwnd);
                if !dc.is_null() {
                    let mut rect = RECT::default();
                    if GetClientRect(hwnd, &mut rect) != 0 && rect.right > 4 && rect.bottom > 4 {
                        rect.left += 2;
                        rect.top += 2;
                        rect.right -= 2;
                        rect.bottom -= 2;
                        DrawFocusRect(dc, &rect);
                    }
                    ReleaseDC(hwnd, dc);
                }
            }
            result
        }
    }

    pub(crate) fn modifiers() -> u8 {
        // SAFETY: Read thread-message modifier state using documented key codes.
        unsafe {
            let down = |key| GetKeyState(i32::from(key)) < 0;
            u8::from(down(VK_CONTROL))
                | u8::from(down(VK_MENU)) << 1
                | u8::from(down(VK_SHIFT)) << 2
                | u8::from(down(VK_LWIN) || down(VK_RWIN)) << 3
        }
    }

    pub(crate) fn phase(message: u32) -> Phase {
        match message {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => Phase::Down,
            WM_LBUTTONUP => Phase::Up,
            WM_MOUSEMOVE => Phase::Move,
            _ => Phase::Cancel,
        }
    }

    pub(crate) fn mouse_buttons(wparam: WPARAM) -> u16 {
        let state = wparam as u16;
        (state & 3) | (state & 0x70) >> 2
    }

    pub(crate) fn hit(hwnd: HWND, lparam: LPARAM) -> Option<(HWND, u16, u16)> {
        let mut point = POINT {
            x: i32::from(lparam as i16),
            y: i32::from((lparam >> 16) as i16),
        };
        // SAFETY: The source HWND is live; points and rectangle are writable.
        unsafe {
            if ClientToScreen(hwnd, &mut point) == 0 {
                return None;
            }
            let mut target = WindowFromPoint(point);
            if target.is_null() {
                return None;
            }
            // A static child can be skipped by WindowFromPoint. Use its
            // top-level window to find the deepest visible client child.
            target = GetAncestor(target, GA_ROOT);
            for _ in 0..32 {
                let mut client = point;
                if ScreenToClient(target, &mut client) == 0 {
                    return None;
                }
                let child = ChildWindowFromPointEx(
                    target,
                    client,
                    CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
                );
                if child == target || child.is_null() {
                    let mut rect = RECT::default();
                    if GetClientRect(target, &mut rect) == 0 {
                        return None;
                    }
                    let (x, y) = super::normalized(client.x, client.y, rect.right, rect.bottom)?;
                    return Some((target, x, y));
                }
                target = child;
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_client_edges_with_rounding_and_encodes_shift_separately() {
        assert_eq!(normalized(80, 2, 161, 8), Some((5000, 2857)));
        assert_eq!(normalized(160, 7, 161, 8), Some((10_000, 10_000)));
        assert_eq!(normalized(161, 7, 161, 8), None);
        let data = pointer_payload(Phase::Down, Some(("compare", 5000, 5000)), 1, 4);
        assert_eq!(&data[..11], &[1, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1]);
        assert_eq!(
            pointer_payload(Phase::Cancel, Some(("ignored", 0, 0)), 0, 0)[10],
            0
        );
        assert_eq!(CAPTURE, 4);
    }

    #[test]
    fn coalesces_only_the_same_mount_and_flushes_before_release() {
        let target = Target {
            key: 4,
            hwnd: 8,
            node: 4,
            listener: Listener {
                handler: 7,
                kind: 3,
                policies: PACED,
            },
        };
        let packet = |phase, key, n| Packet {
            target: Target { key, ..target },
            phase,
            payload: vec![n],
        };
        let mut queue = Queue::default();
        queue.push(packet(Phase::Down, 4, 1));
        queue.push(packet(Phase::Move, 4, 2));
        queue.push(packet(Phase::Move, 9, 3));
        queue.push(packet(Phase::Move, 4, 4));
        queue.push(packet(Phase::Up, 4, 5));
        let actual: Vec<_> = std::iter::from_fn(|| queue.pop())
            .map(|p| p.payload[0])
            .collect();
        assert_eq!(actual, [1, 3, 4, 5]);
    }

    #[test]
    fn matches_exact_navigation_chords_and_respects_keep() {
        let map = "paC---9:ArrowLeftka--S-4:Homea*";
        let (prevent, payload) = key_payload(map, "ArrowLeft", 1, true).unwrap();
        assert!(prevent);
        assert_eq!(payload[0], 17);
        assert_eq!(
            key_payload(map, "ArrowLeft", 0, false).map(|pair| pair.0),
            Some(false)
        );
        assert_eq!(
            key_payload(map, "Home", 4, false).map(|pair| pair.0),
            Some(false)
        );
        assert!(key_payload(map, "Tab", 0, false).is_none());
        assert_eq!(Phase::KeyDown.event(), "keydown");
        assert_eq!(Phase::KeyUp.event(), "keyup");
    }
}
