//! Native projection of stable choice keys onto control-local indexes.

use crate::Result;
use crate::view::{Control, wide};
use crate::wire::{Key, Kind, Tree};
use std::collections::BTreeSet;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::UI::Controls::BST_CHECKED;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

#[derive(Clone, PartialEq, Eq)]
struct Choice {
    key: String,
    label: String,
}

#[derive(Default)]
pub(crate) struct Choices {
    items: Vec<Choice>,
    radios: Vec<HWND>,
    selected: Option<usize>,
    dropdown_open: bool,
}

struct Messages {
    insert: u32,
    delete: u32,
    get_selected: u32,
    set_selected: u32,
    get_top: u32,
    set_top: u32,
}

impl Messages {
    fn of(kind: Control) -> Self {
        match kind {
            Control::ComboBox => Self {
                insert: CB_INSERTSTRING,
                delete: CB_DELETESTRING,
                get_selected: CB_GETCURSEL,
                set_selected: CB_SETCURSEL,
                get_top: CB_GETTOPINDEX,
                set_top: CB_SETTOPINDEX,
            },
            Control::ListBox => Self {
                insert: LB_INSERTSTRING,
                delete: LB_DELETESTRING,
                get_selected: LB_GETCURSEL,
                set_selected: LB_SETCURSEL,
                get_top: LB_GETTOPINDEX,
                set_top: LB_SETTOPINDEX,
            },
            _ => unreachable!("only native option lists use index messages"),
        }
    }
}

impl Choices {
    pub(crate) fn sync(
        &mut self,
        hwnd: HWND,
        font: HFONT,
        kind: Control,
        tree: &Tree,
        owner: Key,
    ) -> Result<()> {
        let items = options(tree, owner)?;
        let requested = tree.node(owner).attr("selected").unwrap_or("");
        let selected = items.iter().position(|item| item.key == requested);
        let mut top = None;
        let controlled_changed = self.items != items || self.selected != selected;
        if kind == Control::RadioGroup {
            if selected.is_none() {
                return Err("a radio group needs a selected key from its options".into());
            }
            self.sync_radios(hwnd, font, &items)?;
        } else if self.items != items {
            top = self.sync_options(hwnd, kind, &items)?;
        }
        self.items = items;
        self.selected = selected;
        if controlled_changed || !self.dropdown_open {
            self.restore(hwnd, kind);
        }
        if let Some(top) = top {
            // SAFETY: hwnd is the option list and top is a mounted index.
            unsafe { SendMessageW(hwnd, Messages::of(kind).set_top, top, 0) };
        }
        Ok(())
    }

    fn sync_options(
        &mut self,
        hwnd: HWND,
        kind: Control,
        items: &[Choice],
    ) -> Result<Option<usize>> {
        let messages = Messages::of(kind);
        // SAFETY: hwnd is the option list owned by this view on the UI thread.
        let top = unsafe { SendMessageW(hwnd, messages.get_top, 0, 0) };
        let old_top = usize::try_from(top).ok();
        let anchor = old_top.and_then(|top| {
            self.items
                .get(top..)?
                .iter()
                .find_map(|old| items.iter().position(|new| new.key == old.key))
        });

        // Delete removed options before inserting so native indexes remain valid.
        let retained: BTreeSet<_> = items.iter().map(|item| item.key.as_str()).collect();
        for index in (0..self.items.len()).rev() {
            if !retained.contains(self.items[index].key.as_str()) {
                // SAFETY: This index exists in the current native list.
                if unsafe { SendMessageW(hwnd, messages.delete, index, 0) } < 0 {
                    return Err("could not remove a native choice".into());
                }
                self.items.remove(index);
            }
        }
        for (index, item) in items.iter().enumerate() {
            if self.items.get(index) == Some(item) {
                continue;
            }
            if let Some(relative) = self.items[index..]
                .iter()
                .position(|old| old.key == item.key)
            {
                let old = index + relative;
                // SAFETY: This index exists in the current native list.
                if unsafe { SendMessageW(hwnd, messages.delete, old, 0) } < 0 {
                    return Err("could not move or rename a native choice".into());
                }
                self.items.remove(old);
            }
            let label = wide(&item.label);
            // SAFETY: The option list copies this UTF-16 string synchronously.
            if unsafe { SendMessageW(hwnd, messages.insert, index, label.as_ptr() as isize) } < 0 {
                return Err("could not add a native choice".into());
            }
            self.items.insert(index, item.clone());
        }

        Ok(anchor)
    }

    fn sync_radios(&mut self, group: HWND, font: HFONT, items: &[Choice]) -> Result<()> {
        if self.radios.is_empty() {
            // The group owns its radio children. Forward the command while
            // retaining the originating child HWND.
            // SAFETY: This is a live group on this UI thread.
            if unsafe { SetWindowSubclass(group, Some(forward_radio_command), 1, 0) } == 0 {
                return Err("could not attach the radio group adapter".into());
            }
            for (index, item) in items.iter().enumerate() {
                let style = BS_AUTORADIOBUTTON as u32 | if index == 0 { WS_GROUP } else { 0 };
                let radio = crate::view::child(group, font, 0, w!("BUTTON"), style)?;
                let label = wide(&item.label);
                // SAFETY: This child belongs to group and copies the label.
                unsafe { SetWindowTextW(radio, label.as_ptr()) };
                self.radios.push(radio);
            }
        } else if self.items != items {
            return Err("radio choices are static for the lifetime of their group".into());
        }
        Ok(())
    }

    pub(crate) fn count(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn contains_radio(&self, hwnd: HWND) -> bool {
        self.radios.contains(&hwnd)
    }

    pub(crate) fn font(&self, font: HFONT) {
        for &radio in &self.radios {
            // SAFETY: Each radio is a live child of its group.
            unsafe { SendMessageW(radio, WM_SETFONT, font as usize, 0) };
        }
    }

    pub(crate) fn arrange(&self, width: i32, row_height: i32, inset: i32) {
        let mut after = HWND_TOP;
        for (index, &radio) in self.radios.iter().enumerate() {
            crate::view::position(
                radio,
                after,
                inset,
                (index as i32 + 1) * row_height,
                (width - 2 * inset).max(1),
                row_height,
            );
            after = radio;
        }
    }

    pub(crate) fn restore(&self, hwnd: HWND, kind: Control) {
        if kind == Control::RadioGroup {
            for (index, &radio) in self.radios.iter().enumerate() {
                let checked = self.selected == Some(index);
                // SAFETY: These live child controls share a parent and UI thread.
                unsafe {
                    SendMessageW(radio, BM_SETCHECK, usize::from(checked), 0);
                    let style = GetWindowLongPtrW(radio, GWL_STYLE);
                    let desired = if checked {
                        style | WS_TABSTOP as isize
                    } else {
                        style & !(WS_TABSTOP as isize)
                    };
                    if style != desired {
                        SetWindowLongPtrW(radio, GWL_STYLE, desired);
                    }
                }
            }
        } else {
            let messages = Messages::of(kind);
            let selected = self.selected.unwrap_or(usize::MAX);
            // SAFETY: The HWND and current index belong to this control.
            unsafe {
                if SendMessageW(hwnd, messages.get_selected, 0, 0) as usize != selected {
                    SendMessageW(hwnd, messages.set_selected, selected, 0);
                }
            }
        }
    }

    pub(crate) fn changed(
        &mut self,
        hwnd: HWND,
        kind: Control,
        origin: HWND,
        code: u32,
    ) -> Option<String> {
        let selected = match (kind, code) {
            (Control::ComboBox, CBN_DROPDOWN) => {
                self.dropdown_open = true;
                return None;
            }
            (Control::ComboBox, CBN_CLOSEUP | CBN_SELENDCANCEL) => {
                self.dropdown_open = false;
                self.restore(hwnd, kind);
                return None;
            }
            (Control::ComboBox, CBN_SELCHANGE) if !self.dropdown_open => self.current(hwnd, kind),
            (Control::ComboBox, CBN_SELENDOK) if self.dropdown_open => self.current(hwnd, kind),
            (Control::ListBox, LBN_SELCHANGE) => self.current(hwnd, kind),
            (Control::RadioGroup, BN_CLICKED) => {
                self.radios.iter().position(|&radio| radio == origin)
            }
            _ => return None,
        };
        if selected == self.selected {
            return None;
        }
        Some(self.key(selected).to_owned())
    }

    pub(crate) fn selected_key(&self, hwnd: HWND, kind: Control) -> &str {
        self.key(self.current(hwnd, kind))
    }

    fn key(&self, index: Option<usize>) -> &str {
        index
            .and_then(|index| self.items.get(index))
            .map_or("", |item| item.key.as_str())
    }

    fn current(&self, hwnd: HWND, kind: Control) -> Option<usize> {
        if kind == Control::RadioGroup {
            return self.radios.iter().position(|&radio| {
                // SAFETY: The radio belongs to this group.
                unsafe { SendMessageW(radio, BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
            });
        }
        // SAFETY: hwnd is the option list owned by the view.
        let index = unsafe { SendMessageW(hwnd, Messages::of(kind).get_selected, 0, 0) };
        usize::try_from(index)
            .ok()
            .filter(|&index| index < self.items.len())
    }

    pub(crate) fn propose(&self, hwnd: HWND, kind: Control, key: &str) -> Result<(HWND, u32)> {
        let selected = if key.is_empty() && kind != Control::RadioGroup {
            None
        } else {
            Some(
                self.items
                    .iter()
                    .position(|item| item.key == key)
                    .ok_or_else(|| format!("no option has key {key:?}"))?,
            )
        };
        if kind == Control::RadioGroup {
            return Ok((self.radios[selected.unwrap()], BN_CLICKED));
        }
        // SAFETY: The HWND belongs to the selected list; -1 removes its selection.
        unsafe {
            SendMessageW(
                hwnd,
                Messages::of(kind).set_selected,
                selected.unwrap_or(usize::MAX),
                0,
            )
        };
        let code = if kind == Control::ComboBox {
            CBN_SELCHANGE
        } else {
            LBN_SELCHANGE
        };
        Ok((hwnd, code))
    }
}

fn options(tree: &Tree, owner: Key) -> Result<Vec<Choice>> {
    let mut items = Vec::new();
    let mut keys = BTreeSet::new();
    let mut pending: Vec<_> = tree.node(owner).children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        let node = tree.node(child);
        match &node.kind {
            Kind::Element(tag) if tag == "ui-choice" => {
                let key = node
                    .attr("key")
                    .filter(|key| !key.is_empty())
                    .ok_or("choice key must not be empty")?;
                if !keys.insert(key) {
                    return Err(format!("duplicate choice key {key:?}"));
                }
                items.push(Choice {
                    key: key.into(),
                    label: node.attr("label").unwrap_or("").into(),
                });
            }
            Kind::Element(tag) if tag.is_empty() => pending.extend(node.children.iter().rev()),
            Kind::Element(tag) => return Err(format!("unexpected {tag:?} under a choice control")),
            _ => pending.extend(node.children.iter().rev()),
        }
    }
    Ok(items)
}

unsafe extern "system" fn forward_radio_command(
    group: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    // SAFETY: Comctl32 dispatches this callback for the installed group only.
    unsafe {
        if message == WM_COMMAND && lparam != 0 {
            return SendMessageW(GetParent(group), message, wparam, lparam);
        }
        DefSubclassProc(group, message, wparam, lparam)
    }
}
