//! Validate tree data and decide which native items can stay in place.

use crate::Result;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_KEY_BYTES: usize = 1024;
const MAX_LABEL_UNITS: usize = 1024;

#[derive(Clone)]
pub(crate) struct TreeItem {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) parent: Option<String>,
    pub(crate) expanded: bool,
}

pub(crate) struct Node {
    pub(crate) item: TreeItem,
    children: Vec<String>,
    sibling_index: usize,
}

#[derive(Default)]
pub(crate) struct TreeModel {
    pub(crate) nodes: BTreeMap<String, Node>,
    pub(crate) order: Vec<String>,
    roots: Vec<String>,
}

impl TreeModel {
    pub(crate) fn read(items: Vec<TreeItem>) -> Result<Self> {
        if items.len() > MAX_NODES {
            return Err(format!("a tree can have at most {MAX_NODES} nodes"));
        }
        let mut model = Self::default();
        let mut order = Vec::with_capacity(items.len());
        for item in items {
            if item.key.is_empty() || item.key.len() > MAX_KEY_BYTES || item.key.contains('\0') {
                return Err(format!(
                    "tree keys need 1 to {MAX_KEY_BYTES} bytes without NUL"
                ));
            }
            if item.label.encode_utf16().count() > MAX_LABEL_UNITS || item.label.contains('\0') {
                return Err(format!(
                    "tree labels can have at most {MAX_LABEL_UNITS} UTF-16 units without NUL"
                ));
            }
            let key = item.key.clone();
            order.push(key.clone());
            if model
                .nodes
                .insert(
                    key.clone(),
                    Node {
                        item,
                        children: Vec::new(),
                        sibling_index: 0,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate tree key {key:?}"));
            }
        }
        for key in order {
            let parent = model.nodes[&key].item.parent.clone();
            let siblings = if let Some(parent) = parent {
                &mut model
                    .nodes
                    .get_mut(&parent)
                    .ok_or_else(|| format!("tree key {key:?} has missing parent {parent:?}"))?
                    .children
            } else {
                &mut model.roots
            };
            let index = siblings.len();
            siblings.push(key.clone());
            model.nodes.get_mut(&key).unwrap().sibling_index = index;
        }

        let mut pending: Vec<_> = model.roots.iter().rev().map(|key| (key, 1)).collect();
        while let Some((key, depth)) = pending.pop() {
            if depth > MAX_DEPTH {
                return Err(format!("native tree depth exceeds {MAX_DEPTH}"));
            }
            model.order.push(key.clone());
            pending.extend(
                model.nodes[key]
                    .children
                    .iter()
                    .rev()
                    .map(|child| (child, depth + 1)),
            );
        }
        if model.order.len() != model.nodes.len() {
            return Err("tree parents contain a cycle".into());
        }
        Ok(model)
    }

    pub(crate) fn is_branch(&self, key: &str) -> bool {
        self.nodes
            .get(key)
            .is_some_and(|node| !node.children.is_empty())
    }

    pub(crate) fn visible_ancestor<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        let mut visible = self.nodes.contains_key(key).then_some(key)?;
        let mut parent = self.nodes[key].item.parent.as_deref();
        while let Some(key) = parent {
            let node = &self.nodes[key];
            if !node.item.expanded {
                visible = key;
            }
            parent = node.item.parent.as_deref();
        }
        Some(visible)
    }

    pub(crate) fn predecessor(&self, key: &str) -> Option<&str> {
        let node = &self.nodes[key];
        let before = node.sibling_index.checked_sub(1)?;
        Some(match &node.item.parent {
            Some(parent) => self.nodes[parent].children[before].as_str(),
            None => &self.roots[before],
        })
    }

    /// Keep the largest ordered subset of siblings whose parent survives.
    pub(crate) fn retained_from(&self, old: &Self) -> BTreeSet<&str> {
        let mut retained = BTreeSet::new();
        self.retain_siblings(old, None, &self.roots, &mut retained);
        for key in &self.order {
            if retained.contains(key.as_str()) {
                self.retain_siblings(old, Some(key), &self.nodes[key].children, &mut retained);
            }
        }
        retained
    }

    fn retain_siblings<'a>(
        &'a self,
        old: &Self,
        parent: Option<&str>,
        siblings: &'a [String],
        retained: &mut BTreeSet<&'a str>,
    ) {
        let mut tails: Vec<(usize, usize)> = Vec::new();
        let mut links = vec![None; siblings.len()];
        for (index, key) in siblings.iter().enumerate() {
            let Some(previous) = old
                .nodes
                .get(key)
                .filter(|node| node.item.parent.as_deref() == parent)
            else {
                continue;
            };
            let position = previous.sibling_index;
            let length = tails.partition_point(|&(tail, _)| tail < position);
            links[index] = length.checked_sub(1).map(|last| tails[last].1);
            if length == tails.len() {
                tails.push((position, index));
            } else {
                tails[length] = (position, index);
            }
        }
        let mut cursor = tails.last().map(|&(_, index)| index);
        while let Some(index) = cursor {
            retained.insert(&siblings[index]);
            cursor = links[index];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, parent: Option<&str>) -> TreeItem {
        TreeItem {
            key: key.into(),
            label: "Été 東京 🐈".into(),
            parent: parent.map(str::to_owned),
            expanded: true,
        }
    }

    #[test]
    fn validates_identity_parentage_and_bounds() {
        for (items, message) in [
            (vec![item("same", None), item("same", None)], "duplicate"),
            (vec![item("", None)], "tree keys"),
            (vec![item("lost", Some("absent"))], "missing parent"),
            (vec![item("a", Some("b")), item("b", Some("a"))], "cycle"),
            (vec![item("many", None); MAX_NODES + 1], "at most"),
            (
                vec![TreeItem {
                    label: "🐈".repeat(MAX_LABEL_UNITS),
                    ..item("long", None)
                }],
                "tree labels",
            ),
            (
                (0..=MAX_DEPTH)
                    .map(|n| TreeItem {
                        key: n.to_string(),
                        parent: n.checked_sub(1).map(|parent| parent.to_string()),
                        ..item("deep", None)
                    })
                    .collect(),
                "depth",
            ),
        ] {
            assert!(
                TreeModel::read(items).err().unwrap().contains(message),
                "{message}"
            );
        }
        let model = TreeModel::read(vec![item("leaf", Some("root")), item("root", None)]).unwrap();
        assert_eq!(model.order, ["root", "leaf"]);
        assert!(model.is_branch("root"));
        assert!(!model.is_branch("leaf"));
        assert_eq!(model.nodes["leaf"].item.label, "Été 東京 🐈");
    }

    #[test]
    fn retains_existing_handles_on_insert_and_reorder() {
        let before = TreeModel::read(vec![
            item("a", None),
            item("b", None),
            item("moving", None),
            item("child", Some("moving")),
        ])
        .unwrap();
        let after = TreeModel::read(vec![
            item("moving", None),
            item("child", Some("moving")),
            item("a", None),
            item("new", None),
            item("b", None),
        ])
        .unwrap();
        assert_eq!(after.retained_from(&before), BTreeSet::from(["a", "b"]));
        assert_eq!(after.predecessor("b"), Some("new"));
        let moved = TreeModel::read(vec![item("a", None), item("child", Some("a"))]).unwrap();
        assert_eq!(moved.retained_from(&before), BTreeSet::from(["a"]));
    }

    #[test]
    fn locates_the_outermost_visible_ancestor_after_collapse() {
        let model = TreeModel::read(vec![
            item("leaf", Some("inner")),
            TreeItem {
                expanded: false,
                ..item("inner", Some("outer"))
            },
            TreeItem {
                expanded: false,
                ..item("outer", None)
            },
        ])
        .unwrap();
        assert_eq!(model.visible_ancestor("leaf"), Some("outer"));
        assert_eq!(model.visible_ancestor("inner"), Some("outer"));
        assert_eq!(model.visible_ancestor("absent"), None);
    }
}
