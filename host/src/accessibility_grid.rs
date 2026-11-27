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

#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) name: String,
    pub(crate) headers: Vec<(String, UiaRect)>,
    pub(crate) order: Vec<Key>,
    pub(crate) cells: BTreeMap<Key, Cell>,
    pub(crate) bounds: UiaRect,
    pub(crate) active: Option<Key>,
    pub(crate) range: Option<(usize, usize, usize, usize)>,
    pub(crate) focused: bool,
    pub(crate) enabled: bool,
    pub(crate) editor: Option<(Key, usize)>,
    pub(crate) scroll: [ScrollAxis; 2],
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
            Item::Cell(key) => self.cells.contains_key(&key),
            Item::Feedback(key) => self
                .cells
                .get(&key)
                .is_some_and(|cell| !cell.feedback.is_empty()),
        }
    }

    fn selected(&self, cell: &Cell) -> bool {
        self.range.is_some_and(|(top, left, bottom, right)| {
            (top..=bottom).contains(&cell.row) && (left..=right).contains(&cell.column)
        })
    }

    fn node_at(&self, index: usize) -> Option<Item> {
        if index < self.headers.len() {
            Some(Item::Header(index))
        } else {
            self.order
                .get(index - self.headers.len())
                .copied()
                .map(Item::Cell)
        }
    }

    fn index(&self, node: Item) -> Option<usize> {
        match node {
            Item::Header(index) => Some(index),
            Item::Cell(key) => {
                let cell = self.cells.get(&key)?;
                Some(self.headers.len() * (cell.row + 1) + cell.column)
            }
            _ => None,
        }
    }

    fn rectangle(&self, node: Item) -> UiaRect {
        match node {
            Item::Grid => self.bounds,
            Item::Header(index) => self.headers[index].1,
            Item::Cell(key) => self.cells[&key].bounds,
            Item::Feedback(_) => UiaRect::default(),
        }
    }

    fn navigation(&self, from: Item, direction: NavigateDirection) -> Option<Item> {
        match (from, direction) {
            (Item::Grid, NavigateDirection_FirstChild) => self.node_at(0),
            (Item::Grid, NavigateDirection_LastChild) => {
                self.node_at((self.headers.len() + self.order.len()).checked_sub(1)?)
            }
            (Item::Grid, _) => None,
            (Item::Feedback(key), NavigateDirection_Parent) => Some(Item::Cell(key)),
            (Item::Feedback(_), _) => None,
            (_, NavigateDirection_Parent) => Some(Item::Grid),
            (Item::Cell(key), NavigateDirection_FirstChild | NavigateDirection_LastChild) => self
                .contains(Item::Feedback(key))
                .then_some(Item::Feedback(key)),
            (_, NavigateDirection_NextSibling) => self.node_at(self.index(from)? + 1),
            (_, NavigateDirection_PreviousSibling) => {
                self.node_at(self.index(from)?.checked_sub(1)?)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Grid,
    Header(usize),
    Cell(Key),
    Feedback(Key),
}

pub(crate) enum Operation {
    SetValue(String),
    Select,
    ExtendSelection,
    Reveal,
    Focus,
    Scroll([i32; 2]),
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

    pub(crate) fn set_focus(&self, focused: bool) {
        let mut state = self.state.lock().unwrap();
        if let Some(snapshot) = &mut state.snapshot
            && snapshot.focused != focused
        {
            Arc::make_mut(snapshot).focused = focused;
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
        if prior.order != current.order || prior.headers.len() != current.headers.len() {
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
        if current.focused
            && (prior.active != current.active || !prior.focused)
            && let Some(key) = current.active
        {
            let cell = Provider::element(self.hwnd, self.state.clone(), Item::Cell(key));
            // SAFETY: The focused cell was read from the committed projection.
            unsafe {
                let _ = UiaRaiseAutomationEvent(&cell, UIA_AutomationFocusChangedEventId);
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
        let provider: SimpleAbi = Self { hwnd, state, item }.into();
        provider.into_provider()
    }
}

impl Provider_Impl {
    fn with<T>(
        &self,
        read: impl FnOnce(&Snapshot) -> windows::core::Result<T>,
    ) -> windows::core::Result<T> {
        let state = self.state.lock().unwrap();
        let snapshot = state
            .snapshot
            .as_ref()
            .filter(|s| s.contains(self.item))
            .ok_or_else(unavailable)?;
        read(snapshot)
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
        let mut state = self.state.lock().unwrap();
        let snapshot = state.snapshot.as_ref().ok_or_else(unavailable)?;
        let key = match self.item {
            Item::Grid if matches!(operation, Operation::Focus | Operation::Scroll(_)) => {
                snapshot.active.ok_or_else(unsupported)?
            }
            _ => self.cell()?,
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
        if state.actions.len() >= MAX_ACTIONS {
            return Err(Error::from_hresult(HRESULT(UIA_E_TIMEOUT as i32)));
        }
        state.actions.push_back(Action {
            cell: key,
            operation,
        });
        Ok(())
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
            let cell = if let Item::Cell(key) = self.item {
                s.cells.get(&key)
            } else {
                None
            };
            Ok(match id {
                UIA_ControlTypePropertyId => integer(
                    match self.item {
                        Item::Grid => UIA_DataGridControlTypeId,
                        Item::Header(_) => UIA_HeaderItemControlTypeId,
                        Item::Cell(_) => UIA_DataItemControlTypeId,
                        Item::Feedback(_) => UIA_TextControlTypeId,
                    }
                    .0,
                ),
                UIA_NamePropertyId => string(match self.item {
                    Item::Grid => &s.name,
                    Item::Header(index) => &s.headers[index].0,
                    Item::Cell(key) => &s.headers[s.cells[&key].column].0,
                    Item::Feedback(key) => &s.cells[&key].feedback,
                }),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => boolean(true),
                UIA_IsEnabledPropertyId => boolean(s.enabled),
                UIA_IsOffscreenPropertyId => boolean(s.rectangle(self.item).width == 0.0),
                UIA_IsKeyboardFocusablePropertyId => {
                    boolean(s.enabled && matches!(self.item, Item::Grid | Item::Cell(_)))
                }
                UIA_HasKeyboardFocusPropertyId => boolean(
                    s.focused
                        && match self.item {
                            Item::Grid => s.active.is_none(),
                            Item::Cell(key) => Some(key) == s.active,
                            _ => false,
                        },
                ),
                UIA_HelpTextPropertyId if cell.is_some() => string(&cell.unwrap().feedback),
                UIA_DescribedByPropertyId
                    if let Item::Cell(key) = self.item
                        && s.contains(Item::Feedback(key)) =>
                {
                    let provider = self.other(Item::Feedback(key)).cast()?;
                    provider_list(array_for([provider])?)
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
                Item::Header(index) => (1, index),
                Item::Cell(key) => (2, key),
                Item::Feedback(key) => (3, key),
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
                .find_map(|(&key, cell)| hit(cell.bounds).then_some(Item::Cell(key)))
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
        self.with(|s| {
            if s.focused {
                self.other(s.active.map_or(Item::Grid, Item::Cell)).cast()
            } else {
                Err(absent())
            }
        })
    }
}

#[implement(SimpleAbi)]
struct EditorOverride {
    hwnd: usize,
    context: IRawElementProviderSimple,
}

impl IRawElementProviderSimple_Impl for EditorOverride_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_OverrideProvider)
    }
    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        Err(absent())
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        if matches!(
            id,
            UIA_NamePropertyId | UIA_HelpTextPropertyId | UIA_DescribedByPropertyId
        ) {
            // SAFETY: The cell provider owns its synchronized retained projection.
            unsafe { self.context.GetPropertyValue(id) }
        } else {
            Ok(VARIANT::default())
        }
    }
    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        // SAFETY: UIA resolves only an extant edit window.
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
    }
}

impl IRawElementProviderHwndOverride_Impl for Provider_Impl {
    fn GetOverrideProviderForHwnd(
        &self,
        hwnd: HWND,
    ) -> windows::core::Result<IRawElementProviderSimple> {
        self.with(|s| {
            let (key, editor) = s
                .editor
                .filter(|(_, editor)| *editor == hwnd.0 as usize)
                .ok_or_else(absent)?;
            let provider: SimpleAbi = EditorOverride {
                hwnd: editor,
                context: self.other(Item::Cell(key)),
            }
            .into();
            Ok(provider.into_provider())
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
            let index = usize::try_from(row)
                .ok()
                .and_then(|r| r.checked_mul(s.headers.len()))
                .and_then(|r| r.checked_add(col))
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            Ok(self.other(Item::Cell(
                *s.order
                    .get(index)
                    .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?,
            )))
        })
    }
    fn RowCount(&self) -> windows::core::Result<i32> {
        self.with(|s| Ok(s.order.len().checked_div(s.headers.len()).unwrap_or(0) as i32))
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
                s.order
                    .iter()
                    .filter(|key| s.selected(&s.cells[key]))
                    .map(|&key| self.other(Item::Cell(key)).into()),
            )
        })
    }
    fn CanSelectMultiple(&self) -> windows::core::Result<BOOL> {
        self.with(|_| Ok(true.into()))
    }
    fn IsSelectionRequired(&self) -> windows::core::Result<BOOL> {
        self.with(|s| Ok((!s.order.is_empty()).into()))
    }
}

impl ISelectionItemProvider_Impl for Provider_Impl {
    fn Select(&self) -> windows::core::Result<()> {
        self.action(Operation::Select)
    }
    fn AddToSelection(&self) -> windows::core::Result<()> {
        self.action(Operation::ExtendSelection)
    }
    fn RemoveFromSelection(&self) -> windows::core::Result<()> {
        self.with(|s| {
            if s.selected(&s.cells[&self.cell()?]) {
                Err(unsupported())
            } else {
                Ok(())
            }
        })
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
    fn automation_targets_are_scoped_to_enabled_live_cells() {
        let mut snapshot = Snapshot {
            enabled: true,
            order: vec![10, 11],
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

            {
                let mut state = state.lock().unwrap();
                Arc::make_mut(state.snapshot.as_mut().unwrap()).enabled = false;
            }
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
