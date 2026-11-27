//! The little-endian Roc signals command stream and its retained node tree.

use crate::{Result, regional::Snapshot};
use std::collections::BTreeMap;

#[cfg(windows)]
pub(crate) const VERSION: u32 = 23;
#[cfg(windows)]
pub(crate) const EFFECT_NODE: u32 = u32::MAX;
pub(crate) const FOLDER_PORT: &str = "roc-native-windows/folder-files";
pub(crate) const ATTACH_RASTER_PORT: &str = "roc-native-windows/bind-raster";
pub(crate) const DECODE_IMAGE_PORT: &str = "roc-native-windows/decode-image";
pub(crate) const MONOTONIC_PORT: &str = "roc-native-windows/monotonic";
pub(crate) const REGIONAL_PORT: &str = "roc-native-windows/regional";
pub(crate) const FORMAT_TIME_PORT: &str = "roc-native-windows/format-time";
const MAX_DEPTH: usize = 256;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn done(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn raw(&mut self, len: usize) -> Result<&'a [u8]> {
        let (bytes, rest) = self
            .bytes
            .split_at_checked(len)
            .ok_or("truncated wire data")?;
        self.bytes = rest;
        Ok(bytes)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.raw(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.raw(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap()))
    }

    #[cfg(windows)]
    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }

    pub(crate) fn string(&mut self) -> Result<&'a str> {
        let len = self.u32()? as usize;
        std::str::from_utf8(self.raw(len)?).map_err(|_| "invalid UTF-8 in wire data".into())
    }

    pub(crate) fn str(&mut self) -> Result<String> {
        self.string().map(str::to_owned)
    }

    fn name(&mut self) -> Result<String> {
        if self.u8()? == 4 {
            self.str()?;
        }
        self.str()?;
        self.str()
    }

    fn presence(&mut self) -> Result<Option<String>> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.str()?)),
            flag => Err(format!("unknown attribute presence {flag}")),
        }
    }
}

pub(crate) fn put_str(bytes: &mut Vec<u8>, text: &str) {
    bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
}

#[derive(Clone, Default)]
pub(crate) struct Declarations {
    pub(crate) local_keys: Vec<String>,
    pub(crate) media_queries: Vec<String>,
    pub(crate) zone_days: u16,
    pub(crate) session_keys: Vec<String>,
    pub(crate) startup: Vec<u8>,
}

impl Declarations {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut out = Self::default();
        let mut reader = Reader::new(bytes);
        for expected in [2, 5, 6, 10, 11, 14] {
            let tag = reader.u8()?;
            let len = reader.u32()? as usize;
            let section = reader.raw(len)?;
            let mut body = Reader::new(section);
            if tag != expected {
                return Err(format!("expected declaration {expected}, got {tag}"));
            }
            match tag {
                2 => out.local_keys = read_strings(&mut body)?,
                5 => {
                    let count = body.u32()?;
                    if count > 6 {
                        return Err("too many native port declarations".into());
                    }
                    let mut seen = Vec::new();
                    for _ in 0..count {
                        let port = body.string()?;
                        let kind = body.u8()?;
                        if seen.contains(&port)
                            || !matches!(
                                (port, kind),
                                (
                                    FOLDER_PORT
                                        | DECODE_IMAGE_PORT
                                        | MONOTONIC_PORT
                                        | REGIONAL_PORT
                                        | FORMAT_TIME_PORT,
                                    2
                                ) | (ATTACH_RASTER_PORT, 1)
                            )
                            || body.u8()? != 0
                            || body.u32()? != 0
                        {
                            return Err("unsupported native port declaration".into());
                        }
                        seen.push(port);
                    }
                }
                6 => out.media_queries = read_strings(&mut body)?,
                10 => out.zone_days = body.u16()?,
                11 => out.session_keys = read_strings(&mut body)?,
                14 => {
                    out.startup = section.to_vec();
                    continue;
                }
                _ => unreachable!(),
            }
            if !body.done() {
                return Err(format!("unread data in declaration {tag}"));
            }
        }
        if !reader.done() {
            return Err("unread data after declarations".into());
        }
        Ok(out)
    }

    pub(crate) fn environment(
        &self,
        snapshot: &Snapshot,
        mut lookup: impl FnMut(bool, &str) -> Option<String>,
    ) -> Vec<u8> {
        let mut result = Vec::new();
        let mut url = Vec::new();
        put_str(&mut url, "/");
        put_section(&mut result, 1, &url);
        put_section(
            &mut result,
            2,
            &storage_pairs(&self.local_keys, |key| lookup(false, key)),
        );
        put_section(&mut result, 3, &[1]);
        put_section(&mut result, 4, &[1]);
        put_section(&mut result, 6, &vec![0; self.media_queries.len()]);
        put_section(&mut result, 7, &snapshot.now.to_le_bytes());
        put_section(&mut result, 8, &snapshot.seed);
        let mut languages = (snapshot.languages.len() as u32).to_le_bytes().to_vec();
        for language in &snapshot.languages {
            put_str(&mut languages, language);
        }
        put_section(&mut result, 9, &languages);

        let mut zone = Vec::new();
        put_str(&mut zone, ""); // Windows zone keys are not IANA names.
        zone.extend_from_slice(&snapshot.zone.from.to_le_bytes());
        zone.extend_from_slice(&snapshot.zone.until.to_le_bytes());
        zone.extend_from_slice(&(snapshot.zone.offsets.len() as u32).to_le_bytes());
        for (at, minutes) in &snapshot.zone.offsets {
            zone.extend_from_slice(&at.to_le_bytes());
            zone.extend_from_slice(&minutes.to_le_bytes());
        }
        put_section(&mut result, 10, &zone);

        put_section(
            &mut result,
            11,
            &storage_pairs(&self.session_keys, |key| lookup(true, key)),
        );
        result
    }

    pub(crate) fn boot(
        &self,
        snapshot: &Snapshot,
        lookup: impl FnMut(bool, &str) -> Option<String>,
    ) -> Result<Vec<u8>> {
        let mut result = self.environment(snapshot, lookup);
        let mut startup = Reader::new(&self.startup);
        match startup.u8()? {
            0 => {
                let mut root = vec![0];
                put_str(&mut root, "");
                put_str(&mut root, "div");
                put_section(&mut result, 13, &root);
                put_section(&mut result, 14, &[0]);
            }
            1 => {
                let start = startup.bytes;
                startup.name()?;
                put_section(&mut result, 13, &start[..start.len() - startup.bytes.len()]);
                put_section(&mut result, 14, &[1, 0, 0xff, 0xff, 0xff, 0xff]);
            }
            profile => return Err(format!("unknown startup profile {profile}")),
        }
        Ok(result)
    }
}

fn put_section(bytes: &mut Vec<u8>, tag: u8, body: &[u8]) {
    bytes.push(tag);
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(body);
}

fn read_strings(reader: &mut Reader<'_>) -> Result<Vec<String>> {
    (0..reader.u32()?).map(|_| reader.str()).collect()
}

fn storage_pairs(keys: &[String], mut get: impl FnMut(&str) -> Option<String>) -> Vec<u8> {
    let mut bytes = vec![1, 0, 0, 0, 0];
    let mut count = 0_u32;
    for key in keys {
        if let Some(value) = get(key) {
            put_str(&mut bytes, key);
            put_str(&mut bytes, &value);
            count += 1;
        }
    }
    bytes[1..5].copy_from_slice(&count.to_le_bytes());
    bytes
}

pub(crate) type Key = usize;
pub(crate) const ROOT: Key = 0;

#[derive(Clone)]
pub(crate) enum Kind {
    Element(String),
    Text,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Listener {
    pub(crate) handler: u32,
    pub(crate) kind: u8,
    pub(crate) policies: u32,
}

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    pub(crate) text: String,
    pub(crate) children: Vec<Key>,
    pub(crate) attrs: BTreeMap<String, String>,
    listeners: BTreeMap<String, Listener>,
    parent: Option<Key>,
    id: Option<u32>,
    holes: Vec<(Key, HoleKind)>,
}

impl Node {
    fn new(kind: Kind, text: String) -> Self {
        Self {
            kind,
            text,
            children: Vec::new(),
            attrs: BTreeMap::new(),
            listeners: BTreeMap::new(),
            parent: None,
            id: None,
            holes: Vec::new(),
        }
    }

    pub(crate) fn listens(&self, name: &str) -> bool {
        self.listeners.contains_key(name)
    }

    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    fn set_attr(&mut self, name: String, value: Option<String>) {
        if let Some(value) = value {
            self.attrs.insert(name, value);
        } else {
            self.attrs.remove(&name);
        }
    }
}

#[derive(Clone)]
enum HoleKind {
    Text,
    Attr(String),
    Event(String, Listener),
    Id,
    Ref,
    Prop(String),
}

#[derive(Clone)]
struct Hole {
    kind: HoleKind,
    path: Vec<usize>,
}

#[derive(Clone)]
struct TemplateNode {
    node: Node,
    children: Vec<Self>,
}

#[derive(Clone)]
struct Template {
    root: TemplateNode,
    holes: Vec<Hole>,
}

impl Template {
    fn decode(bytes: &[u8]) -> Result<Self> {
        fn node(reader: &mut Reader<'_>, depth: usize) -> Result<TemplateNode> {
            if depth > MAX_DEPTH {
                return Err("template nesting is too deep".into());
            }
            let (node, children) = match reader.u8()? {
                1 => {
                    let mut element = Node::new(Kind::Element(reader.name()?), String::new());
                    reader.u8()?; // Child container.
                    for _ in 0..reader.u8()? {
                        element.attrs.insert(reader.name()?, reader.str()?);
                    }
                    let children = (0..reader.u8()?)
                        .map(|_| node(reader, depth + 1))
                        .collect::<Result<_>>()?;
                    (element, children)
                }
                2 => (Node::new(Kind::Text, reader.str()?), Vec::new()),
                3 => (Node::new(Kind::Comment, reader.str()?), Vec::new()),
                kind => return Err(format!("unknown template node {kind}")),
            };
            Ok(TemplateNode { node, children })
        }

        let mut reader = Reader::new(bytes);
        let root = node(&mut reader, 0)?;
        let mut holes = Vec::new();
        for _ in 0..reader.u8()? {
            let tag = reader.u8()?;
            let path = (0..reader.u8()?)
                .map(|_| {
                    reader.u8()?;
                    Ok(reader.u8()? as usize)
                })
                .collect::<Result<_>>()?;
            let kind = match tag {
                1 => HoleKind::Text,
                2 => HoleKind::Attr(reader.name()?),
                3 => {
                    let handler = u32::from(reader.u8()?);
                    let event = reader.str()?;
                    let kind = reader.u8()?;
                    let policies = reader.u32()?;
                    HoleKind::Event(
                        event,
                        Listener {
                            handler,
                            kind,
                            policies,
                        },
                    )
                }
                4 => HoleKind::Id,
                5 => HoleKind::Ref,
                6 => HoleKind::Prop(property(reader.u8()?)?),
                other => return Err(format!("unknown hole type {other}")),
            };
            holes.push(Hole { kind, path });
        }
        if !reader.done() {
            return Err("unread template data".into());
        }
        Ok(Self { root, holes })
    }
}

fn property(id: u8) -> Result<String> {
    match id {
        1 => Ok("value".into()),
        2 => Ok("checked".into()),
        3 => Ok("pointer-zone".into()),
        4 => Ok("keymap-down".into()),
        5 => Ok("keymap-up".into()),
        _ => Err(format!("unsupported property {id}")),
    }
}

pub(crate) struct Tree {
    nodes: BTreeMap<Key, Node>,
    ids: BTreeMap<u32, Key>,
    refs: BTreeMap<u32, Key>,
    templates: BTreeMap<u32, Template>,
    next: Key,
}

impl Tree {
    pub(crate) fn new() -> Self {
        let mut root = Node::new(Kind::Element(String::new()), String::new());
        root.id = Some(0);
        Self {
            nodes: BTreeMap::from([(ROOT, root)]),
            ids: BTreeMap::from([(0, ROOT)]),
            refs: BTreeMap::new(),
            templates: BTreeMap::new(),
            next: 1,
        }
    }

    #[cfg(windows)]
    pub(crate) fn nodes(&self) -> impl Iterator<Item = (Key, &Node)> {
        self.nodes.iter().map(|(&key, node)| (key, node))
    }

    pub(crate) fn node(&self, key: Key) -> &Node {
        &self.nodes[&key]
    }

    #[cfg(windows)]
    pub(crate) fn parent(&self, key: Key) -> Option<Key> {
        self.nodes.get(&key)?.parent
    }

    pub(crate) fn contains(&self, key: Key) -> bool {
        self.nodes.contains_key(&key)
    }

    pub(crate) fn reference(&self, id: u32) -> Option<Key> {
        self.refs.get(&id).copied()
    }

    pub(crate) fn event(&self, mut key: Key, name: &str) -> Option<(u32, Listener)> {
        loop {
            let node = self.nodes.get(&key)?;
            if let Some(&listener) = node.listeners.get(name) {
                let route = std::iter::successors(Some(key), |&key| self.node(key).parent)
                    .find_map(|key| self.node(key).id)?;
                return Some((route, listener));
            }
            key = node.parent?;
        }
    }

    pub(crate) fn mounted_event(&self, key: Key, name: &str) -> Option<(u32, Listener)> {
        let node = self.nodes.get(&key)?;
        let listener = *node.listeners.get(name)?;
        let route = std::iter::successors(Some(key), |&key| self.node(key).parent)
            .find_map(|key| self.node(key).id)?;
        Some((route, listener))
    }

    pub(crate) fn caption(&self, key: Key) -> String {
        let node = self.node(key);
        let mut text = node.text.clone();
        for &child in &node.children {
            if matches!(self.node(child).kind, Kind::Text) {
                text.push_str(&self.node(child).text);
            }
        }
        text
    }

    fn get(&self, id: u32) -> Result<Key> {
        self.ids
            .get(&id)
            .copied()
            .ok_or_else(|| format!("unknown node {id}"))
    }

    fn node_mut(&mut self, key: Key) -> &mut Node {
        self.nodes.get_mut(&key).unwrap()
    }

    fn allocate(&mut self, node: Node) -> Key {
        let key = self.next;
        self.next += 1;
        self.nodes.insert(key, node);
        key
    }

    fn register(&mut self, key: Key, id: u32) -> Result<()> {
        if self.ids.contains_key(&id) {
            return Err(format!("duplicate node {id}"));
        }
        self.node_mut(key).id = Some(id);
        self.ids.insert(id, key);
        Ok(())
    }

    fn adopt(&mut self, key: Key, parent: Key, before: Option<Key>) -> Result<()> {
        let mut ancestor = Some(parent);
        for depth in 0.. {
            let Some(at) = ancestor else { break };
            if depth > MAX_DEPTH || at == key {
                return Err("cyclic or too deeply nested node tree".into());
            }
            ancestor = self.node(at).parent;
        }
        let children = &mut self.node_mut(parent).children;
        let index = if let Some(before) = before {
            children
                .iter()
                .position(|&child| child == before)
                .ok_or("anchor is outside parent")?
        } else {
            children.len()
        };
        children.insert(index, key);
        self.node_mut(key).parent = Some(parent);
        Ok(())
    }

    fn place(&mut self, key: Key, parent_id: u32, before_id: u32) -> Result<()> {
        let parent = self.get(parent_id)?;
        let before = (before_id != 0).then(|| self.get(before_id)).transpose()?;
        self.adopt(
            key,
            before
                .and_then(|key| self.node(key).parent)
                .unwrap_or(parent),
            before,
        )
    }

    fn detach(&mut self, key: Key) -> Result<Key> {
        let parent = self
            .node_mut(key)
            .parent
            .take()
            .ok_or("cannot detach root node")?;
        self.node_mut(parent).children.retain(|&child| child != key);
        Ok(parent)
    }

    fn forget(&mut self, key: Key) {
        let node = self.nodes.remove(&key).unwrap();
        if let Some(id) = node.id {
            self.ids.remove(&id);
        }
        self.refs.retain(|_, target| *target != key);
        for child in node.children {
            self.forget(child);
        }
    }

    fn clear(&mut self, parent: Key) {
        for child in std::mem::take(&mut self.node_mut(parent).children) {
            self.forget(child);
        }
    }

    fn clone_row(&mut self, template: &TemplateNode, parent: Option<Key>) -> Key {
        let mut node = template.node.clone();
        node.parent = parent;
        let key = self.allocate(node);
        let children = template
            .children
            .iter()
            .map(|child| self.clone_row(child, Some(key)))
            .collect();
        self.node_mut(key).children = children;
        key
    }

    fn instantiate(&mut self, reader: &mut Reader<'_>) -> Result<()> {
        let id = reader.u32()?;
        let parent = reader.u32()?;
        let before = reader.u32()?;
        let template_id = reader.u32()?;
        let template = self
            .templates
            .get(&template_id)
            .ok_or_else(|| format!("unknown template {template_id}"))?
            .clone();
        let row = self.clone_row(&template.root, None);
        self.register(row, id)?;
        for hole in template.holes {
            let mut key = row;
            for index in hole.path {
                key = *self
                    .node(key)
                    .children
                    .get(index)
                    .ok_or("invalid template hole path")?;
            }
            match &hole.kind {
                HoleKind::Text => self.node_mut(key).text = reader.str()?,
                HoleKind::Attr(name) => self
                    .node_mut(key)
                    .set_attr(name.clone(), reader.presence()?),
                HoleKind::Event(name, listener) => {
                    self.node_mut(key).listeners.insert(name.clone(), *listener);
                }
                HoleKind::Id => self.register(key, reader.u32()?)?,
                HoleKind::Ref => {
                    self.refs.insert(reader.u32()?, key);
                }
                HoleKind::Prop(name) => self
                    .node_mut(key)
                    .set_attr(name.clone(), Some(reader.str()?)),
            }
            self.node_mut(row).holes.push((key, hole.kind));
        }
        self.place(row, parent, before)
    }

    /// Apply view commands, leaving host effects to the caller.
    pub(crate) fn command(&mut self, opcode: u8, reader: &mut Reader<'_>) -> Result<bool> {
        match opcode {
            1 => self.instantiate(reader)?,
            2 | 3 | 22 => {
                let row = self.get(reader.u32()?)?;
                let index = reader.u8()? as usize;
                let (key, kind) = self
                    .node(row)
                    .holes
                    .get(index)
                    .ok_or("unknown row hole")?
                    .clone();
                match (opcode, kind) {
                    (2, HoleKind::Text) => self.node_mut(key).text = reader.str()?,
                    (3, HoleKind::Attr(name)) => {
                        self.node_mut(key).set_attr(name, reader.presence()?)
                    }
                    (22, HoleKind::Prop(name)) => {
                        self.node_mut(key).set_attr(name, Some(reader.str()?))
                    }
                    _ => return Err("command does not match row hole".into()),
                }
            }
            4 => {
                let key = self.get(reader.u32()?)?;
                let before_id = reader.u32()?;
                let before = (before_id != 0).then(|| self.get(before_id)).transpose()?;
                if before != Some(key) {
                    let parent = self.detach(key)?;
                    self.adopt(key, parent, before)?;
                }
            }
            5 => {
                let key = self.get(reader.u32()?)?;
                self.detach(key)?;
                self.forget(key);
            }
            6 => {
                let parent = self.get(reader.u32()?)?;
                reader.u32()?; // Site id.
                self.clear(parent);
            }
            7 => {
                let id = reader.u32()?;
                let len = reader.u32()? as usize;
                self.templates
                    .insert(id, Template::decode(reader.raw(len)?)?);
            }
            8 | 10 | 26 => {
                let id = reader.u32()?;
                let parent = reader.u32()?;
                let before = reader.u32()?;
                let node = match opcode {
                    8 => {
                        let name = reader.name()?;
                        reader.u8()?; // Child container.
                        Node::new(Kind::Element(name), String::new())
                    }
                    10 => Node::new(Kind::Text, reader.str()?),
                    _ => Node::new(Kind::Comment, reader.str()?),
                };
                let key = self.allocate(node);
                self.register(key, id)?;
                self.place(key, parent, before)?;
            }
            9 => {
                let key = self.get(reader.u32()?)?;
                self.node_mut(key)
                    .set_attr(reader.name()?, reader.presence()?);
            }
            11 => {
                let key = self.get(reader.u32()?)?;
                let handler = reader.u32()?;
                let name = reader.str()?;
                let kind = reader.u8()?;
                let policies = reader.u32()?;
                self.node_mut(key).listeners.insert(
                    name,
                    Listener {
                        handler,
                        kind,
                        policies,
                    },
                );
            }
            12 => {
                let key = self.get(reader.u32()?)?;
                let text = reader.str()?;
                if matches!(self.node(key).kind, Kind::Element(_)) {
                    self.clear(key);
                }
                self.node_mut(key).text = text;
            }
            21 => {
                let key = self.get(reader.u32()?)?;
                self.node_mut(key)
                    .set_attr(property(reader.u8()?)?, Some(reader.str()?));
            }
            23 => {
                reader.u32()?; // No out-of-tree document registrations.
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(out: &mut Vec<u8>, words: &[u32]) {
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }

    fn name(out: &mut Vec<u8>, text: &str) {
        out.push(0);
        put_str(out, "");
        put_str(out, text);
    }

    fn apply(tree: &mut Tree, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        while !reader.done() {
            let opcode = reader.u8().unwrap();
            assert!(tree.command(opcode, &mut reader).unwrap());
        }
    }

    #[test]
    fn keeps_input_properties_and_policies_on_elements_and_template_holes() {
        let mut bytes = vec![8];
        ints(&mut bytes, &[1, 0, 0]);
        name(&mut bytes, "ui-asset-image");
        bytes.push(0);
        bytes.push(21);
        ints(&mut bytes, &[1]);
        bytes.push(3);
        put_str(&mut bytes, "source");
        bytes.push(11);
        ints(&mut bytes, &[1, 4]);
        put_str(&mut bytes, "pointerdown");
        bytes.push(3);
        ints(&mut bytes, &[4]);

        let mut template = vec![1];
        name(&mut template, "ui-asset-image");
        template.extend_from_slice(&[0, 0, 0, 2]);
        template.extend_from_slice(&[6, 0, 3]); // Top-level pointer zone property.
        template.extend_from_slice(&[3, 0, 8]); // Top-level pointer listener.
        put_str(&mut template, "pointermove");
        template.push(3);
        ints(&mut template, &[8]); // Latest move.
        bytes.push(7);
        ints(&mut bytes, &[9, template.len() as u32]);
        bytes.extend(template);
        bytes.push(1);
        ints(&mut bytes, &[2, 0, 0, 9]);
        put_str(&mut bytes, "clone");

        let mut tree = Tree::new();
        apply(&mut tree, &bytes);
        let element = tree.get(1).unwrap();
        let clone = tree.get(2).unwrap();
        assert!(tree.node(element).listens("pointerdown"));
        assert_eq!(tree.node(element).attr("pointer-zone"), Some("source"));
        assert_eq!(
            tree.mounted_event(element, "pointerdown")
                .unwrap()
                .1
                .policies,
            4
        );
        assert!(tree.node(clone).listens("pointermove"));
        assert_eq!(tree.node(clone).attr("pointer-zone"), Some("clone"));
        assert_eq!(
            tree.mounted_event(clone, "pointermove").unwrap().1.policies,
            8
        );
    }

    #[test]
    fn cloned_rows_keep_identity_and_route_their_handlers() {
        let mut template = vec![1];
        name(&mut template, "ui-button");
        template.extend_from_slice(&[0, 1]);
        name(&mut template, "enabled");
        put_str(&mut template, "true");
        template.extend_from_slice(&[1, 2]);
        put_str(&mut template, "");
        template.extend_from_slice(&[3, 1, 1, 0, 0, 3, 0, 2]);
        put_str(&mut template, "click");
        template.push(10);
        ints(&mut template, &[0]);
        template.extend_from_slice(&[5, 0]); // Reference to the row root.

        let mut bytes = vec![7];
        ints(&mut bytes, &[100, template.len() as u32]);
        bytes.extend(template);
        for (id, caption) in [(20, "first"), (21, "second")] {
            bytes.push(1);
            ints(&mut bytes, &[id, 0, 0, 100]);
            put_str(&mut bytes, caption);
            ints(&mut bytes, &[id + 100]);
        }
        bytes.push(4);
        ints(&mut bytes, &[21, 20]);
        let mut tree = Tree::new();
        apply(&mut tree, &bytes);

        let first = tree.get(20).unwrap();
        let second = tree.get(21).unwrap();
        assert_eq!(tree.node(ROOT).children, [second, first]);
        assert!(matches!(&tree.node(first).kind, Kind::Element(tag) if tag == "ui-button"));
        assert_eq!(tree.caption(second), "second");
        assert_eq!(tree.node(second).attr("enabled"), Some("true"));
        assert_eq!(tree.reference(121), Some(second));
        assert_eq!(
            tree.event(second, "click"),
            Some((
                21,
                Listener {
                    handler: 2,
                    kind: 10,
                    policies: 0
                }
            ))
        );

        let mut update = vec![2];
        ints(&mut update, &[21]);
        update.push(0);
        put_str(&mut update, "updated");
        update.push(5);
        ints(&mut update, &[20]);
        apply(&mut tree, &update);
        assert_eq!(tree.caption(second), "updated");
        assert_eq!(tree.node(ROOT).children, [second]);
        assert!(!tree.contains(first));
    }

    #[test]
    fn report_reads_cells_in_column_order_and_validates_schema() {
        fn element(bytes: &mut Vec<u8>, id: u32, parent: u32, tag: &str, key: &str) {
            bytes.push(8);
            ints(bytes, &[id, parent, 0]);
            name(bytes, tag);
            bytes.push(1);
            bytes.push(9);
            ints(bytes, &[id]);
            name(bytes, "list-key");
            bytes.push(1);
            put_str(bytes, key);
        }

        fn caption(bytes: &mut Vec<u8>, id: u32, parent: u32, text: &str) {
            bytes.push(10);
            ints(bytes, &[id, parent, 0]);
            put_str(bytes, text);
        }

        let mut tree = Tree::new();
        let mut bytes = Vec::new();
        element(&mut bytes, 1, 0, "ui-list-view", "");
        element(&mut bytes, 2, 1, "ui-list-column", "title");
        caption(&mut bytes, 3, 2, "Title");
        element(&mut bytes, 4, 1, "ui-list-column", "status");
        element(&mut bytes, 5, 1, "ui-list-row", "a");
        element(&mut bytes, 6, 5, "ui-list-cell", "status");
        caption(&mut bytes, 7, 6, "Unread");
        apply(&mut tree, &bytes);
        let root = tree.get(1).unwrap();
        let read = crate::list_view::Report::read;
        assert!(read(&tree, root).unwrap_err().contains("missing column"));

        bytes.clear();
        element(&mut bytes, 8, 5, "ui-list-cell", "title");
        caption(&mut bytes, 9, 8, "Café");
        apply(&mut tree, &bytes);
        let report = read(&tree, root).unwrap();
        assert_eq!(report.rows[0].cells, ["Café", "Unread"]);
        assert_eq!(report.columns[0].heading, "Title");

        bytes.clear();
        element(&mut bytes, 10, 1, "ui-list-row", "a");
        apply(&mut tree, &bytes);
        assert!(
            read(&tree, root)
                .unwrap_err()
                .contains("duplicate report row")
        );
    }

    #[test]
    fn malformed_wire_data_is_an_error() {
        let mut reader = Reader::new(&[255, 255, 255, 255, 0]);
        assert_eq!(reader.str().unwrap_err(), "truncated wire data");
        let mut reader = Reader::new(&[1, 0, 0, 0, 255]);
        assert_eq!(reader.str().unwrap_err(), "invalid UTF-8 in wire data");
    }

    #[test]
    fn boot_read_copies_declared_storage_and_frames_sections() {
        let source = Declarations {
            local_keys: vec!["key".into()],
            startup: vec![0],
            ..Declarations::default()
        };
        let bytes = source
            .boot(&Snapshot::simulated(123, 3), |session, key| {
                assert!(!session);
                assert_eq!(key, "key");
                Some("value".into())
            })
            .unwrap();
        let mut frame = Reader::new(&bytes);
        let mut tags = Vec::new();
        while !frame.done() {
            let tag = frame.u8().unwrap();
            tags.push(tag);
            let len = frame.u32().unwrap() as usize;
            let section = frame.raw(len).unwrap();
            if tag == 2 {
                let mut values = Reader::new(section);
                assert_eq!(values.u8().unwrap(), 1);
                assert_eq!(values.u32().unwrap(), 1);
                assert_eq!(values.str().unwrap(), "key");
                assert_eq!(values.str().unwrap(), "value");
            }
        }
        assert_eq!(tags, [1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 13, 14]);

        let mut keys = Vec::new();
        for (tag, body) in [
            (2_u8, vec![0; 4]),
            (5, vec![0; 4]),
            (6, vec![0; 4]),
            (10, vec![3, 0]),
            (11, vec![0; 4]),
            (14, vec![0]),
        ] {
            keys.push(tag);
            ints(&mut keys, &[body.len() as u32]);
            keys.extend(body);
        }
        assert_eq!(Declarations::decode(&keys).unwrap().zone_days, 3);
    }
}
