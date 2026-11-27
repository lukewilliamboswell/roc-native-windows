//! UI Automation projection of the grid's controlled selection, values and mounts.

#![allow(non_upper_case_globals)] // UI Automation identifiers follow the Windows SDK spelling.

use crate::accessibility::{Mount, boolean, integer, string, text_changed, unavailable};
use crate::accessibility_abi::{FragmentAbi, FragmentRootAbi, HwndOverrideAbi, SimpleAbi};
use crate::wire::Key;
use std::collections::{BTreeMap, VecDeque};
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, HWND};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_I4, VT_UNKNOWN};
use windows::Win32::UI::Accessibility::*;
use windows::core::{BOOL, BSTR, Error, HRESULT, IUnknown, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::HWND as RawHwnd;

const MAX_ACTIONS: usize = 64;
const MAX_DRAFT: usize = 4096;

#[derive(Clone)]
pub(crate) struct Cell {
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) value: String,
    pub(crate) feedback: String,
    pub(crate) read_only: bool,
    pub(crate) bounds: UiaRect,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) key: Key,
    pub(crate) cells: Vec<Key>,
}

#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) name: String,
    pub(crate) headers: Vec<(String, UiaRect)>,
    pub(crate) rows: Vec<Row>,
    pub(crate) cells: BTreeMap<Key, Cell>,
    pub(crate) bounds: UiaRect,
    pub(crate) active: Option<Key>,
    pub(crate) range: Option<Range>,
    pub(crate) focused: bool,
    pub(crate) enabled: bool,
    pub(crate) editor: Option<Editor>,
    pub(crate) scroll: [ScrollAxis; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Editor {
    pub(crate) cell: Key,
    pub(crate) hwnd: usize,
    pub(crate) generation: u64,
    pub(crate) focused: bool,
}

impl Editor {
    fn item(self) -> Item {
        Item::Editor {
            cell: self.cell,
            generation: self.generation,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ScrollAxis {
    pub(crate) position: i32,
    pub(crate) length: i32,
    pub(crate) page: i32,
}

impl ScrollAxis {
    fn travel(self) -> i32 {
        (self.length - self.page).max(0)
    }

    fn percent(self) -> f64 {
        if self.travel() == 0 {
            UIA_ScrollPatternNoScroll
        } else {
            100.0 * f64::from(self.position) / f64::from(self.travel())
        }
    }

    fn view_size(self) -> f64 {
        if self.travel() == 0 {
            100.0
        } else {
            100.0 * f64::from(self.page) / f64::from(self.length)
        }
    }
}

impl Snapshot {
    fn contains(&self, node: Item) -> bool {
        match node {
            Item::Grid => true,
            Item::Header(index) => self.headers.get(index).is_some(),
            Item::Row(key) => self.rows.iter().any(|row| row.key == key),
            Item::Cell(key) => self.cells.contains_key(&key),
            Item::Editor { .. } => self.editor.is_some_and(|editor| editor.item() == node),
            Item::Feedback(key) => self
                .cells
                .get(&key)
                .is_some_and(|cell| !cell.feedback.is_empty()),
        }
    }

    fn focus(&self) -> Option<Item> {
        if !self.enabled {
            return None;
        }
        if let Some(editor) = self.editor.filter(|editor| editor.focused) {
            return Some(editor.item());
        }
        self.focused
            .then(|| self.active.map_or(Item::Grid, Item::Cell))
    }

    fn selected(&self, cell: &Cell) -> bool {
        self.range
            .is_some_and(|range| contains_cell(range, cell.row, cell.column))
    }

    fn grid_child(&self, index: usize) -> Option<Item> {
        if index < self.headers.len() {
            Some(Item::Header(index))
        } else {
            self.rows
                .get(index - self.headers.len())
                .map(|row| Item::Row(row.key))
        }
    }

    fn grid_child_index(&self, node: Item) -> Option<usize> {
        match node {
            Item::Header(index) => Some(index),
            Item::Row(key) => self
                .rows
                .iter()
                .position(|row| row.key == key)
                .map(|index| self.headers.len() + index),
            _ => None,
        }
    }

    fn row(&self, key: Key) -> &Row {
        self.rows.iter().find(|row| row.key == key).unwrap()
    }

    fn rectangle(&self, node: Item) -> UiaRect {
        match node {
            Item::Grid => self.bounds,
            Item::Header(index) => self.headers[index].1,
            Item::Row(key) => self
                .row(key)
                .cells
                .iter()
                .map(|key| self.cells[key].bounds)
                .filter(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
                .reduce(|left, right| {
                    let x = left.left.min(right.left);
                    let y = left.top.min(right.top);
                    UiaRect {
                        left: x,
                        top: y,
                        width: (left.left + left.width).max(right.left + right.width) - x,
                        height: (left.top + left.height).max(right.top + right.height) - y,
                    }
                })
                .unwrap_or_default(),
            Item::Cell(key) => self.cells[&key].bounds,
            Item::Feedback(_) => UiaRect::default(),
            Item::Editor { cell, .. } => self.cells[&cell].bounds,
        }
    }

    fn navigation(&self, from: Item, direction: NavigateDirection) -> Option<Item> {
        match (from, direction) {
            (Item::Grid, NavigateDirection_FirstChild) => self.grid_child(0),
            (Item::Grid, NavigateDirection_LastChild) => {
                self.grid_child((self.headers.len() + self.rows.len()).checked_sub(1)?)
            }
            (Item::Grid, _) => None,
            (Item::Editor { cell, .. }, NavigateDirection_Parent)
            | (Item::Feedback(cell), NavigateDirection_Parent) => Some(Item::Cell(cell)),
            (Item::Editor { cell, .. }, NavigateDirection_NextSibling) => self.feedback(cell),
            (Item::Feedback(cell), NavigateDirection_PreviousSibling) => self.editor_item(cell),
            (Item::Editor { .. } | Item::Feedback(_), _) => None,
            (Item::Cell(key), NavigateDirection_Parent) => {
                Some(Item::Row(self.rows[self.cells[&key].row].key))
            }
            (_, NavigateDirection_Parent) => Some(Item::Grid),
            (Item::Row(key), NavigateDirection_FirstChild) => {
                self.row(key).cells.first().copied().map(Item::Cell)
            }
            (Item::Row(key), NavigateDirection_LastChild) => {
                self.row(key).cells.last().copied().map(Item::Cell)
            }
            (Item::Cell(key), NavigateDirection_FirstChild) => {
                self.editor_item(key).or_else(|| self.feedback(key))
            }
            (Item::Cell(key), NavigateDirection_LastChild) => {
                self.feedback(key).or_else(|| self.editor_item(key))
            }
            (Item::Cell(key), NavigateDirection_NextSibling) => {
                let cell = &self.cells[&key];
                self.rows[cell.row]
                    .cells
                    .get(cell.column + 1)
                    .copied()
                    .map(Item::Cell)
            }
            (Item::Cell(key), NavigateDirection_PreviousSibling) => {
                let cell = &self.cells[&key];
                self.rows[cell.row]
                    .cells
                    .get(cell.column.checked_sub(1)?)
                    .copied()
                    .map(Item::Cell)
            }
            (_, NavigateDirection_NextSibling) => self.grid_child(self.grid_child_index(from)? + 1),
            (_, NavigateDirection_PreviousSibling) => {
                self.grid_child(self.grid_child_index(from)?.checked_sub(1)?)
            }
            _ => None,
        }
    }

    fn editor_item(&self, cell: Key) -> Option<Item> {
        self.editor
            .filter(|editor| editor.cell == cell)
            .map(Editor::item)
    }

    fn feedback(&self, cell: Key) -> Option<Item> {
        self.contains(Item::Feedback(cell))
            .then_some(Item::Feedback(cell))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Grid,
    Header(usize),
    Row(Key),
    Cell(Key),
    Editor { cell: Key, generation: u64 },
    Feedback(Key),
}

pub(crate) enum Operation {
    SetValue(String),
    Select,
    ChangeSelection(SelectionChange),
    Reveal,
    Focus,
    FocusEditor(u64),
    Scroll([i32; 2]),
}

pub(crate) type Range = (usize, usize, usize, usize);

#[derive(Clone, Copy)]
pub(crate) enum SelectionChange {
    Add,
    Remove,
}

impl SelectionChange {
    // The grid stores a nonempty rectangle. A single added or removed item
    // may only extend or shrink a contiguous row or column.
    pub(crate) fn updated(
        self,
        range: Option<Range>,
        row: usize,
        column: usize,
    ) -> windows::core::Result<Option<Range>> {
        let Some((top, left, bottom, right)) = range else {
            return Ok(match self {
                Self::Add => Some((row, column, row, column)),
                Self::Remove => None,
            });
        };
        let selected = contains_cell((top, left, bottom, right), row, column);
        let result = match self {
            Self::Add if selected => None,
            Self::Remove if !selected => None,
            Self::Add if top == bottom && row == top && column.checked_add(1) == Some(left) => {
                Some((top, column, bottom, right))
            }
            Self::Add if top == bottom && row == top && right.checked_add(1) == Some(column) => {
                Some((top, left, bottom, column))
            }
            Self::Add if left == right && column == left && row.checked_add(1) == Some(top) => {
                Some((row, left, bottom, right))
            }
            Self::Add if left == right && column == left && bottom.checked_add(1) == Some(row) => {
                Some((top, left, row, right))
            }
            Self::Remove if top == bottom && left < right && column == left => {
                Some((top, left + 1, bottom, right))
            }
            Self::Remove if top == bottom && left < right && column == right => {
                Some((top, left, bottom, right - 1))
            }
            Self::Remove if left == right && top < bottom && row == top => {
                Some((top + 1, left, bottom, right))
            }
            Self::Remove if left == right && top < bottom && row == bottom => {
                Some((top, left, bottom - 1, right))
            }
            _ => return Err(unsupported()),
        };
        Ok(result)
    }
}

fn contains_cell((top, left, bottom, right): Range, row: usize, column: usize) -> bool {
    (top..=bottom).contains(&row) && (left..=right).contains(&column)
}

pub(crate) struct Action {
    pub(crate) cell: Key,
    pub(crate) operation: Operation,
}

#[derive(Default)]
struct State {
    snapshot: Option<Arc<Snapshot>>,
    actions: VecDeque<Action>,
}

impl State {
    fn snapshot(&self, item: Item) -> windows::core::Result<&Snapshot> {
        self.snapshot
            .as_deref()
            .filter(|s| s.contains(item))
            .ok_or_else(unavailable)
    }

    fn action(&mut self, item: Item, operation: Operation) -> windows::core::Result<()> {
        let snapshot = self.snapshot(item)?;
        let key = match item {
            Item::Grid if matches!(operation, Operation::Focus | Operation::Scroll(_)) => {
                snapshot.active.ok_or_else(unsupported)?
            }
            Item::Cell(key) | Item::Editor { cell: key, .. } => key,
            _ => return Err(unsupported()),
        };
        let cell = snapshot.cells.get(&key).ok_or_else(unavailable)?;
        if !snapshot.enabled {
            return Err(Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTENABLED as i32)));
        }
        if matches!(operation, Operation::SetValue(_))
            && (cell.read_only || snapshot.editor.is_some())
        {
            return Err(unsupported());
        }
        if let Operation::ChangeSelection(change) = operation
            && change
                .updated(snapshot.range, cell.row, cell.column)?
                .is_none()
        {
            return Ok(());
        }
        if self.actions.len() >= MAX_ACTIONS {
            return Err(Error::from_hresult(HRESULT(UIA_E_TIMEOUT as i32)));
        }
        self.actions.push_back(Action {
            cell: key,
            operation,
        });
        Ok(())
    }
}

pub(crate) struct Projection {
    mount: Mount,
    state: Arc<Mutex<State>>,
    reported: Option<Arc<Snapshot>>,
    hwnd: usize,
}

impl Projection {
    pub(crate) fn new(hwnd: RawHwnd) -> crate::Result<Self> {
        let state = Arc::new(Mutex::new(State::default()));
        let provider = Provider::element(hwnd as usize, state.clone(), Item::Grid);
        Ok(Self {
            mount: Mount::new(hwnd, provider)?,
            state,
            reported: None,
            hwnd: hwnd as usize,
        })
    }

    pub(crate) fn update(&self, snapshot: Snapshot) {
        self.state.lock().unwrap().snapshot = Some(Arc::new(snapshot));
    }

    pub(crate) fn set_interaction(&self, enabled: bool, focused: bool, editor_focus: bool) {
        let mut state = self.state.lock().unwrap();
        if !enabled {
            state.actions.clear();
        }
        if let Some(snapshot) = &mut state.snapshot
            && (snapshot.enabled != enabled
                || snapshot.focused != focused
                || snapshot
                    .editor
                    .is_some_and(|editor| editor.focused != editor_focus))
        {
            let snapshot = Arc::make_mut(snapshot);
            snapshot.enabled = enabled;
            snapshot.focused = focused;
            if let Some(editor) = &mut snapshot.editor {
                editor.focused = editor_focus;
            }
        }
    }

    pub(crate) fn mount_editor(&self, hwnd: RawHwnd, editor: Editor) -> crate::Result<Mount> {
        Mount::new(
            hwnd,
            Provider::element(self.hwnd, self.state.clone(), editor.item()),
        )
    }

    pub(crate) fn retire_editor(&self) {
        if let Some(snapshot) = &mut self.state.lock().unwrap().snapshot {
            Arc::make_mut(snapshot).editor = None;
        }
    }

    pub(crate) fn drain(&self) -> VecDeque<Action> {
        std::mem::take(&mut self.state.lock().unwrap().actions)
    }

    pub(crate) fn announce(&mut self) {
        let current = self.state.lock().unwrap().snapshot.clone();
        if let (Some(current), Some(last)) = (&current, &self.reported)
            && Arc::ptr_eq(current, last)
        {
            return;
        }
        let prior = std::mem::replace(&mut self.reported, current);
        if !self.mount.events_allowed() {
            return;
        }
        let Some(prior) = prior else { return };
        let current = self.reported.as_ref().unwrap();
        if prior.rows != current.rows
            || prior.headers.len() != current.headers.len()
            || prior.editor.map(Editor::item) != current.editor.map(Editor::item)
        {
            // SAFETY: The retained fragment provider is live and null runtime ID names its root.
            unsafe {
                let _ = UiaRaiseStructureChangedEvent(
                    self.mount.provider(),
                    StructureChangeType_ChildrenInvalidated,
                    null_mut(),
                    0,
                );
            }
        }
        if let Some(item) = current.focus()
            && Some(item) != prior.focus()
            && !matches!(item, Item::Editor { .. })
        {
            // Windows raises focus for the native EDIT. The grid supplies its
            // own event when focus returns to an internal cell.
            let provider = Provider::element(self.hwnd, self.state.clone(), item);
            // SAFETY: The focused item was read from the committed projection.
            unsafe {
                let _ = UiaRaiseAutomationEvent(&provider, UIA_AutomationFocusChangedEventId);
            }
        }
        if prior.range != current.range {
            // SAFETY: The selection changed in this provider's committed projection.
            unsafe {
                let _ = UiaRaiseAutomationEvent(
                    self.mount.provider(),
                    UIA_Selection_InvalidatedEventId,
                );
            }
        }
        for (&key, cell) in &current.cells {
            if let Some(old) = prior.cells.get(&key) {
                if old.value != cell.value {
                    let item = Provider::element(self.hwnd, self.state.clone(), Item::Cell(key));
                    text_changed(&item, UIA_ValueValuePropertyId, &old.value, &cell.value);
                    if cell.column == 0 {
                        let row = current.rows[cell.row].key;
                        let item = Provider::element(self.hwnd, self.state.clone(), Item::Row(row));
                        text_changed(&item, UIA_NamePropertyId, &old.value, &cell.value);
                    }
                }
                if old.feedback != cell.feedback {
                    let item = Provider::element(self.hwnd, self.state.clone(), Item::Cell(key));
                    text_changed(&item, UIA_HelpTextPropertyId, &old.feedback, &cell.feedback);
                }
            }
        }
    }
}

impl Drop for Projection {
    fn drop(&mut self) {
        *self.state.lock().unwrap() = State::default();
    }
}

#[implement(
    SimpleAbi,
    FragmentAbi,
    FragmentRootAbi,
    HwndOverrideAbi,
    IGridProvider,
    IGridItemProvider,
    ITableProvider,
    ITableItemProvider,
    ISelectionProvider,
    ISelectionItemProvider,
    IScrollItemProvider,
    IValueProvider,
    IScrollProvider
)]
struct Provider {
    hwnd: usize,
    state: Arc<Mutex<State>>,
    item: Item,
}

impl Provider {
    fn element(hwnd: usize, state: Arc<Mutex<State>>, item: Item) -> IRawElementProviderSimple {
        let provider: SimpleAbi = if matches!(item, Item::Editor { .. }) {
            EditorProvider { hwnd, state, item }.into()
        } else {
            Self { hwnd, state, item }.into()
        };
        provider.into_provider()
    }
}

impl Provider_Impl {
    fn with<T>(
        &self,
        read: impl FnOnce(&Snapshot) -> windows::core::Result<T>,
    ) -> windows::core::Result<T> {
        let state = self.state.lock().unwrap();
        read(state.snapshot(self.item)?)
    }

    fn other(&self, item: Item) -> IRawElementProviderSimple {
        Provider::element(self.hwnd, self.state.clone(), item)
    }

    fn cell(&self) -> windows::core::Result<Key> {
        match self.item {
            Item::Cell(key) => Ok(key),
            _ => Err(unsupported()),
        }
    }

    fn action(&self, operation: Operation) -> windows::core::Result<()> {
        self.state.lock().unwrap().action(self.item, operation)
    }
}

fn unsupported() -> Error {
    Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32))
}

fn absent() -> Error {
    Error::from_hresult(E_NOINTERFACE)
}

impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        self.with(|s| {
            let available = match self.item {
                Item::Grid => {
                    matches!(
                        id,
                        UIA_GridPatternId | UIA_TablePatternId | UIA_SelectionPatternId
                    ) || (id == UIA_ScrollPatternId
                        && s.scroll.iter().any(|axis| axis.travel() > 0))
                }
                Item::Cell(_) => matches!(
                    id,
                    UIA_GridItemPatternId
                        | UIA_TableItemPatternId
                        | UIA_ValuePatternId
                        | UIA_SelectionItemPatternId
                        | UIA_ScrollItemPatternId
                ),
                _ => false,
            };
            if available {
                self.other(self.item).cast()
            } else {
                Err(absent())
            }
        })
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        self.with(|s| {
            let cell = match self.item {
                Item::Cell(key) => s.cells.get(&key),
                _ => None,
            };
            Ok(match id {
                UIA_ControlTypePropertyId => integer(
                    match self.item {
                        Item::Grid => UIA_DataGridControlTypeId,
                        Item::Header(_) => UIA_HeaderItemControlTypeId,
                        Item::Row(_) | Item::Cell(_) => UIA_DataItemControlTypeId,
                        Item::Feedback(_) => UIA_TextControlTypeId,
                        Item::Editor { .. } => unreachable!("editor has its own provider"),
                    }
                    .0,
                ),
                UIA_NamePropertyId => string(match self.item {
                    Item::Grid => &s.name,
                    Item::Header(index) => &s.headers[index].0,
                    Item::Row(key) => &s.cells[&s.row(key).cells[0]].value,
                    Item::Cell(key) => &s.headers[s.cells[&key].column].0,
                    Item::Feedback(key) => &s.cells[&key].feedback,
                    Item::Editor { .. } => unreachable!("editor has its own provider"),
                }),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => boolean(true),
                UIA_IsEnabledPropertyId => boolean(s.enabled),
                UIA_IsOffscreenPropertyId => boolean(s.rectangle(self.item).width == 0.0),
                UIA_IsKeyboardFocusablePropertyId => {
                    boolean(s.enabled && matches!(self.item, Item::Grid | Item::Cell(_)))
                }
                UIA_HasKeyboardFocusPropertyId => boolean(s.focus() == Some(self.item)),
                UIA_HelpTextPropertyId if cell.is_some() => string(&cell.unwrap().feedback),
                UIA_DescribedByPropertyId if let Item::Cell(key) = self.item => {
                    description(s, self.hwnd, &self.state, key)?
                }
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|_| Ok(()))?;
        if self.item != Item::Grid {
            return Err(absent());
        }
        // SAFETY: This is the fragment root's HWND. UIA will reject it if retired.
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
    }
}

impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(
        &self,
        direction: NavigateDirection,
    ) -> windows::core::Result<IRawElementProviderFragment> {
        self.with(|s| {
            self.other(s.navigation(self.item, direction).ok_or_else(absent)?)
                .cast()
        })
    }

    fn GetRuntimeId(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|_| {
            let (kind, key) = match self.item {
                Item::Grid => return Ok(null_mut()),
                Item::Editor { .. } => unreachable!("editor has its own provider"),
                Item::Header(index) => (1, index),
                Item::Cell(key) => (2, key),
                Item::Feedback(key) => (3, key),
                Item::Row(key) => (4, key),
            };
            let key = key as u64;
            let parts = [
                UiaAppendRuntimeId as i32,
                kind,
                key as i32,
                (key >> 32) as i32,
            ];
            array(VT_I4, parts.len(), |at| {
                std::ptr::from_ref(&parts[at]).cast()
            })
        })
    }

    fn BoundingRectangle(&self) -> windows::core::Result<UiaRect> {
        self.with(|s| Ok(s.rectangle(self.item)))
    }
    fn GetEmbeddedFragmentRoots(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|_| Ok(null_mut()))
    }
    fn SetFocus(&self) -> windows::core::Result<()> {
        self.action(Operation::Focus)
    }
    fn FragmentRoot(&self) -> windows::core::Result<IRawElementProviderFragmentRoot> {
        self.with(|_| self.other(Item::Grid).cast())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(
        &self,
        x: f64,
        y: f64,
    ) -> windows::core::Result<IRawElementProviderFragment> {
        self.with(|s| {
            let hit = |r: UiaRect| {
                x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height
            };
            let item = s
                .cells
                .iter()
                .find_map(|(&key, cell)| {
                    hit(cell.bounds).then(|| s.editor_item(key).unwrap_or(Item::Cell(key)))
                })
                .or_else(|| {
                    s.headers
                        .iter()
                        .position(|(_, bounds)| hit(*bounds))
                        .map(Item::Header)
                })
                .unwrap_or(Item::Grid);
            self.other(item).cast()
        })
    }

    fn GetFocus(&self) -> windows::core::Result<IRawElementProviderFragment> {
        self.with(|s| self.other(s.focus().ok_or_else(absent)?).cast())
    }
}

// Keep the native EDIT's patterns, but attach its host to the cell fragment.
#[implement(SimpleAbi, FragmentAbi)]
struct EditorProvider {
    hwnd: usize,
    state: Arc<Mutex<State>>,
    item: Item,
}

impl EditorProvider_Impl {
    fn with<T>(
        &self,
        read: impl FnOnce(&Snapshot, Editor) -> windows::core::Result<T>,
    ) -> windows::core::Result<T> {
        let state = self.state.lock().unwrap();
        let snapshot = state.snapshot(self.item)?;
        read(snapshot, snapshot.editor.ok_or_else(unavailable)?)
    }

    fn other(&self, item: Item) -> IRawElementProviderSimple {
        Provider::element(self.hwnd, self.state.clone(), item)
    }
}

impl IRawElementProviderSimple_Impl for EditorProvider_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_OverrideProvider)
    }

    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        self.with(|_, _| Err(absent()))
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        self.with(|s, editor| {
            let cell = &s.cells[&editor.cell];
            Ok(match id {
                UIA_NamePropertyId => string(&s.headers[cell.column].0),
                UIA_HelpTextPropertyId => string(&cell.feedback),
                UIA_DescribedByPropertyId => description(s, self.hwnd, &self.state, editor.cell)?,
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        let hwnd = self.with(|_, editor| Ok(editor.hwnd))?;
        // SAFETY: This HWND is the currently mounted editable child.
        unsafe { UiaHostProviderFromHwnd(HWND(hwnd as _)) }
    }
}

impl IRawElementProviderFragment_Impl for EditorProvider_Impl {
    fn Navigate(
        &self,
        direction: NavigateDirection,
    ) -> windows::core::Result<IRawElementProviderFragment> {
        self.with(|s, _| {
            self.other(s.navigation(self.item, direction).ok_or_else(absent)?)
                .cast()
        })
    }

    fn GetRuntimeId(&self) -> windows::core::Result<*mut SAFEARRAY> {
        // UIA uses the native EDIT HWND's runtime identity.
        self.with(|_, _| Ok(null_mut()))
    }

    fn BoundingRectangle(&self) -> windows::core::Result<UiaRect> {
        self.with(|s, _| Ok(s.rectangle(self.item)))
    }

    fn GetEmbeddedFragmentRoots(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|_, _| Ok(null_mut()))
    }

    fn SetFocus(&self) -> windows::core::Result<()> {
        let Item::Editor { generation, .. } = self.item else {
            return Err(unsupported());
        };
        self.state
            .lock()
            .unwrap()
            .action(self.item, Operation::FocusEditor(generation))
    }

    fn FragmentRoot(&self) -> windows::core::Result<IRawElementProviderFragmentRoot> {
        self.with(|_, _| self.other(Item::Grid).cast())
    }
}

fn description(
    snapshot: &Snapshot,
    hwnd: usize,
    state: &Arc<Mutex<State>>,
    key: Key,
) -> windows::core::Result<VARIANT> {
    let item = Item::Feedback(key);
    if snapshot.contains(item) {
        let provider = Provider::element(hwnd, state.clone(), item).cast()?;
        Ok(provider_list(array_for([provider])?))
    } else {
        Ok(VARIANT::default())
    }
}

impl IRawElementProviderHwndOverride_Impl for Provider_Impl {
    fn GetOverrideProviderForHwnd(
        &self,
        hwnd: HWND,
    ) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|s| {
            let editor = s
                .editor
                .filter(|editor| editor.hwnd == hwnd.0 as usize)
                .ok_or_else(absent)?;
            Ok(self.other(editor.item()))
        })
    }
}

impl IGridProvider_Impl for Provider_Impl {
    fn GetItem(&self, row: i32, column: i32) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|s| {
            let col = usize::try_from(column)
                .ok()
                .filter(|&i| i < s.headers.len())
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            let key = usize::try_from(row)
                .ok()
                .and_then(|index| s.rows.get(index))
                .and_then(|row| row.cells.get(col))
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            Ok(self.other(Item::Cell(*key)))
        })
    }
    fn RowCount(&self) -> windows::core::Result<i32> {
        self.with(|s| Ok(s.rows.len() as i32))
    }
    fn ColumnCount(&self) -> windows::core::Result<i32> {
        self.with(|s| Ok(s.headers.len() as i32))
    }
}

impl IGridItemProvider_Impl for Provider_Impl {
    fn Row(&self) -> windows::core::Result<i32> {
        self.with(|s| Ok(s.cells[&self.cell()?].row as i32))
    }
    fn Column(&self) -> windows::core::Result<i32> {
        self.with(|s| Ok(s.cells[&self.cell()?].column as i32))
    }
    fn RowSpan(&self) -> windows::core::Result<i32> {
        self.with(|_| Ok(1))
    }
    fn ColumnSpan(&self) -> windows::core::Result<i32> {
        self.with(|_| Ok(1))
    }
    fn ContainingGrid(&self) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|_| Ok(self.other(Item::Grid)))
    }
}

impl ITableProvider_Impl for Provider_Impl {
    fn GetRowHeaders(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|_| Ok(null_mut()))
    }
    fn GetColumnHeaders(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|s| array_for((0..s.headers.len()).map(|at| self.other(Item::Header(at)).into())))
    }
    fn RowOrColumnMajor(&self) -> windows::core::Result<RowOrColumnMajor> {
        self.with(|_| Ok(RowOrColumnMajor_RowMajor))
    }
}

impl ITableItemProvider_Impl for Provider_Impl {
    fn GetRowHeaderItems(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|_| Ok(null_mut()))
    }
    fn GetColumnHeaderItems(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|s| {
            array_for([self
                .other(Item::Header(s.cells[&self.cell()?].column))
                .into()])
        })
    }
}

impl ISelectionProvider_Impl for Provider_Impl {
    fn GetSelection(&self) -> windows::core::Result<*mut SAFEARRAY> {
        self.with(|s| {
            array_for(
                s.rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .filter(|key| s.selected(&s.cells[key]))
                    .map(|&key| self.other(Item::Cell(key)).into()),
            )
        })
    }
    fn CanSelectMultiple(&self) -> windows::core::Result<BOOL> {
        self.with(|_| Ok(true.into()))
    }
    fn IsSelectionRequired(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok((!s.rows.is_empty()).into()))
    }
}

impl ISelectionItemProvider_Impl for Provider_Impl {
    fn Select(&self) -> windows::core::Result<()> {
        self.action(Operation::Select)
    }
    fn AddToSelection(&self) -> windows::core::Result<()> {
        self.action(Operation::ChangeSelection(SelectionChange::Add))
    }
    fn RemoveFromSelection(&self) -> windows::core::Result<()> {
        self.action(Operation::ChangeSelection(SelectionChange::Remove))
    }
    fn IsSelected(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok(s.selected(&s.cells[&self.cell()?]).into()))
    }
    fn SelectionContainer(&self) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|_| Ok(self.other(Item::Grid)))
    }
}

impl IScrollItemProvider_Impl for Provider_Impl {
    fn ScrollIntoView(&self) -> windows::core::Result<()> {
        self.action(Operation::Reveal)
    }
}

impl IScrollProvider_Impl for Provider_Impl {
    fn Scroll(
        &self,
        horizontal: ScrollAmount,
        vertical: ScrollAmount,
    ) -> windows::core::Result<()> {
        let target = self.with(|s| {
            let mut target = [0; 2];
            for (index, amount) in [horizontal, vertical].into_iter().enumerate() {
                let axis = s.scroll[index];
                let delta = match amount {
                    ScrollAmount_NoAmount => 0,
                    ScrollAmount_SmallDecrement => -1,
                    ScrollAmount_SmallIncrement => 1,
                    ScrollAmount_LargeDecrement => -axis.page,
                    ScrollAmount_LargeIncrement => axis.page,
                    _ => return Err(Error::from_hresult(E_INVALIDARG)),
                };
                if delta != 0 && axis.travel() == 0 {
                    return Err(unsupported());
                }
                target[index] = (axis.position + delta).clamp(0, axis.travel());
            }
            Ok(target)
        })?;
        self.action(Operation::Scroll(target))
    }

    fn SetScrollPercent(&self, horizontal: f64, vertical: f64) -> windows::core::Result<()> {
        let target = self.with(|s| {
            let mut target = [0; 2];
            for (index, percent) in [horizontal, vertical].into_iter().enumerate() {
                let axis = s.scroll[index];
                target[index] = if percent == UIA_ScrollPatternNoScroll {
                    axis.position
                } else if (0.0..=100.0).contains(&percent) {
                    if axis.travel() == 0 {
                        return Err(unsupported());
                    }
                    (percent * f64::from(axis.travel()) / 100.0).round() as i32
                } else {
                    return Err(Error::from_hresult(E_INVALIDARG));
                };
            }
            Ok(target)
        })?;
        self.action(Operation::Scroll(target))
    }

    fn HorizontalScrollPercent(&self) -> windows::core::Result<f64> {
        self.with(|s| Ok(s.scroll[0].percent()))
    }
    fn VerticalScrollPercent(&self) -> windows::core::Result<f64> {
        self.with(|s| Ok(s.scroll[1].percent()))
    }
    fn HorizontalViewSize(&self) -> windows::core::Result<f64> {
        self.with(|s| Ok(s.scroll[0].view_size()))
    }
    fn VerticalViewSize(&self) -> windows::core::Result<f64> {
        self.with(|s| Ok(s.scroll[1].view_size()))
    }
    fn HorizontallyScrollable(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok((s.scroll[0].travel() > 0).into()))
    }
    fn VerticallyScrollable(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok((s.scroll[1].travel() > 0).into()))
    }
}

impl IValueProvider_Impl for Provider_Impl {
    fn SetValue(&self, value: &PCWSTR) -> windows::core::Result<()> {
        if value.is_null() {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        // SAFETY: COM supplies a terminated UTF-16 string. Scan only the allowed draft length.
        let units = unsafe {
            let len = (0..=MAX_DRAFT)
                .find(|&at| *value.0.add(at) == 0)
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            std::slice::from_raw_parts(value.0, len)
        };
        self.action(Operation::SetValue(String::from_utf16_lossy(units)))
    }
    fn Value(&self) -> windows::core::Result<BSTR> {
        self.with(|s| Ok(BSTR::from(s.cells[&self.cell()?].value.as_str())))
    }
    fn IsReadOnly(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok((!s.enabled || s.cells[&self.cell()?].read_only).into()))
    }
}

fn array_for(values: impl IntoIterator<Item = IUnknown>) -> windows::core::Result<*mut SAFEARRAY> {
    let values = values.into_iter().collect::<Vec<_>>();
    array(VT_UNKNOWN, values.len(), |at| {
        values[at].as_raw().cast_const()
    })
}

fn array(
    ty: VARENUM,
    len: usize,
    value: impl Fn(usize) -> *const std::ffi::c_void,
) -> windows::core::Result<*mut SAFEARRAY> {
    if len == 0 {
        return Ok(null_mut());
    }
    // SAFETY: Each element pointer matches the SAFEARRAY's type for the synchronous call.
    unsafe {
        let array = SafeArrayCreateVector(ty, 0, len as u32);
        if array.is_null() {
            return Err(Error::from_hresult(E_OUTOFMEMORY));
        }
        for index in 0..len {
            if let Err(error) = SafeArrayPutElement(array, &(index as i32), value(index)) {
                let _ = SafeArrayDestroy(array);
                return Err(error);
            }
        }
        Ok(array)
    }
}

fn provider_list(value: *mut SAFEARRAY) -> VARIANT {
    crate::accessibility::variant(
        VARENUM(VT_UNKNOWN.0 | windows::Win32::System::Variant::VT_ARRAY.0),
        windows::Win32::System::Variant::VARIANT_0_0_0 { parray: value },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_fragment_identity_follows_the_mount_when_rows_move() {
        let cell = |row| Cell {
            row,
            column: 0,
            value: String::new(),
            feedback: String::new(),
            read_only: true,
            bounds: UiaRect::default(),
        };
        let mut snapshot = Snapshot {
            headers: vec![("Name".into(), UiaRect::default())],
            rows: vec![
                Row {
                    key: 10,
                    cells: vec![11],
                },
                Row {
                    key: 20,
                    cells: vec![21],
                },
            ],
            cells: BTreeMap::from([(11, cell(0)), (21, cell(1))]),
            ..Snapshot::default()
        };
        assert_eq!(
            snapshot.navigation(Item::Header(0), NavigateDirection_NextSibling),
            Some(Item::Row(10))
        );
        assert_eq!(
            snapshot.navigation(Item::Row(10), NavigateDirection_FirstChild),
            Some(Item::Cell(11))
        );
        assert_eq!(
            snapshot.navigation(Item::Cell(11), NavigateDirection_NextSibling),
            None
        );
        snapshot.rows.swap(0, 1);
        snapshot.cells.get_mut(&11).unwrap().row = 1;
        snapshot.cells.get_mut(&21).unwrap().row = 0;
        assert_eq!(
            snapshot.navigation(Item::Header(0), NavigateDirection_NextSibling),
            Some(Item::Row(20))
        );
        assert_eq!(
            snapshot.navigation(Item::Cell(11), NavigateDirection_Parent),
            Some(Item::Row(10))
        );
        snapshot.rows.pop();
        snapshot.cells.remove(&11);
        assert!(!snapshot.contains(Item::Row(10)));
        assert!(!snapshot.contains(Item::Cell(11)));
    }

    #[test]
    fn selection_item_changes_preserve_unrelated_cells() {
        use SelectionChange::{Add, Remove};

        let row = Some((1, 2, 1, 4));
        assert_eq!(Add.updated(row, 1, 3).unwrap(), None);
        assert_eq!(Add.updated(row, 1, 1).unwrap(), Some((1, 1, 1, 4)));
        assert_eq!(Add.updated(row, 1, 5).unwrap(), Some((1, 2, 1, 5)));
        assert_eq!(Remove.updated(row, 1, 1).unwrap(), None);
        assert_eq!(Remove.updated(row, 1, 2).unwrap(), Some((1, 3, 1, 4)));
        assert_eq!(Remove.updated(row, 1, 4).unwrap(), Some((1, 2, 1, 3)));
        assert!(Add.updated(row, 1, 6).is_err());
        assert!(Remove.updated(row, 1, 3).is_err());

        let column = Some((1, 0, 2, 0));
        assert_eq!(Add.updated(column, 0, 0).unwrap(), Some((0, 0, 2, 0)));
        assert_eq!(Remove.updated(column, 2, 0).unwrap(), Some((1, 0, 1, 0)));
        assert!(Remove.updated(Some((1, 0, 1, 0)), 1, 0).is_err());

        let square = Some((0, 0, 1, 1));
        assert!(Add.updated(square, 2, 0).is_err());
        assert!(Remove.updated(square, 0, 0).is_err());
    }

    #[test]
    fn automation_targets_are_scoped_to_enabled_live_cells() {
        let mut snapshot = Snapshot {
            enabled: true,
            rows: vec![Row {
                key: 21,
                cells: vec![10, 11],
            }],
            headers: vec![
                ("Name".into(), UiaRect::default()),
                ("Count".into(), UiaRect::default()),
            ],
            ..Snapshot::default()
        };
        snapshot.cells.insert(
            10,
            Cell {
                row: 0,
                column: 0,
                value: "Paper".into(),
                feedback: String::new(),
                read_only: true,
                bounds: UiaRect::default(),
            },
        );
        snapshot.cells.insert(
            11,
            Cell {
                row: 0,
                column: 1,
                value: "4".into(),
                feedback: String::new(),
                read_only: false,
                bounds: UiaRect::default(),
            },
        );
        let state = Arc::new(Mutex::new(State {
            snapshot: Some(Arc::new(snapshot)),
            ..State::default()
        }));
        let root = Provider::element(0, state.clone(), Item::Grid);
        let grid = root.cast::<IGridProvider>().unwrap();
        // SAFETY: These in-process provider calls use only the test snapshot, never an HWND.
        unsafe {
            let name = grid
                .GetItem(0, 0)
                .unwrap()
                .cast::<IValueProvider>()
                .unwrap();
            let value = grid
                .GetItem(0, 1)
                .unwrap()
                .cast::<IValueProvider>()
                .unwrap();
            assert!(name.SetValue(windows::core::w!("Different")).is_err());
            assert_eq!(value.Value().unwrap().to_string(), "4");
            value.SetValue(windows::core::w!("7")).unwrap();
            assert_eq!(value.Value().unwrap().to_string(), "4");
            let action = state.lock().unwrap().actions.pop_front().unwrap();
            assert_eq!(action.cell, 11);
            assert!(matches!(action.operation, Operation::SetValue(draft) if draft == "7"));

            let before = Editor {
                cell: 11,
                hwnd: 1,
                generation: 1,
                focused: true,
            };
            {
                let mut state = state.lock().unwrap();
                let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
                snapshot.active = Some(11);
                snapshot.focused = true;
                snapshot.editor = Some(before);
                snapshot.cells.get_mut(&11).unwrap().feedback = "Invalid count".into();
                assert_eq!(snapshot.focus(), Some(before.item()));
                assert_eq!(
                    snapshot.navigation(Item::Cell(11), NavigateDirection_FirstChild),
                    Some(before.item())
                );
                assert_eq!(
                    snapshot.navigation(before.item(), NavigateDirection_Parent),
                    Some(Item::Cell(11))
                );
            }
            let fragment_root = root.cast::<IRawElementProviderFragmentRoot>().unwrap();
            let edit = fragment_root.GetFocus().unwrap();
            assert!(edit.GetRuntimeId().unwrap().is_null());
            let parent = edit.Navigate(NavigateDirection_Parent).unwrap();
            let id = parent.GetRuntimeId().unwrap();
            assert!(!id.is_null());
            SafeArrayDestroy(id).unwrap();
            assert!(value.SetValue(windows::core::w!("While editing")).is_err());
            edit.SetFocus().unwrap();
            let action = state.lock().unwrap().actions.pop_front().unwrap();
            assert!(matches!(action.operation, Operation::FocusEditor(1)));
            {
                let mut state = state.lock().unwrap();
                let snapshot = Arc::make_mut(state.snapshot.as_mut().unwrap());
                snapshot.editor = Some(Editor {
                    generation: 2,
                    ..before
                });
                assert!(!snapshot.contains(before.item()));
                snapshot.enabled = false;
                assert_eq!(snapshot.focus(), None);
            }
            assert_eq!(
                edit.SetFocus().unwrap_err().code().0,
                UIA_E_ELEMENTNOTAVAILABLE as i32
            );
            assert_eq!(
                value.SetValue(windows::core::w!("9")).unwrap_err().code().0,
                UIA_E_ELEMENTNOTENABLED as i32
            );
            state.lock().unwrap().snapshot = None;
            assert_eq!(
                value.Value().unwrap_err().code().0,
                UIA_E_ELEMENTNOTAVAILABLE as i32
            );
        }
        assert!(state.lock().unwrap().actions.is_empty());
    }
}
