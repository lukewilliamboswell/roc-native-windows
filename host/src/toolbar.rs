use crate::Result;
use crate::wire::Key;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    node: Key,
    key: Option<String>,
    title: String,
    enabled: bool,
    icon: String,
}

#[derive(Clone, Debug)]
struct Entry {
    id: u16,
    item: Item,
}

#[derive(Default)]
struct Commands {
    last: u16,
    mounted: Vec<Entry>,
}

impl Commands {
    fn project(&mut self, items: Vec<Item>) -> Result<Vec<Entry>> {
        items
            .into_iter()
            .map(|item| {
                let id = if let Some(entry) = self
                    .mounted
                    .iter()
                    .find(|entry| entry.item.node == item.node && entry.item.key == item.key)
                {
                    entry.id
                } else {
                    self.last = self
                        .last
                        .checked_add(1)
                        .ok_or("toolbar command IDs exhausted")?;
                    self.last
                };
                Ok(Entry { id, item })
            })
            .collect()
    }

    fn dispatch(&self, id: u16) -> Option<Key> {
        self.mounted.iter().find_map(|entry| {
            (entry.id == id && entry.item.enabled && entry.item.key.is_some())
                .then_some(entry.item.node)
        })
    }
}

#[cfg(windows)]
mod native {
    use super::{Commands, Entry, Item, Key, Result};
    use crate::command_icon::Images;
    use crate::view::{scale, wide};
    use crate::window::enabled_flag;
    use crate::wire::{Kind, Tree};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
    use windows_sys::Win32::UI::Controls::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOREDRAW, SWP_NOZORDER, SendMessageW,
        SetWindowPos,
    };

    #[derive(Default)]
    pub(crate) struct Toolbar {
        commands: Commands,
        icons: ToolbarIcons,
    }

    #[derive(Default)]
    struct ToolbarIcons {
        names: BTreeSet<String>,
        dpi: u32,
        indices: BTreeMap<String, i32>,
        list: Option<Images>,
    }

    impl ToolbarIcons {
        fn sync(&mut self, hwnd: HWND, entries: &[Entry], assets: &Path, dpi: u32) -> bool {
            let names = entries
                .iter()
                .map(|entry| &entry.item.icon)
                .filter(|name| !name.is_empty())
                .cloned()
                .collect::<BTreeSet<_>>();
            if names == self.names && (names.is_empty() || dpi == self.dpi) {
                return false;
            }
            let mut indices = BTreeMap::new();
            let list = (!names.is_empty())
                .then(|| Images::new(dpi, names.len() as i32))
                .flatten()
                .filter(|list| {
                    for name in &names {
                        if let Some(index) = list.add(assets, name, dpi) {
                            indices.insert(name.clone(), index);
                        }
                    }
                    !indices.is_empty()
                });
            if let Some(list) = &list {
                let edge = scale(dpi, 16).max(1) as isize;
                send(hwnd, TB_SETBITMAPSIZE, 0, edge | (edge << 16));
                send(hwnd, TB_SETIMAGELIST, 0, list.handle());
            } else {
                send(hwnd, TB_SETIMAGELIST, 0, 0);
            }
            self.names = names;
            self.dpi = dpi;
            self.indices = indices;
            self.list = list;
            true
        }

        fn index(&self, name: &str) -> i32 {
            self.indices.get(name).copied().unwrap_or(I_IMAGENONE)
        }
    }

    impl Toolbar {
        pub(crate) fn initialize(hwnd: HWND) {
            send(hwnd, TB_BUTTONSTRUCTSIZE, size_of::<TBBUTTON>(), 0);
            send(
                hwnd,
                TB_SETEXTENDEDSTYLE,
                0,
                TBSTYLE_EX_MIXEDBUTTONS as isize,
            );
            send(hwnd, TB_SETMAXTEXTROWS, 1, 0);
        }

        pub(crate) fn sync(
            &mut self,
            hwnd: HWND,
            tree: &Tree,
            key: Key,
            assets: &Path,
            dpi: u32,
        ) -> Result<()> {
            let desired = self.commands.project(read(tree, key)?)?;
            let mut live: Vec<_> = self.commands.mounted.iter().map(|entry| entry.id).collect();
            let hot = self
                .commands
                .mounted
                .get(send(hwnd, TB_GETHOTITEM, 0, 0) as usize)
                .map(|entry| entry.id);
            let images_changed = self.icons.sync(hwnd, &desired, assets, dpi);
            let mut changed = images_changed;
            for index in (0..live.len()).rev() {
                if !desired.iter().any(|entry| entry.id == live[index]) {
                    checked(
                        send(hwnd, TB_DELETEBUTTON, index, 0),
                        "delete a toolbar item",
                    )?;
                    live.remove(index);
                    changed = true;
                }
            }
            for (index, entry) in desired.iter().enumerate() {
                if let Some(old_position) = live.iter().position(|&id| id == entry.id) {
                    if old_position != index {
                        checked(
                            send(hwnd, TB_MOVEBUTTON, old_position, index as isize),
                            "move a toolbar item",
                        )?;
                        live.remove(old_position);
                        live.insert(index, entry.id);
                        changed = true;
                    }
                    let old = self
                        .commands
                        .mounted
                        .iter()
                        .find(|old| old.id == entry.id)
                        .unwrap();
                    if old.item.title != entry.item.title {
                        title(hwnd, entry.id, &entry.item.title)?;
                        changed = true;
                    }
                    if entry.item.key.is_some()
                        && (images_changed || old.item.icon != entry.item.icon)
                    {
                        let image = self.icons.index(&entry.item.icon);
                        checked(
                            send(hwnd, TB_CHANGEBITMAP, usize::from(entry.id), image as isize),
                            "update a toolbar icon",
                        )?;
                        changed = true;
                    }
                    if old.item.enabled != entry.item.enabled {
                        checked(
                            send(
                                hwnd,
                                TB_ENABLEBUTTON,
                                usize::from(entry.id),
                                isize::from(entry.item.enabled),
                            ),
                            "set toolbar command availability",
                        )?;
                    }
                } else {
                    insert(hwnd, index, entry, self.icons.index(&entry.item.icon))?;
                    live.insert(index, entry.id);
                    changed = true;
                }
            }
            if changed {
                send(hwnd, TB_AUTOSIZE, 0, 0);
                if let Some(hot_id) = hot
                    && let Some(index) = live.iter().position(|&id| id == hot_id)
                {
                    send(hwnd, TB_SETHOTITEM, index, 0);
                }
            }
            self.commands.mounted = desired;
            Ok(())
        }

        pub(crate) fn dispatch(&self, hwnd: HWND, id: u16) -> Option<Key> {
            (send(hwnd, TB_ISBUTTONENABLED, usize::from(id), 0) != 0)
                .then(|| self.commands.dispatch(id))
                .flatten()
        }

        pub(crate) fn id(&self, node: Key) -> Option<u16> {
            self.commands
                .mounted
                .iter()
                .find(|entry| entry.item.node == node && entry.item.key.is_some())
                .map(|entry| entry.id)
        }

        pub(crate) fn text(&self, hwnd: HWND, id: u16) -> Option<String> {
            let length = send(hwnd, TB_GETBUTTONTEXTW, usize::from(id), 0);
            let length = usize::try_from(length).ok()?;
            let mut buf = vec![0u16; length + 1];
            let written = send(
                hwnd,
                TB_GETBUTTONTEXTW,
                usize::from(id),
                buf.as_mut_ptr() as isize,
            );
            let written = usize::try_from(written).ok()?.min(length);
            Some(String::from_utf16_lossy(&buf[..written]))
        }

        pub(crate) fn height(&self, hwnd: HWND, width: i32, dpi: u32) -> i32 {
            let mut client = RECT::default();
            // SAFETY: The toolbar lives on this thread and RECT is writable.
            unsafe {
                GetClientRect(hwnd, &mut client);
                if client.right != width {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        0,
                        0,
                        width,
                        client.bottom.max(1),
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
                    );
                    send(hwnd, TB_AUTOSIZE, 0, 0);
                }
            }
            let mut size = SIZE::default();
            send(hwnd, TB_GETMAXSIZE, 0, address(&mut size));
            size.cy.max(scale(dpi, 27))
        }
    }

    fn read(tree: &Tree, root: Key) -> Result<Vec<Item>> {
        let mut commands = BTreeSet::new();
        let mut items = Vec::new();
        for &key in &tree.node(root).children {
            let child = tree.node(key);
            let Kind::Element(tag) = &child.kind else {
                continue;
            };
            let item = match tag.as_str() {
                "ui-toolbar-separator" => Item {
                    node: key,
                    key: None,
                    title: String::new(),
                    enabled: false,
                    icon: String::new(),
                },
                "ui-toolbar-button" => {
                    let logical = child.attr("command-key").unwrap_or("");
                    let title = tree.caption(key);
                    if logical.is_empty() || logical.contains('\0') || !commands.insert(logical) {
                        return Err(format!(
                            "toolbar keys must be nonempty, unique and contain no NUL: {logical:?}"
                        ));
                    }
                    if title.is_empty() || title.contains('\0') {
                        return Err(format!(
                            "toolbar command {logical:?} needs a nonempty title without NUL"
                        ));
                    }
                    Item {
                        node: key,
                        key: Some(logical.to_owned()),
                        title,
                        enabled: child.attr("enabled").is_none_or(enabled_flag),
                        icon: child.attr("command-icon").unwrap_or("").into(),
                    }
                }
                _ => return Err(format!("{tag} is not a toolbar item")),
            };
            items.push(item);
        }
        Ok(items)
    }

    fn insert(hwnd: HWND, index: usize, entry: &Entry, image: i32) -> Result<()> {
        let separator = entry.item.key.is_none();
        let text = wide(&entry.item.title);
        let mut button = TBBUTTON {
            iBitmap: if separator { 0 } else { image },
            idCommand: i32::from(entry.id),
            fsState: if entry.item.enabled {
                TBSTATE_ENABLED as u8
            } else {
                0
            },
            fsStyle: if separator {
                BTNS_SEP as u8
            } else {
                (BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT | BTNS_NOPREFIX) as u8
            },
            iString: if separator {
                -1
            } else {
                text.as_ptr() as isize
            },
            ..TBBUTTON::default()
        };
        checked(
            send(hwnd, TB_INSERTBUTTONW, index, address(&mut button)),
            "insert a toolbar item",
        )
    }

    fn title(hwnd: HWND, id: u16, value: &str) -> Result<()> {
        let mut text = wide(value);
        let mut info = TBBUTTONINFOW {
            cbSize: size_of::<TBBUTTONINFOW>() as u32,
            dwMask: TBIF_TEXT,
            pszText: text.as_mut_ptr(),
            ..TBBUTTONINFOW::default()
        };
        checked(
            send(hwnd, TB_SETBUTTONINFOW, usize::from(id), address(&mut info)),
            "update a toolbar title",
        )
    }

    fn checked(value: isize, step: &str) -> Result<()> {
        if value == 0 {
            Err(format!("could not {step}"))
        } else {
            Ok(())
        }
    }

    fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: The caller owns the control and keeps any payload live during this send.
        unsafe { SendMessageW(hwnd, message, wparam, lparam) }
    }

    fn address<T>(value: &mut T) -> isize {
        std::ptr::from_mut(value) as isize
    }
}

#[cfg(windows)]
pub(crate) use native::Toolbar;

#[cfg(test)]
mod tests {
    use super::*;

    fn command(node: Key, title: &str, enabled: bool) -> Item {
        Item {
            node,
            key: Some("mark".into()),
            title: title.into(),
            enabled,
            icon: String::new(),
        }
    }

    #[test]
    fn only_current_enabled_toolbar_ids_dispatch() {
        let mut state = Commands::default();
        state.mounted = state.project(vec![command(1, "Mark read", true)]).unwrap();
        let first = state.mounted[0].id;
        assert_eq!(state.dispatch(first), Some(1));

        state.mounted = state
            .project(vec![command(1, "Mark unread", false)])
            .unwrap();
        assert_eq!(state.mounted[0].id, first);
        assert_eq!(state.dispatch(first), None);

        state.mounted = state.project(vec![command(2, "Mark read", true)]).unwrap();
        assert_ne!(state.mounted[0].id, first);
        assert_eq!(state.dispatch(first), None);
        assert_eq!(state.dispatch(state.mounted[0].id), Some(2));
    }
}
