//! Project controlled tree state into the Win32 common control.

use crate::Result;
use crate::tree_model::{MAX_NODES, TreeItem, TreeModel};
use crate::view::wide;
use crate::wire::{Key, Kind, Tree};
use std::collections::{BTreeMap, VecDeque};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

pub(crate) enum TreeIntent {
    Select(String),
    Expand { key: String, expanded: bool },
}

impl TreeIntent {
    pub(crate) fn event(self) -> (&'static str, String) {
        match self {
            Self::Select(key) => ("change", key),
            Self::Expand { key, expanded } => ("expand", format!("{}{key}", u8::from(expanded))),
        }
    }
}

#[derive(Default)]
pub(crate) struct TreeProjection {
    model: TreeModel,
    handles: BTreeMap<String, HTREEITEM>,
    keys: BTreeMap<HTREEITEM, String>,
    selected: String,
    collapsing: bool,
    pending: VecDeque<TreeIntent>,
}

impl TreeProjection {
    pub(crate) fn sync(&mut self, hwnd: HWND, tree: &Tree, owner: Key) -> Result<()> {
        let model = TreeModel::read(metadata(tree, owner)?)?;
        let retained = model.retained_from(&self.model);
        let structure_changed =
            retained.len() != self.model.nodes.len() || retained.len() != model.nodes.len();
        let first = structure_changed
            .then(|| self.keys.get(&get_item(hwnd, TVGN_FIRSTVISIBLE)).cloned())
            .flatten();

        for key in &self.model.order {
            let parent = self.model.nodes[key].item.parent.as_deref();
            if !retained.contains(key.as_str())
                && parent.is_none_or(|parent| retained.contains(parent))
            {
                let item = self.handles[key];
                // SAFETY: The item is a live subtree root in the owning native control.
                if unsafe { SendMessageW(hwnd, TVM_DELETEITEM, 0, item) } == 0 {
                    return Err("could not remove a native tree item".into());
                }
            }
        }
        self.handles
            .retain(|key, _| retained.contains(key.as_str()));
        self.keys.retain(|_, key| retained.contains(key.as_str()));

        for key in &model.order {
            let node = &model.nodes[key].item;
            if let Some(&item) = self.handles.get(key) {
                if self.model.nodes[key].item.label != node.label {
                    let mut title = wide(&node.label);
                    let mut native = text_item(item, &mut title);
                    // SAFETY: The tree copies this temporary UTF-16 string synchronously.
                    if unsafe {
                        SendMessageW(hwnd, TVM_SETITEMW, 0, &mut native as *mut _ as LPARAM)
                    } == 0
                    {
                        return Err("could not rename a native tree item".into());
                    }
                }
                continue;
            }
            let parent = node
                .parent
                .as_ref()
                .map_or(TVI_ROOT, |parent| self.handles[parent]);
            let after = model
                .predecessor(key)
                .map_or(TVI_FIRST, |sibling| self.handles[sibling]);
            let mut title = wide(&node.label);
            let mut native = TVINSERTSTRUCTW {
                hParent: parent,
                hInsertAfter: after,
                Anonymous: TVINSERTSTRUCTW_0 {
                    item: text_item(0, &mut title),
                },
            };
            // SAFETY: The parent and predecessor are live, and text is copied synchronously.
            let item =
                unsafe { SendMessageW(hwnd, TVM_INSERTITEMW, 0, &mut native as *mut _ as LPARAM) };
            if item == 0 {
                return Err("could not insert a native tree item".into());
            }
            self.handles.insert(key.clone(), item);
            self.keys.insert(item, key.clone());
        }

        self.model = model;
        self.selected = tree.node(owner).attr("selected").unwrap_or("").into();
        self.restore(hwnd);
        if let Some(key) = first.filter(|key| self.visible(key)) {
            // SAFETY: This key names a live visible item in this tree.
            unsafe {
                SendMessageW(
                    hwnd,
                    TVM_SELECTITEM,
                    TVGN_FIRSTVISIBLE as usize,
                    self.handles[&key],
                )
            };
        }
        Ok(())
    }

    pub(crate) fn restore(&self, hwnd: HWND) {
        for key in &self.model.order {
            if self.model.is_branch(key) && self.model.nodes[key].item.expanded {
                let item = self.handles[key];
                if !is_expanded(hwnd, item) {
                    expand(hwnd, item, true);
                }
            }
        }

        // Move the caret out of a subtree before collapsing it. Win32 can
        // reveal a selected descendant while moving its native caret.
        let caret = self
            .model
            .visible_ancestor(&self.selected)
            .map_or(0, |key| self.handles[key]);
        if get_item(hwnd, TVGN_CARET) != caret {
            // SAFETY: Zero clears selection, otherwise caret is a visible live item.
            unsafe { SendMessageW(hwnd, TVM_SELECTITEM, TVGN_CARET as usize, caret) };
        }
        for key in self.model.order.iter().rev() {
            if self.model.is_branch(key) && !self.model.nodes[key].item.expanded {
                let item = self.handles[key];
                if is_expanded(hwnd, item) {
                    expand(hwnd, item, false);
                }
            }
        }
    }

    pub(crate) fn capture(&mut self, code: u32, item: HTREEITEM, action: u32) -> bool {
        match code {
            TVN_ITEMEXPANDINGW if action == TVE_COLLAPSE && self.keys.contains_key(&item) => {
                self.collapsing = true;
                return false;
            }
            TVN_ITEMEXPANDEDW => {
                let Some(key) = self.keys.get(&item).filter(|key| self.model.is_branch(key)) else {
                    return false;
                };
                let expanded = action == TVE_EXPAND;
                self.pending.push_back(TreeIntent::Expand {
                    key: key.clone(),
                    expanded,
                });
            }
            TVN_SELCHANGEDW if !self.collapsing => {
                let key = if item == 0 {
                    String::new()
                } else if let Some(key) = self.keys.get(&item) {
                    key.clone()
                } else {
                    return false;
                };
                self.pending.push_back(TreeIntent::Select(key));
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn take_intent(&mut self) -> Option<TreeIntent> {
        let intent = self.pending.pop_front()?;
        match &intent {
            TreeIntent::Expand { key, expanded } => {
                if !expanded {
                    self.collapsing = false;
                }
                self.model.is_branch(key).then_some(intent)
            }
            TreeIntent::Select(key) => (key.is_empty() || self.visible(key)).then_some(intent),
        }
    }

    pub(crate) fn selected(&self, hwnd: HWND) -> &str {
        if !self.visible(&self.selected) {
            return "";
        }
        self.keys
            .get(&get_item(hwnd, TVGN_CARET))
            .map_or("", String::as_str)
    }

    pub(crate) fn propose_selection(&self, hwnd: HWND, key: &str) -> Result<Option<TreeIntent>> {
        if !key.is_empty() && !self.visible(key) {
            return Err(format!("no visible tree item has key {key:?}"));
        }
        let item = self.handles.get(key).copied().unwrap_or(0);
        if get_item(hwnd, TVGN_CARET) == item {
            return Ok(None);
        }
        // SAFETY: This is a live visible item, or zero to clear selection.
        unsafe { SendMessageW(hwnd, TVM_SELECTITEM, TVGN_CARET as usize, item) };
        Ok(Some(TreeIntent::Select(key.into())))
    }

    pub(crate) fn expanded(&self, hwnd: HWND, key: &str) -> Result<bool> {
        if !self.model.is_branch(key) {
            return Err(format!("no tree branch has key {key:?}"));
        }
        Ok(is_expanded(hwnd, self.handles[key]))
    }

    pub(crate) fn propose_expansion(
        &self,
        hwnd: HWND,
        key: &str,
        expanded: bool,
    ) -> Result<Option<TreeIntent>> {
        if !self.visible(key) {
            return Err(format!("no visible tree branch has key {key:?}"));
        }
        if self.expanded(hwnd, key)? == expanded {
            return Ok(None);
        }
        Ok(Some(TreeIntent::Expand {
            key: key.into(),
            expanded,
        }))
    }

    fn visible(&self, key: &str) -> bool {
        self.model.visible_ancestor(key) == Some(key)
    }
}

fn metadata(tree: &Tree, owner: Key) -> Result<Vec<TreeItem>> {
    let mut items = Vec::new();
    let mut pending: Vec<_> = tree.node(owner).children.iter().rev().copied().collect();
    while let Some(key) = pending.pop() {
        let node = tree.node(key);
        match &node.kind {
            Kind::Element(tag) if tag == "ui-tree-node" => {
                if items.len() == MAX_NODES {
                    return Err(format!("a tree can have at most {MAX_NODES} nodes"));
                }
                let parent = match node.attr("parent") {
                    None | Some("") => None,
                    Some(value) => Some(
                        value
                            .strip_prefix('1')
                            .ok_or("invalid tree parent encoding")?
                            .into(),
                    ),
                };
                items.push(TreeItem {
                    key: node.attr("key").unwrap_or("").into(),
                    label: node.attr("label").unwrap_or("").into(),
                    parent,
                    expanded: node.attr("expanded") == Some("true"),
                });
            }
            Kind::Element(tag) if !tag.is_empty() => {
                return Err(format!("unexpected {tag:?} under a tree view"));
            }
            _ => pending.extend(node.children.iter().rev().copied()),
        }
    }
    Ok(items)
}

fn text_item(handle: HTREEITEM, utf16: &mut [u16]) -> TVITEMW {
    TVITEMW {
        mask: TVIF_TEXT,
        hItem: handle,
        pszText: utf16.as_mut_ptr(),
        ..TVITEMW::default()
    }
}

fn get_item(hwnd: HWND, relation: u32) -> HTREEITEM {
    // SAFETY: The tree HWND and relationship belong to this UI thread.
    unsafe { SendMessageW(hwnd, TVM_GETNEXTITEM, relation as usize, 0) }
}

fn is_expanded(hwnd: HWND, item: HTREEITEM) -> bool {
    // SAFETY: The item belongs to the live system tree control.
    unsafe {
        SendMessageW(
            hwnd,
            TVM_GETITEMSTATE,
            item as usize,
            TVIS_EXPANDED as LPARAM,
        ) != 0
    }
}

fn expand(hwnd: HWND, item: HTREEITEM, expanded: bool) {
    let action = if expanded { TVE_EXPAND } else { TVE_COLLAPSE };
    // SAFETY: The item belongs to the live system tree control.
    unsafe { SendMessageW(hwnd, TVM_EXPAND, action as usize, item) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_absorbs_its_caret_notification_and_rejects_removed_handles() {
        let mut tree = TreeProjection {
            model: TreeModel::read(vec![
                TreeItem {
                    key: "parent".into(),
                    label: "Parent".into(),
                    parent: None,
                    expanded: true,
                },
                TreeItem {
                    key: "leaf".into(),
                    label: "Été".into(),
                    parent: Some("parent".into()),
                    expanded: false,
                },
            ])
            .unwrap(),
            keys: BTreeMap::from([(1, "parent".into()), (2, "leaf".into())]),
            ..Default::default()
        };
        assert!(!tree.capture(TVN_ITEMEXPANDINGW, 1, TVE_COLLAPSE));
        assert!(!tree.capture(TVN_SELCHANGEDW, 1, TVC_UNKNOWN));
        assert!(tree.capture(TVN_ITEMEXPANDEDW, 1, TVE_COLLAPSE));
        assert!(!tree.capture(TVN_SELCHANGEDW, 1, TVC_UNKNOWN));
        assert_eq!(
            tree.take_intent().unwrap().event(),
            ("expand", "0parent".into())
        );
        assert!(tree.take_intent().is_none());
        assert!(!tree.capture(TVN_SELCHANGEDW, 3, TVC_BYKEYBOARD));
        assert!(tree.capture(TVN_SELCHANGEDW, 2, TVC_BYKEYBOARD));
        assert_eq!(
            tree.take_intent().unwrap().event(),
            ("change", "leaf".into())
        );
    }
}
