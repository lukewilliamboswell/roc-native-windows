use crate::Result;
use crate::accessibility::{ElementProvider, NativeName, Role, Semantics};
use crate::choice::Choices;
use crate::civil::{self, Kind as CivilKind, Value as CivilValue};
use crate::command_icon::ButtonIcon;
use crate::display_preferences::DisplayPreferences;
use crate::grid::Grid;
use crate::list_view::{ListView, Report};
use crate::picture::{Picture, Resize};
use crate::progress::{Progress, native::ProgressBar};
use crate::raster::Pixels;
use crate::shader::{Inputs as ShaderInputs, ShaderView};
use crate::slider::native::Slider;
use crate::style::{
    Accent,
    native::{FontRole, Fonts, Paint, Palette, StyledWindow},
};
use crate::surface_input::native as surface_input;
use crate::toolbar::Toolbar;
use crate::tree_view::{TreeIntent, TreeProjection};
use crate::window::{APPLY_PENDING_EDIT, enabled_flag};
use crate::window_chrome::{WindowDeclaration, native::StatusBar};
use crate::wire::{Key, Kind, Node, ROOT, Tree};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, BST_INDETERMINATE, BST_UNCHECKED, CCM_SETUNICODEFORMAT, CCS_NODIVIDER,
    CCS_NOPARENTALIGN, CCS_NORESIZE, DTS_SHORTDATEFORMAT, DTS_SHOWNONE, DTS_TIMEFORMAT,
    EM_SETCUEBANNER, EM_SETREADONLY, EM_SETSEL, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL,
    SetScrollInfo, TBSTYLE_FLAT, TBSTYLE_LIST, TBSTYLE_WRAPABLE, TCM_ADJUSTRECT, TOOLBARCLASSNAMEW,
    TVM_GETITEMHEIGHT, TVS_DISABLEDRAGDROP, TVS_HASBUTTONS, TVS_HASLINES, TVS_LINESATROOT,
    TVS_SHOWSELALWAYS, WC_TABCONTROLW, WC_TREEVIEWW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetActiveWindow, GetFocus, IsWindowEnabled, SetFocus,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, RemoveWindowSubclass, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Text,
    Panel,
    Button,
    FileButton,
    FileDropTarget,
    FolderButton,
    Checkbox,
    AggregateCheckbox,
    Edit,
    TextArea,
    Image,
    ShaderView,
    ComboBox,
    ListBox,
    RadioGroup,
    ListView,
    DatePicker,
    TimePicker,
    DataGrid,
    Tabs,
    ProgressBar,
    Slider,
    Toolbar,
    TreeView,
}

impl Control {
    fn from_tag(tag: &str) -> Result<Option<Self>> {
        let control = match tag {
            "" | "ui-vstack" | "ui-hstack" | "ui-window-content" | "ui-dialog-content"
            | "ui-tab-panel" | "ui-image-raster" => {
                return Ok(None);
            }
            "ui-window" => return Err("Ui.window must be the single outermost element".into()),
            "ui-text" | "ui-status-text" => Self::Text,
            "ui-panel" => Self::Panel,
            "ui-button" => Self::Button,
            "ui-file-button" => Self::FileButton,
            "ui-file-drop-target" => Self::FileDropTarget,
            "ui-folder-button" => Self::FolderButton,
            "ui-checkbox" => Self::Checkbox,
            "ui-aggregate-checkbox" => Self::AggregateCheckbox,
            "ui-text-field" => Self::Edit,
            "textarea" => Self::TextArea,
            "ui-asset-image" | "ui-image" => Self::Image,
            "ui-shader-view" => Self::ShaderView,
            "ui-combo-box" => Self::ComboBox,
            "ui-list-box" => Self::ListBox,
            "ui-radio-group" => Self::RadioGroup,
            "ui-list-view" => Self::ListView,
            "ui-date-picker" => Self::DatePicker,
            "ui-time-picker" => Self::TimePicker,
            "ui-data-grid" => Self::DataGrid,
            "ui-tabs" => Self::Tabs,
            "ui-progress-bar" => Self::ProgressBar,
            "ui-slider" => Self::Slider,
            "ui-toolbar" => Self::Toolbar,
            "ui-tree-view" => Self::TreeView,
            _ => return Err(format!("unsupported native control {tag}")),
        };
        Ok(Some(control))
    }

    pub(crate) fn is_checkbox(self) -> bool {
        matches!(self, Self::Checkbox | Self::AggregateCheckbox)
    }

    pub(crate) fn is_choice(self) -> bool {
        matches!(
            self,
            Self::ComboBox | Self::ListBox | Self::RadioGroup | Self::Tabs
        )
    }

    fn is_edit(self) -> bool {
        matches!(self, Self::Edit | Self::TextArea)
    }

    pub(crate) fn civil(self) -> Option<CivilKind> {
        match self {
            Self::DatePicker => Some(CivilKind::Date),
            Self::TimePicker => Some(CivilKind::Time),
            _ => None,
        }
    }

    fn separate_label(self) -> bool {
        matches!(
            self,
            Self::Edit
                | Self::TextArea
                | Self::ListView
                | Self::DataGrid
                | Self::Tabs
                | Self::TreeView
                | Self::ProgressBar
                | Self::Slider
                | Self::ListBox
                | Self::ComboBox
                | Self::DatePicker
                | Self::TimePicker
        )
    }

    fn class(self) -> (u32, PCWSTR, u32) {
        match self {
            Self::Toolbar => (
                0,
                TOOLBARCLASSNAMEW,
                WS_TABSTOP
                    | (CCS_NODIVIDER | CCS_NOPARENTALIGN | CCS_NORESIZE) as u32
                    | (TBSTYLE_FLAT | TBSTYLE_LIST | TBSTYLE_WRAPABLE),
            ),
            Self::Text => (0, w!("STATIC"), SS_LEFT | SS_NOPREFIX),
            Self::Panel => (0, w!("STATIC"), 0),
            Self::Button | Self::FileButton | Self::FolderButton => {
                (0, w!("BUTTON"), WS_TABSTOP | BS_PUSHBUTTON as u32)
            }
            Self::FileDropTarget => (0, w!("BUTTON"), BS_GROUPBOX as u32 | WS_CLIPSIBLINGS),
            Self::Checkbox => (0, w!("BUTTON"), WS_TABSTOP | BS_AUTOCHECKBOX as u32),
            Self::AggregateCheckbox => (0, w!("BUTTON"), WS_TABSTOP | BS_3STATE as u32),
            Self::Edit => (
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
            ),
            Self::TextArea => (
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                WS_TABSTOP
                    | WS_VSCROLL
                    | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_READONLY) as u32,
            ),
            Self::Image | Self::ShaderView => (0, w!("STATIC"), SS_LEFT | SS_NOPREFIX),
            Self::ComboBox => (
                0,
                w!("COMBOBOX"),
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            ),
            Self::ListBox => (
                WS_EX_CLIENTEDGE,
                w!("LISTBOX"),
                WS_TABSTOP | WS_VSCROLL | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            ),
            Self::RadioGroup => (WS_EX_CONTROLPARENT, w!("BUTTON"), BS_GROUPBOX as u32),
            Self::Slider => (0, w!("msctls_trackbar32"), WS_TABSTOP),
            Self::ProgressBar => (0, w!("msctls_progress32"), 0),
            Self::ListView | Self::DataGrid => (
                WS_EX_CLIENTEDGE,
                w!("SysListView32"),
                WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            ),
            Self::DatePicker => (
                0,
                w!("SysDateTimePick32"),
                WS_TABSTOP | DTS_SHOWNONE | DTS_SHORTDATEFORMAT,
            ),
            Self::TimePicker => (
                0,
                w!("SysDateTimePick32"),
                WS_TABSTOP | DTS_SHOWNONE | DTS_TIMEFORMAT,
            ),
            Self::Tabs => (0, WC_TABCONTROLW, WS_TABSTOP | WS_CLIPSIBLINGS),
            Self::TreeView => (
                WS_EX_CLIENTEDGE,
                WC_TREEVIEWW,
                WS_TABSTOP
                    | TVS_HASLINES
                    | TVS_LINESATROOT
                    | TVS_HASBUTTONS
                    | TVS_SHOWSELALWAYS
                    | TVS_DISABLEDRAGDROP,
            ),
        }
    }
}

struct View {
    handle: HWND,
    label: HWND,
    native_name: Option<NativeName>,
    semantics: Option<ElementProvider>,
    kind: Control,
    props: Props,
    styled: Option<StyledWindow>,
    label_style: Option<StyledWindow>,
    font_role: FontRole,
    force_pending: bool,
    picture: Option<Picture>,
    shader: Option<ShaderView>,
    choices: Choices,
    tree: TreeProjection,
    tab_owner: Option<Key>,
    combo_position: Option<(HWND, i32, i32, i32, i32)>,
    list_view: Option<ListView>,
    grid: Option<Grid>,
    progress: ProgressBar,
    slider: Slider,
    input_attached: bool,
    toolbar: Option<Toolbar>,
    button_icon: ButtonIcon,
}

#[derive(Default)]
struct Props {
    caption: String,
    label: String,
    placeholder: String,
    enabled: bool,
    check_state: u32,
    window_default: bool,
    command_icon: String,
    editable: bool,
    ellipsize: bool,
    civil: Option<CivilValue>,
}

impl View {
    fn new(window: HWND, font: HFONT, kind: Control) -> Result<Self> {
        let (ex, class, style) = kind.class();
        let handle = child(window, font, ex, class, style)?;
        if kind == Control::TreeView {
            // SAFETY: Configure wide notifications before inserting any items.
            unsafe { SendMessageW(handle, CCM_SETUNICODEFORMAT, 1, 0) };
        }
        if kind.is_edit() {
            // SAFETY: The new HWND belongs to this thread, with a static callback.
            if unsafe {
                SetWindowSubclass(
                    handle,
                    Some(edit_subclass),
                    1,
                    usize::from(kind == Control::TextArea),
                )
            } == 0
            {
                unsafe { DestroyWindow(handle) };
                return Err("could not track input method composition".into());
            }
        }
        if kind == Control::ProgressBar {
            ProgressBar::initialize(handle);
        }
        if matches!(kind, Control::ListView | Control::DataGrid) {
            ListView::initialize(handle);
        }
        let list_view = if kind == Control::ListView {
            Some(ListView::default())
        } else {
            None
        };
        let grid = (kind == Control::DataGrid).then(Grid::default);
        let toolbar = (kind == Control::Toolbar).then(|| {
            Toolbar::initialize(handle);
            Toolbar::default()
        });
        let mount_picture = || -> Result<_> {
            let picture = (kind == Control::Image)
                .then(|| Picture::new(handle))
                .transpose()?;
            let shader = (kind == Control::ShaderView)
                .then(|| ShaderView::attach(handle))
                .transpose()?;
            Ok((picture, shader))
        };
        let (picture, shader) = mount_picture().inspect_err(|_| {
            // SAFETY: The failed control mount belongs to this thread.
            unsafe { DestroyWindow(handle) };
        })?;
        let view = Self {
            handle,
            label: null_mut(),
            native_name: None,
            semantics: None,
            kind,
            styled: None,
            label_style: None,
            font_role: FontRole::Body,
            props: Props {
                enabled: true,
                ..Props::default()
            },
            force_pending: false,
            picture,
            shader,
            choices: Choices::default(),
            tree: TreeProjection::default(),
            tab_owner: None,
            combo_position: None,
            list_view,
            progress: ProgressBar::default(),
            slider: Slider::default(),
            input_attached: false,
            grid,
            toolbar,
            button_icon: ButtonIcon::default(),
        };
        if let Some(mode) = kind.civil() {
            civil::native::initialize(handle, mode)?;
        }
        Ok(view)
    }

    fn label_title(&self) -> Cow<'_, str> {
        if self.progress.steady() {
            if self.props.label.is_empty() {
                return Cow::Borrowed("Busy");
            }
            return Cow::Owned(format!("{} (busy)", self.props.label));
        }
        Cow::Borrowed(&self.props.label)
    }

    fn label_height(&self, dc: HDC, dpi: u32, width: i32) -> i32 {
        let title = self.label_title();
        if self.label.is_null() || title.is_empty() {
            0
        } else {
            (wrap_height(dc, &title, width) + scale(dpi, 3)).max(scale(dpi, 21))
        }
    }

    fn font(&self, font: HFONT) {
        self.choices.font(font);
        // SAFETY: Both child windows, when present, are owned by this view.
        unsafe {
            SendMessageW(self.handle, WM_SETFONT, font as usize, 0);
            if !self.label.is_null() {
                SendMessageW(self.label, WM_SETFONT, font as usize, 0);
            }
        }
    }

    fn paint(&mut self, paint: Paint) -> Result<()> {
        if let Some(style) = &mut self.styled {
            style.update(paint);
        } else {
            self.styled = Some(StyledWindow::new(self.handle, paint)?);
        }
        Ok(())
    }

    fn update(&mut self, window: HWND, font: HFONT, desired: Props) -> Result<()> {
        // Compare the controlled edit value with what's on screen before
        // writing. Writing the same string can reset the selection and IME.
        if !matches!(self.kind, Control::ListBox | Control::ComboBox)
            && self.props.caption != desired.caption
        {
            if self.kind == Control::TextArea {
                if !desired.editable || !edit_is_active(self.handle) {
                    self.write_edit(&desired.caption);
                }
            } else if self.kind != Control::Edit || window_text(self.handle) != desired.caption {
                set_text(self.handle, &desired.caption);
            }
        }
        if self.kind.separate_label() && self.props.label != desired.label {
            NativeName::annotate(self.handle, &desired.label, &mut self.native_name);
        }
        if self.kind.separate_label()
            && self.label.is_null()
            && (!desired.label.is_empty() || self.kind == Control::ProgressBar)
        {
            self.label = child(window, font, 0, w!("STATIC"), SS_LEFT | SS_NOPREFIX)?;
        }

        // SAFETY: WM_ENABLE, BM_SETCHECK and EM_SETCUEBANNER operate on this
        // view's live window on its creating thread. The string is retained
        // until the synchronous cue-banner message returns.
        unsafe {
            if self.kind == Control::TextArea && self.props.editable != desired.editable {
                SendMessageW(
                    self.handle,
                    EM_SETREADONLY,
                    usize::from(!desired.editable),
                    0,
                );
            }
            if self.kind == Control::Text && self.props.ellipsize != desired.ellipsize {
                let style = GetWindowLongPtrW(self.handle, GWL_STYLE) as u32;
                let ellipsis = if desired.ellipsize { SS_ENDELLIPSIS } else { 0 };
                SetWindowLongPtrW(
                    self.handle,
                    GWL_STYLE,
                    ((style & !SS_ENDELLIPSIS) | ellipsis) as isize,
                );
                InvalidateRect(self.handle, null(), 1);
            }
            if self.props.enabled != desired.enabled {
                EnableWindow(self.handle, i32::from(desired.enabled));
            }
            if self.kind.is_checkbox() && self.props.check_state != desired.check_state {
                SendMessageW(self.handle, BM_SETCHECK, desired.check_state as usize, 0);
            }
            if self.kind == Control::Button && self.props.window_default != desired.window_default {
                let style = if desired.window_default {
                    BS_DEFPUSHBUTTON
                } else {
                    BS_PUSHBUTTON
                };
                SendMessageW(self.handle, BM_SETSTYLE, style as usize, 1);
            }
            if self.kind == Control::Edit && self.props.placeholder != desired.placeholder {
                let placeholder = wide(&desired.placeholder);
                SendMessageW(
                    self.handle,
                    EM_SETCUEBANNER,
                    0,
                    placeholder.as_ptr() as isize,
                );
            }
        }
        if let (Some(mode), Some(value)) = (self.kind.civil(), desired.civil) {
            civil::native::restore(self.handle, mode, value)?;
        }
        self.props = desired;
        Ok(())
    }

    fn read_edit(&self) -> String {
        let text = window_text(self.handle);
        if self.kind == Control::TextArea {
            canonical_lines(&text)
        } else {
            text
        }
    }

    fn write_edit(&self, value: &str) {
        if self.read_edit() == value {
            return;
        }
        if self.kind == Control::TextArea {
            set_text(self.handle, &native_lines(value));
        } else {
            set_text(self.handle, value);
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        drop(self.semantics.take());
        drop(self.native_name.take());
        drop(self.picture.take());
        drop(self.grid.take());
        // SAFETY: Children are owned by this surface and were created on
        // this thread. DestroyWindow also tolerates an already closed HWND.
        unsafe {
            DestroyWindow(self.handle);
            if !self.label.is_null() {
                DestroyWindow(self.label);
            }
        }
    }
}

pub(crate) struct Surface {
    window: HWND,
    content: Key,
    status: Option<StatusBar>,
    taskbar_progress: Progress,
    font: HFONT,
    fonts: Fonts,
    accent: Accent,
    palette: Palette,
    dpi: u32,
    preferences: DisplayPreferences,
    assets: PathBuf,
    views: BTreeMap<Key, View>,
    raster_inputs: BTreeMap<Key, Arc<Pixels>>,
    handles: BTreeMap<usize, Key>,
    scroll_position: i32,
    last_focus: HWND,
    wheel_delta: i32,
}

impl Surface {
    pub(crate) fn new(window: HWND, preferences: DisplayPreferences) -> Result<Self> {
        // SAFETY: The HWND has been created and lives on this thread.
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let assets = std::env::current_exe()
            .map_err(|error| format!("cannot locate app assets: {error}"))?
            .with_file_name("assets");
        let fonts = Fonts::new(dpi, preferences.text_scale)?;
        let accent = Accent::default();
        Ok(Self {
            window,
            content: ROOT,
            status: None,
            taskbar_progress: Progress::Idle,
            font: fonts.get(FontRole::Body),
            fonts,
            accent,
            palette: Palette::new(accent, window),
            dpi,
            preferences,
            assets,
            views: BTreeMap::new(),
            raster_inputs: BTreeMap::new(),
            handles: BTreeMap::new(),
            scroll_position: 0,
            last_focus: null_mut(),
            wheel_delta: 0,
        })
    }

    pub(crate) fn dialog_size(&self, tree: &Tree) -> (i32, i32) {
        // SAFETY: The surface owns the live HWND and font on its UI thread.
        let dc = unsafe { GetDC(self.window) };
        let width = scale(self.dpi, 480);
        if dc.is_null() {
            return (width, scale(self.dpi, 320));
        }
        let height = unsafe {
            let previous = SelectObject(dc, self.font);
            let (_, height) = self.layout(tree, self.content, dc, width, 0, 0, &mut Vec::new());
            SelectObject(dc, previous);
            ReleaseDC(self.window, dc);
            height
        };
        (width, height.max(scale(self.dpi, 96)))
    }

    pub(crate) fn title(&self, title: &str) {
        set_text(self.window, title);
    }

    pub(crate) fn raster(&self, key: Key) -> Option<Arc<Pixels>> {
        self.raster_inputs.get(&key).cloned()
    }

    pub(crate) fn set_raster(&mut self, key: Key, pixels: Option<Arc<Pixels>>) {
        if let Some(pixels) = pixels {
            self.raster_inputs.insert(key, pixels);
        } else {
            self.raster_inputs.remove(&key);
        }
    }

    pub(crate) fn image_resizes(&mut self) -> impl Iterator<Item = (Key, Resize)> + '_ {
        self.views.iter_mut().filter_map(|(&key, view)| {
            view.picture
                .as_mut()?
                .take_resize()
                .map(|resize| (key, resize))
        })
    }

    pub(crate) fn accept_image_resize(
        &mut self,
        key: Key,
        source: Arc<Pixels>,
        image: Arc<Pixels>,
    ) -> Result<()> {
        if let Some(picture) = self
            .views
            .get_mut(&key)
            .and_then(|view| view.picture.as_mut())
        {
            picture.accept_resize(source, image)?;
        }
        Ok(())
    }

    pub(crate) fn key(&self, handle: HWND) -> Option<Key> {
        self.handles.get(&(handle as usize)).copied().or_else(|| {
            self.views
                .iter()
                .find(|(_, view)| view.choices.contains_radio(handle))
                .map(|(&key, _)| key)
        })
    }

    pub(crate) fn choice_event(
        &mut self,
        key: Key,
        child: HWND,
        notification: u32,
    ) -> Option<String> {
        let view = self.views.get_mut(&key)?;
        view.choices
            .changed(view.handle, view.kind, child, notification)
    }

    pub(crate) fn spec_choice(&self, key: Key, selected: &str) -> Result<(HWND, u32)> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind.is_choice())
            .ok_or("select requires a choice control")?;
        view.choices.propose(view.handle, view.kind, selected)
    }

    pub(crate) fn spec_selection(&self, key: Key) -> Result<&str> {
        let view = self
            .views
            .get(&key)
            .ok_or("locator is not a native control")?;
        if view.kind == Control::TreeView {
            return Ok(view.tree.selected(view.handle));
        }
        if !view.kind.is_choice() {
            return Err("expect-selection requires a choice control".into());
        }
        Ok(view.choices.selected_key(view.handle, view.kind))
    }

    pub(crate) fn register_file_drop(&self, root_listener: bool) {
        let accepted = root_listener
            || self
                .views
                .values()
                .any(|view| view.kind == Control::FileDropTarget);
        // SAFETY: This is the live app window on the UI thread.
        unsafe {
            let style = GetWindowLongPtrW(self.window, GWL_EXSTYLE);
            if ((style & WS_EX_ACCEPTFILES as isize) != 0) != accepted {
                DragAcceptFiles(self.window, i32::from(accepted));
            }
        }
    }

    pub(crate) fn drop_target(&self, tree: &Tree, point: POINT) -> Option<Key> {
        // SAFETY: The point is in the root coordinate system. The lookup
        // follows the live child HWND in z order, including system controls.
        let hwnd = unsafe { ChildWindowFromPointEx(self.window, point, CWP_SKIPINVISIBLE) };
        if hwnd.is_null() || hwnd == self.window {
            return None;
        }
        let mut key = self.key(hwnd).or_else(|| {
            self.views
                .iter()
                .find_map(|(&key, view)| (hwnd == view.label).then_some(key))
        })?;
        loop {
            if self
                .views
                .get(&key)
                .is_some_and(|view| view.kind == Control::FileDropTarget)
            {
                return Some(key);
            }
            key = tree.parent(key)?;
        }
    }

    pub(crate) fn spec_drop_point(&self, tree: &Tree, target: Option<Key>) -> Result<POINT> {
        // SAFETY: The rectangles are writable and their HWNDs are live views.
        let candidates = unsafe {
            let mut client = RECT::default();
            GetClientRect(self.window, &mut client);
            let bounds = if let Some(key) = target {
                let view = self
                    .views
                    .get(&key)
                    .filter(|view| view.kind == Control::FileDropTarget)
                    .ok_or("drop-files requires a file drop target")?;
                let mut frame = RECT::default();
                GetWindowRect(view.handle, &mut frame);
                MapWindowPoints(null_mut(), self.window, (&mut frame as *mut RECT).cast(), 2);
                RECT {
                    left: frame.left.max(client.left),
                    top: frame.top.max(client.top),
                    right: frame.right.min(client.right),
                    bottom: frame.bottom.min(client.bottom),
                }
            } else {
                client
            };
            if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
                return Err("drop area is outside the viewport".into());
            }
            [
                POINT {
                    x: bounds.left,
                    y: bounds.top,
                },
                POINT {
                    x: bounds.right - 1,
                    y: bounds.top,
                },
                POINT {
                    x: bounds.left,
                    y: bounds.bottom - 1,
                },
                POINT {
                    x: bounds.right - 1,
                    y: bounds.bottom - 1,
                },
                POINT {
                    x: (bounds.left + bounds.right) / 2,
                    y: (bounds.top + bounds.bottom) / 2,
                },
            ]
        };
        candidates
            .into_iter()
            .find(|&point| self.drop_target(tree, point) == target)
            .ok_or_else(|| "no exposed point for the requested drop area".into())
    }

    pub(crate) fn tree_notification(
        &mut self,
        key: Key,
        code: u32,
        item: isize,
        action: u32,
    ) -> bool {
        self.views
            .get_mut(&key)
            .is_some_and(|view| view.tree.capture(code, item, action))
    }

    pub(crate) fn tree_intent(&mut self, key: Key) -> Option<TreeIntent> {
        self.views.get_mut(&key)?.tree.take_intent()
    }

    pub(crate) fn spec_tree_select(&self, key: Key, selected: &str) -> Result<Option<TreeIntent>> {
        let view = self.spec_tree(key)?;
        view.tree.propose_selection(view.handle, selected)
    }

    pub(crate) fn spec_tree_expand(
        &self,
        key: Key,
        item: &str,
        expanded: bool,
    ) -> Result<Option<TreeIntent>> {
        let view = self.spec_tree(key)?;
        view.tree.propose_expansion(view.handle, item, expanded)
    }

    pub(crate) fn spec_tree_expanded(&self, key: Key, item: &str) -> Result<bool> {
        let view = self.spec_tree(key)?;
        view.tree.expanded(view.handle, item)
    }

    fn spec_tree(&self, key: Key) -> Result<&View> {
        self.views
            .get(&key)
            .filter(|view| view.kind == Control::TreeView)
            .ok_or_else(|| "operation requires a tree view".into())
    }

    pub(crate) fn kind(&self, key: Key) -> Control {
        self.views[&key].kind
    }

    pub(crate) fn input(&self, key: Key) -> String {
        let view = &self.views[&key];
        if view.kind.is_edit() {
            return view.read_edit();
        }
        let checked = self.check_state(key) == "true";
        (if view.kind == Control::AggregateCheckbox {
            !checked
        } else {
            checked
        })
        .to_string()
    }

    pub(crate) fn check_state(&self, key: Key) -> &str {
        // SAFETY: The child is a live BUTTON with check-box semantics.
        match unsafe { SendMessageW(self.views[&key].handle, BM_GETCHECK, 0, 0) } as u32 {
            BST_CHECKED => "true",
            BST_INDETERMINATE => "mixed",
            _ => "false",
        }
    }

    pub(crate) fn default_button(&self, focus: HWND, message: Option<&MSG>) -> Option<HWND> {
        let default = self.views.values().find(|view| view.props.window_default)?;
        let focused = self.key(focus).and_then(|key| self.views.get(&key));
        if focused.is_some_and(|view| view.kind == Control::TextArea) {
            return None;
        }
        if !focus.is_null() {
            let parameter = message.map_or(0, |msg| std::ptr::from_ref(msg) as isize);
            // SAFETY: The message pointer, when present, is live for this SendMessage call.
            let flags = unsafe { SendMessageW(focus, WM_GETDLGCODE, 13, parameter) } as u32;
            if flags & DLGC_WANTALLKEYS != 0 || composing(focus) {
                return None;
            }
        }
        let target = focused
            .filter(|view| {
                matches!(
                    view.kind,
                    Control::Button | Control::FileButton | Control::FolderButton
                )
            })
            .unwrap_or(default);
        // SAFETY: The target is a mounted control owned by this thread.
        (unsafe { IsWindowEnabled(target.handle) } != 0).then_some(target.handle)
    }

    pub(crate) fn toolbar(&self, handle: HWND) -> Option<&Toolbar> {
        let key = self.key(handle)?;
        self.views.get(&key)?.toolbar.as_ref()
    }

    pub(crate) fn toolbar_command(&self, node: Key) -> Option<(HWND, u16)> {
        self.views
            .values()
            .find_map(|view| Some((view.handle, view.toolbar.as_ref()?.id(node)?)))
    }

    pub(crate) fn spec_handle(&self, key: Key) -> Option<HWND> {
        self.views.get(&key).map(|view| view.handle)
    }

    pub(crate) fn spec_label(&self, key: Key, kind: &str) -> Option<String> {
        if kind == "toolbar-button" {
            let (handle, id) = self.toolbar_command(key)?;
            return self.toolbar(handle)?.text(handle, id);
        }
        let view = self.views.get(&key)?;
        if matches!(kind, "text-field" | "text-area") {
            return Some(view.props.label.clone());
        }
        if view.kind.separate_label() {
            return Some(view.props.label.clone());
        }
        if matches!(kind, "image" | "list-view" | "progress-bar") {
            return Some(view.props.label.clone());
        }
        Some(window_text(view.handle))
    }

    pub(crate) fn poll_shaders(&mut self) {
        for view in self.views.values_mut() {
            if let Some(semantics) = &mut view.semantics {
                semantics.announce();
            }
            if let Some(shader) = &mut view.shader {
                shader.poll(self.window);
            }
        }
    }

    pub(crate) fn shader_status(&self, key: Key) -> Result<&'static str> {
        Ok(self.shader_at(key)?.status())
    }

    pub(crate) fn shader_pixel(&self, key: Key, x: u32, y: u32) -> Result<[u8; 3]> {
        self.shader_at(key)?.pixel(x, y)
    }

    fn shader_at(&self, key: Key) -> Result<&ShaderView> {
        self.views
            .get(&key)
            .and_then(|view| view.shader.as_ref())
            .ok_or("step requires a shader view".into())
    }

    pub(crate) fn taskbar_progress(&self) -> Progress {
        self.taskbar_progress
    }

    pub(crate) fn status_texts(&self) -> Vec<String> {
        self.status.as_ref().map_or_else(Vec::new, StatusBar::texts)
    }

    pub(crate) fn slider_change(&self, key: Key) -> Option<i32> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind == Control::Slider)?;
        view.slider.change(view.handle)
    }

    pub(crate) fn spec_slider(&self, key: Key, proposal: Option<i32>) -> Result<(HWND, i32)> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind == Control::Slider)
            .ok_or("locator needs a native slider")?;
        if let Some(value) = proposal {
            if self.spec_disabled(key)? {
                return Err("control is disabled".into());
            }
            Slider::propose(view.handle, value);
        }
        Ok((view.handle, Slider::actual(view.handle)))
    }

    pub(crate) fn spec_progress(&self, key: Key, wire: &str) -> Result<()> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind == Control::ProgressBar)
            .ok_or("expect-progress requires a native progress bar")?;
        view.progress
            .expect(view.handle, view.label, Progress::parse(wire)?)
    }

    pub(crate) fn spec_text(&self, key: Key) -> Result<String> {
        if let Some((handle, id)) = self.toolbar_command(key) {
            return self
                .toolbar(handle)
                .and_then(|toolbar| toolbar.text(handle, id))
                .ok_or("toolbar button has no caption".into());
        }
        let view = self
            .views
            .get(&key)
            .ok_or("locator is not a native control")?;
        Ok(view.read_edit())
    }

    pub(crate) fn spec_disabled(&self, key: Key) -> Result<bool> {
        if let Some((handle, id)) = self.toolbar_command(key) {
            return Ok(self
                .toolbar(handle)
                .is_none_or(|toolbar| toolbar.dispatch(handle, id).is_none()));
        }
        let handle = self
            .spec_handle(key)
            .ok_or("locator is not a native control")?;
        // SAFETY: The HWND is a live control on the UI thread.
        Ok(unsafe { IsWindowEnabled(handle) } == 0)
    }

    pub(crate) fn spec_input(&self, key: Key, text: &str) -> Result<HWND> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind.is_edit())
            .ok_or("fill requires a text field or text area")?;
        if view.kind == Control::TextArea && !view.props.editable {
            return Err("cannot fill a read-only text area".into());
        }
        if view.kind == Control::TextArea {
            view.write_edit(&canonical_lines(text));
        } else {
            view.write_edit(text);
        }
        Ok(view.handle)
    }

    pub(crate) fn spec_toggle(&self, key: Key) -> Result<HWND> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind.is_checkbox())
            .ok_or("toggle requires a checkbox")?;
        if view.kind == Control::Checkbox {
            let checked = self.check_state(key) == "true";
            // SAFETY: Simulate the native auto-checkbox style before BN_CLICKED.
            unsafe { SendMessageW(view.handle, BM_SETCHECK, usize::from(!checked), 0) };
        }
        Ok(view.handle)
    }

    pub(crate) fn report(&self, key: Key) -> Option<(HWND, &ListView)> {
        let view = self.views.get(&key)?;
        Some((view.handle, view.list_view.as_ref()?))
    }

    pub(crate) fn grid(&self, key: Key) -> Option<(HWND, &Grid)> {
        let view = self.views.get(&key)?;
        Some((view.handle, view.grid.as_ref()?))
    }

    pub(crate) fn grid_mut(&mut self, key: Key) -> Option<(HWND, &mut Grid)> {
        let view = self.views.get_mut(&key)?;
        Some((view.handle, view.grid.as_mut()?))
    }

    pub(crate) fn automation_actions(
        &mut self,
    ) -> impl Iterator<Item = (Key, crate::accessibility_grid::Action)> + '_ {
        self.views.iter_mut().flat_map(|(&key, view)| {
            let hwnd = view.handle;
            view.grid.iter_mut().flat_map(move |grid| {
                grid.refresh_automation_focus(hwnd);
                grid.take_automation().map(move |action| (key, action))
            })
        })
    }

    pub(crate) fn grid_coordinates(&self) -> (HWND, HFONT, u32) {
        (self.window, self.font, self.dpi)
    }

    pub(crate) fn grid_key(&self, hwnd: HWND) -> Option<Key> {
        self.key(hwnd)
            .filter(|&key| self.grid(key).is_some())
            .or_else(|| {
                self.views.iter().find_map(|(&key, view)| {
                    view.grid
                        .as_ref()
                        .filter(|grid| grid.editing(hwnd))
                        .map(|_| key)
                })
            })
    }

    pub(crate) fn restore_controlled_value(&self, key: Key) -> Result<()> {
        let Some(view) = self.views.get(&key) else {
            return Ok(());
        };
        match view.kind {
            Control::Edit => view.write_edit(&view.props.caption),
            Control::TextArea if !edit_is_active(view.handle) => {
                view.write_edit(&view.props.caption);
            }
            Control::Checkbox | Control::AggregateCheckbox => {
                // SAFETY: The handle is a live check box owned by this thread.
                let actual = unsafe { SendMessageW(view.handle, BM_GETCHECK, 0, 0) } as u32;
                if actual != view.props.check_state {
                    unsafe {
                        SendMessageW(view.handle, BM_SETCHECK, view.props.check_state as usize, 0)
                    };
                }
            }
            kind if kind.is_choice() => view.choices.restore(view.handle, kind),
            Control::TreeView => view.tree.restore(view.handle),
            Control::Slider => view.slider.restore(view.handle),
            _ => {}
        }
        if let (Some(mode), Some(value)) = (view.kind.civil(), view.props.civil) {
            civil::native::restore(view.handle, mode, value)?;
        }
        Ok(())
    }

    pub(crate) fn civil_changed(&self, key: Key, value: CivilValue) -> bool {
        self.views
            .get(&key)
            .is_some_and(|view| view.props.civil.is_some_and(|current| current != value))
    }

    pub(crate) fn spec_civil(
        &self,
        key: Key,
        mode: CivilKind,
        replacement: Option<&str>,
    ) -> Result<CivilValue> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind.civil() == Some(mode))
            .ok_or("locator needs a picker of the requested type")?;
        if let Some(text) = replacement {
            if self.spec_disabled(key)? {
                return Err("control is disabled".into());
            }
            civil::native::restore(view.handle, mode, CivilValue::parse(mode, text)?)?;
        }
        civil::native::read(view.handle, mode)
    }

    pub(crate) fn settle_edit(&mut self, handle: HWND) {
        let Some(view) = self.key(handle).and_then(|key| self.views.get_mut(&key)) else {
            return;
        };
        if view.kind == Control::TextArea
            && !composing(handle)
            && (view.force_pending || !edit_is_active(handle))
        {
            view.force_pending = false;
            view.write_edit(&view.props.caption);
        }
    }

    pub(crate) fn reference(&mut self, tree: &Tree, op: u8, key: Key) -> Result<()> {
        let active = self.active();
        let Some(view) = self.views.get_mut(&key) else {
            return Ok(());
        };
        let handle = view.handle;
        let is_edit = view.kind.is_edit();
        match op {
            1 | 3 | 5 if active => {
                if let Some(target) = self
                    .tab_order()
                    .into_iter()
                    .find(|&item| item == handle || self.focus_key(item) == Some(key))
                {
                    // SAFETY: The target has a live Tab stop on this active surface.
                    unsafe {
                        SetFocus(target);
                        if op == 3 && is_edit {
                            SendMessageW(target, EM_SETSEL, 0, -1);
                        }
                    }
                    self.reveal_focus(tree, true)?;
                }
            }
            2 if active => {
                // SAFETY: The parent is on the current UI thread.
                unsafe { SetFocus(self.window) };
            }
            4 if is_edit => {
                view.force_pending = composing(handle);
                if !view.force_pending {
                    view.write_edit(&view.props.caption);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn dpi_changed(&mut self, tree: &Tree) -> Result<()> {
        // SAFETY: WM_DPICHANGED names our live root HWND.
        let dpi = unsafe { GetDpiForWindow(self.window) }.max(96);
        let fonts = Fonts::new(dpi, self.preferences.text_scale)?;
        let font = fonts.get(FontRole::Body);
        for view in self.views.values() {
            view.font(fonts.get(view.font_role));
            if let Some(grid) = &view.grid {
                grid.font(font);
            }
        }
        if let Some(status) = &self.status {
            status.font(font);
        }
        self.font = font;
        self.fonts = fonts;
        for (&key, view) in &mut self.views {
            if let Some(toolbar) = &mut view.toolbar {
                toolbar.sync(view.handle, tree, key, &self.assets, dpi)?;
            }
            if view.kind == Control::Button {
                view.button_icon
                    .sync(view.handle, &self.assets, &view.props.command_icon, dpi);
            }
            if let Some(report) = &mut view.list_view {
                report.measure(view.handle, font, dpi)?;
            }
            if let Some(grid) = &mut view.grid {
                grid.report.measure(view.handle, font, dpi)?;
            }
        }
        self.scroll_position =
            ((i64::from(self.scroll_position) * i64::from(dpi)) / i64::from(self.dpi)) as i32;
        self.dpi = dpi;
        self.arrange(tree)?;
        self.reveal_focus(tree, true)
    }

    pub(crate) fn display_preferences(
        &mut self,
        tree: &Tree,
        preferences: DisplayPreferences,
    ) -> Result<()> {
        let old = self.preferences;
        self.preferences = preferences;
        if old.animations != preferences.animations {
            for view in self
                .views
                .values_mut()
                .filter(|view| view.kind == Control::ProgressBar)
            {
                view.progress.motion(view.handle, preferences.animations);
            }
        }
        if old.text_scale != preferences.text_scale {
            self.dpi_changed(tree)
        } else {
            self.arrange(tree)
        }
    }

    pub(crate) fn theme_changed(&mut self, tree: &Tree) -> Result<()> {
        self.palette = Palette::new(self.accent, self.window);
        self.style_node(tree, self.content, self.palette.background())?;
        self.dpi_changed(tree)
    }

    pub(crate) fn accent(&self) -> Accent {
        self.accent
    }

    fn style_node(&mut self, tree: &Tree, key: Key, mut background: u32) -> Result<()> {
        let node = tree.node(key);
        let Kind::Element(tag) = &node.kind else {
            return Ok(());
        };
        if tag == "ui-dialog" {
            return Ok(());
        }
        if let Some(view) = self.views.get_mut(&key) {
            if view.kind == Control::Panel {
                let panel = self.palette.panel(
                    node.attr("accent-tint").is_some_and(enabled_flag),
                    node.attr("outline").is_some_and(enabled_flag),
                );
                background = panel.background;
                view.paint(panel)?;
            } else if view.kind == Control::Text {
                view.paint(self.palette.text(
                    background,
                    node.attr("text-tone").unwrap_or(""),
                    node.attr("text-align").unwrap_or(""),
                    view.props.ellipsize,
                ))?;
            }
            if !view.label.is_null() {
                let label = self.palette.text(background, "primary", "start", true);
                if let Some(style) = &mut view.label_style {
                    style.update(label);
                } else {
                    view.label_style = Some(StyledWindow::new(view.label, label)?);
                }
            }
            if let Some(picture) = &mut view.picture {
                picture.set_background(background);
            }
            if let Some(shader) = &mut view.shader {
                shader.set_background(background);
            }
        }
        for &child in &node.children {
            self.style_node(tree, child, background)?;
        }
        Ok(())
    }

    pub(crate) fn sync(
        &mut self,
        tree: &Tree,
        wrapper: WindowDeclaration<'_>,
        scopes: &BTreeMap<Key, Key>,
        owner: Key,
        accent: Accent,
    ) -> Result<()> {
        let owns = |key: &Key| scopes.get(key) == Some(&owner);
        let focus = self.snapshot_focus();
        self.content = wrapper.content;
        if self.accent != accent {
            self.accent = accent;
            self.palette = Palette::new(accent, self.window);
        }
        self.taskbar_progress = wrapper.taskbar_progress;
        if let Some(declaration) = wrapper.status {
            if self.status.is_none() {
                self.status = Some(StatusBar::new(self.window, self.font)?);
            }
            self.status.as_mut().unwrap().update(declaration);
        } else {
            self.status = None;
        }
        self.raster_inputs.retain(|key, _| owns(key));
        self.views.retain(|key, view| {
            if owns(key) {
                true
            } else {
                self.handles.remove(&(view.handle as usize));
                false
            }
        });
        self.sync_node(tree, self.content, None)?;
        self.style_node(tree, self.content, self.palette.background())?;
        if self
            .views
            .values()
            .filter(|view| view.props.window_default)
            .count()
            > 1
        {
            return Err("a window can have only one default button".into());
        }
        self.arrange(tree)?;
        if let Some(focus) = focus {
            let reveal = focus.was_in_view;
            self.restore_focus(focus);
            self.reveal_focus(tree, reveal)?;
        }
        Ok(())
    }

    fn active(&self) -> bool {
        // SAFETY: Activation and input focus are local to this UI thread.
        unsafe { GetActiveWindow() == self.window && IsWindowEnabled(self.window) != 0 }
    }

    fn tab_order(&self) -> Vec<HWND> {
        // SAFETY: The native dialog order descends into WS_EX_CONTROLPARENT composites.
        unsafe {
            let first = GetNextDlgTabItem(self.window, null_mut(), 0);
            let mut order = Vec::new();
            let mut next = first;
            while !next.is_null() && !order.contains(&next) {
                order.push(next);
                next = GetNextDlgTabItem(self.window, next, 0);
            }
            order
        }
    }

    fn focus_key(&self, mut handle: HWND) -> Option<Key> {
        while !handle.is_null() && handle != self.window {
            if let Some(&key) = self.handles.get(&(handle as usize)) {
                return Some(key);
            }
            // SAFETY: Walk live child ownership on the UI thread.
            handle = unsafe { GetAncestor(handle, GA_PARENT) };
        }
        None
    }

    fn focus_eligible(&self, mut handle: HWND) -> bool {
        // SAFETY: Descendants, including internal editor HWNDs, must be visible
        // and enabled through their entire native parent chain.
        unsafe {
            while !handle.is_null() && handle != self.window {
                if IsWindowEnabled(handle) == 0
                    || GetWindowLongPtrW(handle, GWL_STYLE) & WS_VISIBLE as isize == 0
                {
                    return false;
                }
                handle = GetAncestor(handle, GA_PARENT);
            }
        }
        handle == self.window
    }

    fn snapshot_focus(&self) -> Option<FocusSnapshot> {
        if !self.active() {
            return None;
        }
        // SAFETY: Read this thread's active child, including composite descendants.
        let handle = unsafe { GetFocus() };
        let key = self.focus_key(handle)?;
        let mut owners = Vec::new();
        let mut owner = self.views[&key].tab_owner;
        while let Some(key) = owner {
            owners.push(key);
            owner = self.views[&key].tab_owner;
        }
        let order: Vec<_> = self
            .tab_order()
            .into_iter()
            .filter_map(|handle| self.focus_key(handle))
            .collect();
        let mut nearby = Vec::new();
        if let Some(index) = order.iter().position(|&item| item == key) {
            for distance in 1..order.len() {
                nearby.extend(order.get(index + distance));
                nearby.extend(
                    index
                        .checked_sub(distance)
                        .and_then(|before| order.get(before)),
                );
            }
        }
        let was_in_view = self.focus_bounds(handle).is_some_and(|bounds| {
            bounds.top >= 0 && bounds.top < self.scroll_info(SIF_PAGE).nPage as i32
        });
        Some(FocusSnapshot {
            handle,
            was_in_view,
            key,
            owners,
            nearby,
        })
    }

    fn restore_focus(&self, focus: FocusSnapshot) {
        if !self.active() {
            return;
        }
        let handle = if self.focus_key(focus.handle) == Some(focus.key)
            && self.focus_eligible(focus.handle)
        {
            focus.handle
        } else {
            let order = self.tab_order();
            focus
                .owners
                .into_iter()
                .chain(focus.nearby)
                .find_map(|key| {
                    order
                        .iter()
                        .copied()
                        .find(|&item| self.focus_key(item) == Some(key))
                })
                .or_else(|| order.first().copied())
                .unwrap_or(self.window)
        };
        // SAFETY: The chosen replacement is live in the settled layout.
        unsafe {
            if GetFocus() != handle {
                SetFocus(handle);
            }
        }
    }

    pub(crate) fn reveal_focus(&mut self, tree: &Tree, force: bool) -> Result<()> {
        if !self.active() {
            self.last_focus = null_mut();
            return Ok(());
        }
        // SAFETY: Only query focus and geometry on the active surface's UI thread.
        let handle = unsafe { GetFocus() };
        if !force && handle == self.last_focus {
            return Ok(());
        }
        self.last_focus = handle;
        let Some(bounds) = self.focus_bounds(handle) else {
            return Ok(());
        };
        let available = self.scroll_info(SIF_PAGE).nPage as i32;
        let shift = if bounds.top < 0 || bounds.bottom - bounds.top > available {
            bounds.top
        } else {
            (bounds.bottom - available).max(0)
        };
        if shift != 0 {
            self.scroll_position += shift;
            self.arrange(tree)?;
        }
        Ok(())
    }

    fn focus_bounds(&self, handle: HWND) -> Option<RECT> {
        self.focus_key(handle)
            .filter(|_| self.focus_eligible(handle))?;
        let mut bounds = RECT::default();
        // SAFETY: The focus is a live descendant of this surface.
        unsafe {
            if GetWindowRect(handle, &mut bounds) == 0 {
                return None;
            }
            MapWindowPoints(
                null_mut(),
                self.window,
                (&mut bounds as *mut RECT).cast(),
                2,
            );
        }
        Some(bounds)
    }

    fn sync_node(&mut self, tree: &Tree, key: Key, tab_owner: Option<Key>) -> Result<()> {
        let node = tree.node(key);
        if let Kind::Element(tag) = &node.kind {
            if tag == "ui-dialog" {
                return Ok(());
            }
            if let Some(kind) = Control::from_tag(tag)? {
                if let std::collections::btree_map::Entry::Vacant(slot) = self.views.entry(key) {
                    let view = View::new(self.window, self.font, kind)?;
                    self.handles.insert(view.handle as usize, key);
                    slot.insert(view);
                }
                let caption = match kind {
                    Control::Panel => String::new(),
                    Control::Checkbox | Control::AggregateCheckbox => {
                        node.attr("title").unwrap_or("").into()
                    }
                    Control::Edit => node.attr("value").unwrap_or("").into(),
                    Control::RadioGroup
                    | Control::Tabs
                    | Control::TreeView
                    | Control::FileDropTarget => node.attr("label").unwrap_or("").into(),
                    Control::TextArea => canonical_lines(node.attr("value").unwrap_or("")),
                    Control::Image | Control::ShaderView | Control::Toolbar => {
                        node.attr("label").unwrap_or("").into()
                    }
                    _ => tree.caption(key),
                };
                let shader_inputs = (kind == Control::ShaderView).then(|| {
                    ShaderInputs::read(&self.assets, tree, key, |child| self.raster(child))
                });
                let view = self.views.get_mut(&key).unwrap();
                view.tab_owner = tab_owner;
                if kind == Control::Text {
                    let role = FontRole::read(node.attr("text-style").unwrap_or(""));
                    if role != view.font_role {
                        view.font_role = role;
                        view.font(self.fonts.get(role));
                    }
                }
                view.update(
                    self.window,
                    self.font,
                    Props {
                        caption,
                        civil: kind
                            .civil()
                            .map(|mode| CivilValue::parse(mode, node.attr("value").unwrap_or("")))
                            .transpose()?,
                        label: node.attr("label").unwrap_or("").into(),
                        placeholder: node.attr("placeholder").unwrap_or("").into(),
                        enabled: node.attr("enabled").is_none_or(enabled_flag),
                        check_state: if kind == Control::AggregateCheckbox {
                            match node.attr("check-state") {
                                Some("true") => BST_CHECKED,
                                Some("mixed") => BST_INDETERMINATE,
                                _ => BST_UNCHECKED,
                            }
                        } else {
                            u32::from(node.attr("checked").is_some_and(enabled_flag))
                        },
                        window_default: node.attr("window-default").is_some_and(enabled_flag),
                        command_icon: node.attr("command-icon").unwrap_or("").into(),
                        editable: node.attr("editable").is_some_and(enabled_flag),
                        ellipsize: node.attr("ellipsize-end").is_some_and(enabled_flag),
                    },
                )?;
                if matches!(kind, Control::Image | Control::ShaderView) || tag == "ui-status-text" {
                    let role = if tag == "ui-status-text" {
                        Role::Status {
                            polite: node.attr("announcement") == Some("polite"),
                        }
                    } else {
                        Role::Graphic {
                            included: node.attr("graphic-kind") != Some("decorative")
                                && node.attr("graphic-present") != Some("false")
                                && !view.props.caption.trim().is_empty(),
                        }
                    };
                    let semantic = Semantics {
                        name: view.props.caption.clone(),
                        details: node.attr("graphic-details").unwrap_or("").into(),
                        role,
                    };
                    if let Some(provider) = &view.semantics {
                        provider.update(semantic);
                    } else {
                        view.semantics = Some(ElementProvider::new(view.handle, semantic)?);
                    }
                }
                let keys = node.listens("keydown") || node.listens("keyup");
                if !view.input_attached && (keys || node.attr("pointer-zone").is_some()) {
                    surface_input::attach(view.handle, keys)?;
                    view.input_attached = true;
                }
                if kind == Control::Slider {
                    view.slider.sync(view.handle, node)?;
                }
                if let (Some(shader), Some(inputs)) = (&mut view.shader, shader_inputs) {
                    shader.set_inputs(inputs);
                    return Ok(());
                }
                if kind == Control::ProgressBar {
                    let requested = Progress::parse(node.attr("progress").unwrap_or("idle"))?;
                    view.progress
                        .update(view.handle, requested, self.preferences.animations);
                }
                if kind == Control::TreeView {
                    view.tree.sync(view.handle, tree, key)?;
                    return Ok(());
                }
                if kind.is_choice() {
                    view.choices.sync(view.handle, self.font, kind, tree, key)?;
                    if kind == Control::Tabs {
                        let panel = tab_panel(tree, node).ok_or("tab strip is missing its page")?;
                        self.sync_node(tree, panel, Some(key))?;
                    }
                    return Ok(());
                }
                if let Some(toolbar) = &mut view.toolbar {
                    return toolbar.sync(view.handle, tree, key, &self.assets, self.dpi);
                }
                if kind == Control::Button {
                    view.button_icon.sync(
                        view.handle,
                        &self.assets,
                        &view.props.command_icon,
                        self.dpi,
                    );
                }
                if let Some(grid) = &mut view.grid {
                    grid.mount_automation(view.handle, &view.props.label)?;
                    return grid.sync(
                        tree,
                        key,
                        view.handle,
                        self.font,
                        self.dpi,
                        view.props.enabled,
                    );
                }
                if let Some(report) = &mut view.list_view {
                    return report.sync(
                        view.handle,
                        self.font,
                        self.dpi,
                        Report::read(tree, key)?,
                        node.attr("selected"),
                    );
                }
            }
            for &child in &node.children {
                self.sync_node(tree, child, tab_owner)?;
            }
        }
        Ok(())
    }

    pub(crate) fn scroll(&mut self, tree: &Tree, request: u16, wheel: Option<i16>) -> Result<()> {
        let scroll = self.scroll_info(SIF_ALL);
        self.scroll_position = if let Some(wheel) = wheel {
            self.wheel_delta += i32::from(wheel);
            let lines = self.wheel_delta / 120;
            self.wheel_delta %= 120;
            self.scroll_position - lines * scale(self.dpi, 48)
        } else {
            let page = scroll.nPage as i32;
            let line = scale(self.dpi, 18);
            match i32::from(request) {
                SB_LINEUP => self.scroll_position - line,
                SB_LINEDOWN => self.scroll_position + line,
                SB_PAGEUP => self.scroll_position - page,
                SB_PAGEDOWN => self.scroll_position + page,
                SB_THUMBTRACK | SB_THUMBPOSITION => scroll.nTrackPos,
                SB_TOP => 0,
                SB_BOTTOM => scroll.nMax,
                _ => return Ok(()),
            }
        };
        self.arrange(tree)
    }

    fn scroll_info(&self, mask: u32) -> SCROLLINFO {
        let mut scroll = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: mask,
            ..SCROLLINFO::default()
        };
        // SAFETY: scroll is writable and its size accurately describes it.
        unsafe { GetScrollInfo(self.window, SB_VERT, &mut scroll) };
        scroll
    }

    pub(crate) fn arrange(&mut self, tree: &Tree) -> Result<()> {
        let mut placement = Vec::new();
        let mut client = RECT::default();
        // SAFETY: The client rectangle and app HWND are live.
        unsafe { GetClientRect(self.window, &mut client) };
        let reserved = match &mut self.status {
            Some(status) => status.arrange(
                client.right,
                client.bottom,
                self.dpi,
                self.preferences.text_scale,
            )?,
            None => 0,
        };
        let content_height = client.bottom - reserved;
        // SAFETY: The window owns the device context until ReleaseDC below.
        let dc = unsafe { GetDC(self.window) };
        if dc.is_null() {
            return Ok(());
        }
        // SAFETY: Re-select the original font before returning the DC.
        let previous = unsafe { SelectObject(dc, self.font) };
        let height = self
            .layout(
                tree,
                self.content,
                dc,
                client.right.max(1),
                0,
                0,
                &mut placement,
            )
            .1;
        unsafe {
            SelectObject(dc, previous);
            ReleaseDC(self.window, dc);
        }

        let height = height.max(1);
        self.scroll_position = self
            .scroll_position
            .clamp(0, (height - content_height).max(0));
        let scroll = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: height - 1,
            nPage: content_height.max(0) as u32,
            nPos: self.scroll_position,
            ..SCROLLINFO::default()
        };
        // SAFETY: The window owns its standard scrollbar.
        unsafe { SetScrollInfo(self.window, SB_VERT, &scroll, 1) };
        let mut after = self
            .status
            .as_ref()
            .map_or(HWND_TOP, |status| status.handle);
        for item in placement {
            let pixels = self.picture_pixels(tree, item.key);
            let view = self.views.get_mut(&item.key).unwrap();
            let y = item.y - self.scroll_position;
            let visible = view.kind != Control::ProgressBar || view.progress.visible();
            let mut label_height = 0;
            if item.label_height > 0 {
                label_height = item.label_height;
                let title = view.label_title();
                if window_text(view.label) != title {
                    set_text(view.label, &title);
                }
                position_visible(
                    view.label,
                    after,
                    item.x,
                    y,
                    item.width,
                    label_height,
                    visible || view.progress.steady(),
                );
                after = view.label;
            }
            if !view.label.is_null() && label_height == 0 {
                // SAFETY: The label belongs to this thread and is optional.
                unsafe { ShowWindow(view.label, SW_HIDE) };
            }
            let control_y = y + label_height;
            let control_height = item.height
                - label_height
                - if view.grid.is_some() {
                    item.detail_height
                } else {
                    0
                };
            if let Some(picture) = &mut view.picture {
                let contain = tree.node(item.key).attr("fit") == Some("contain");
                picture.update(pixels, contain, (item.width, control_height))?;
            }
            if let Some(shader) = &mut view.shader {
                shader.set_size(item.width, control_height);
            }
            if view.kind == Control::ComboBox {
                let dropdown_height = control_height + scale(self.dpi, 200);
                let placement = (after, item.x, control_y, item.width, dropdown_height);
                // Repositioning an already laid-out combo can close its dropdown.
                if view.combo_position != Some(placement) {
                    position(
                        view.handle,
                        after,
                        item.x,
                        control_y,
                        item.width,
                        dropdown_height,
                    );
                    view.combo_position = Some(placement);
                }
            } else {
                position_visible(
                    view.handle,
                    after,
                    item.x,
                    control_y,
                    item.width,
                    control_height,
                    visible,
                );
            }
            if view.kind == Control::RadioGroup {
                view.choices
                    .arrange(item.width, item.detail_height, scale(self.dpi, 12));
            }
            after = if let Some(grid) = &mut view.grid {
                grid.arrange(view.handle, self.window, self.font, self.dpi)?
            } else {
                view.handle
            };
        }
        Ok(())
    }

    fn picture_pixels(&self, tree: &Tree, key: Key) -> Option<Arc<Pixels>> {
        let node = tree.node(key);
        match &node.kind {
            Kind::Element(tag) if tag == "ui-asset-image" => self.raster(key),
            Kind::Element(tag) if tag == "ui-image" => {
                node.children.iter().find_map(|&child| self.raster(child))
            }
            _ => None,
        }
    }

    /// Measure and place from the same tree so the command order also sets
    /// the tab order. A measuring pass uses an empty output buffer.
    #[allow(clippy::too_many_arguments)]
    fn layout(
        &self,
        tree: &Tree,
        key: Key,
        dc: HDC,
        max_width: i32,
        x: i32,
        y: i32,
        placements: &mut Vec<Placement>,
    ) -> (i32, i32) {
        let node = tree.node(key);
        let Kind::Element(tag) = &node.kind else {
            return (0, 0);
        };
        if tag == "ui-dialog" {
            return (0, 0);
        }
        if let Some(view) = self.views.get(&key) {
            if view.kind == Control::Panel {
                let padding = scale(self.dpi, number(node, "padding", 12).clamp(0, 4096));
                let inner = (max_width - padding * 2).max(1);
                let (width, height) = node.children.first().map_or((0, 0), |&child| {
                    self.layout(tree, child, dc, inner, x + padding, y + padding, placements)
                });
                let width = if node.attr("fill").is_some_and(enabled_flag) {
                    max_width
                } else {
                    (width + padding * 2).min(max_width)
                };
                let height = height + padding * 2;
                // Panels are sibling HWNDs behind their children, in both painting and capture.
                placements.push(Placement {
                    key,
                    x,
                    y,
                    width,
                    height,
                    label_height: 0,
                    detail_height: 0,
                });
                return (width, height);
            }
            if view.kind == Control::FileDropTarget {
                let inset = scale(self.dpi, 12);
                let header = scale(self.dpi, 24).max(line_height(dc) + scale(self.dpi, 6));
                let inner = (max_width - inset * 2).max(1);
                let mut height = header;
                for &child in &node.children {
                    height += self
                        .layout(tree, child, dc, inner, x + inset, y + height, placements)
                        .1;
                }
                height += inset;
                // Paint the native frame behind the content HWNDs without
                // changing the tab order of the interactive children.
                placements.push(Placement {
                    key,
                    x,
                    y,
                    width: max_width,
                    height,
                    label_height: 0,
                    detail_height: 0,
                });
                return (max_width, height);
            }
            if view.kind == Control::Tabs
                && let Some(panel) = tab_panel(tree, node)
            {
                // The tab strip and page remain siblings, in keyboard order.
                // Size the strip to the header so it never covers the page.
                let mut page = RECT {
                    right: max_width,
                    bottom: scale(self.dpi, 256),
                    ..RECT::default()
                };
                // SAFETY: The tab is live and the native message fills this rectangle.
                unsafe {
                    SendMessageW(
                        view.handle,
                        TCM_ADJUSTRECT,
                        0,
                        &mut page as *mut _ as LPARAM,
                    );
                }
                let label_height = view.label_height(dc, self.dpi, max_width);
                let header = page.top.max(line_height(dc) + scale(self.dpi, 5));
                let inset = scale(self.dpi, 8);
                placements.push(Placement {
                    key,
                    x,
                    y,
                    width: max_width,
                    height: label_height + header,
                    label_height,
                    detail_height: 0,
                });
                let page_width = (page.right - page.left - inset * 2).max(1);
                let (_, page_height) = self.layout(
                    tree,
                    panel,
                    dc,
                    page_width,
                    x + page.left + inset,
                    y + label_height + header + inset,
                    placements,
                );
                return (max_width, label_height + header + page_height + inset * 2);
            }
            let previous = if view.kind == Control::Text {
                // SAFETY: Layout holds a live DC and the font belongs to this surface.
                unsafe { SelectObject(dc, self.fonts.get(view.font_role)) }
            } else {
                null_mut()
            };
            let label_height = view.label_height(dc, self.dpi, max_width);
            let detail_height = match view.kind {
                Control::RadioGroup => {
                    scale(self.dpi, 24).max(line_height(dc) + scale(self.dpi, 6))
                }
                Control::DataGrid => scale(self.dpi, 22).max(line_height(dc) + scale(self.dpi, 4)),
                _ => 0,
            };
            let (width, height) = natural(view, node, dc, self.dpi, max_width, detail_height);
            let height = height + label_height;
            if !previous.is_null() {
                unsafe { SelectObject(dc, previous) };
            }
            placements.push(Placement {
                key,
                x,
                y,
                width,
                height,
                label_height,
                detail_height,
            });
            return (width, height);
        }

        let padding = scale(self.dpi, number(node, "padding", 0));
        let default_spacing = if tag == "ui-tab-panel" { 0 } else { 8 };
        let spacing = scale(self.dpi, number(node, "spacing", default_spacing));
        let inner = (max_width - padding * 2).max(1);
        let children: Vec<Key> = node
            .children
            .iter()
            .copied()
            .filter(
                |&child| matches!(&tree.node(child).kind, Kind::Element(tag) if tag != "ui-dialog"),
            )
            .collect();
        let mut width = 0;
        let mut height = 0;
        let breakpoint = scale(self.dpi, number(node, "stack-below", 0).clamp(0, 4096));
        if tag == "ui-hstack" && max_width >= breakpoint {
            let count = i32::try_from(children.len()).unwrap_or(i32::MAX);
            let mut room = (inner - spacing.saturating_mul((count - 1).max(0))).max(1);
            // Place fixed-size controls before distributing the leftover
            // width to edit boxes, which benefit from stretching.
            let is_flex = |key| {
                tree.node(key).attr("fill").is_some_and(enabled_flag)
                    || self
                        .views
                        .get(&key)
                        .is_some_and(|view| view.kind.is_edit() || view.kind.civil().is_some())
            };
            let flex_count = children.iter().filter(|&&key| is_flex(key)).count() as i32;
            let preferred = children
                .iter()
                .map(|&child| {
                    let mut measured = Vec::new();
                    self.layout(tree, child, dc, inner, 0, 0, &mut measured).0
                })
                .collect::<Vec<_>>();
            let fixed: i32 = children
                .iter()
                .zip(&preferred)
                .filter(|&(&child, _)| !is_flex(child))
                .map(|(_, &width)| width)
                .fold(0, i32::saturating_add);
            if fixed.saturating_add(flex_count.saturating_mul(scale(self.dpi, 44))) > room {
                return self.layout_column(
                    tree, dc, &children, placements, inner, padding, spacing, x, y,
                );
            }
            let flex_width = if flex_count > 0 {
                ((room - fixed) / flex_count).max(scale(self.dpi, 44))
            } else {
                0
            };
            for (child, preferred) in children.into_iter().zip(preferred) {
                let available = if is_flex(child) {
                    flex_width
                } else {
                    preferred
                }
                .min(room)
                .max(1);
                let (used, child_height) = self.layout(
                    tree,
                    child,
                    dc,
                    available,
                    x + padding + width,
                    y + padding,
                    placements,
                );
                width += used + spacing;
                room = (room - used).max(1);
                height = height.max(child_height);
            }
            width = (width - spacing).max(0);
        } else {
            return self.layout_column(
                tree, dc, &children, placements, inner, padding, spacing, x, y,
            );
        }
        (width + padding * 2, height.saturating_add(padding * 2))
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_column(
        &self,
        tree: &Tree,
        dc: HDC,
        children: &[Key],
        placements: &mut Vec<Placement>,
        inner: i32,
        padding: i32,
        spacing: i32,
        x: i32,
        y: i32,
    ) -> (i32, i32) {
        let mut width = 0;
        let mut height: i32 = 0;
        for &child in children {
            let (child_width, child_height) = self.layout(
                tree,
                child,
                dc,
                inner,
                x + padding,
                y + padding + height,
                placements,
            );
            height = height.saturating_add(child_height + spacing);
            width = width.max(child_width);
        }
        (
            width + padding * 2,
            (height - spacing).max(0).saturating_add(padding * 2),
        )
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.views.clear();
        self.status = None;
    }
}

struct FocusSnapshot {
    handle: HWND,
    was_in_view: bool,
    key: Key,
    owners: Vec<Key>,
    nearby: Vec<Key>,
}

struct Placement {
    key: Key,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    label_height: i32,
    detail_height: i32,
}

fn tab_panel(tree: &Tree, node: &Node) -> Option<Key> {
    node.children.iter().copied().find(
        |&child| matches!(&tree.node(child).kind, Kind::Element(tag) if tag == "ui-tab-panel"),
    )
}

pub(crate) fn scale(dpi: u32, value: i32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn number(node: &Node, key: &str, fallback: i32) -> i32 {
    node.attr(key)
        .and_then(|text| text.parse().ok())
        .unwrap_or(fallback)
        .clamp(0, 4096)
}

fn natural(
    view: &View,
    node: &Node,
    dc: HDC,
    dpi: u32,
    max_width: i32,
    detail_height: i32,
) -> (i32, i32) {
    let dip = |value| scale(dpi, value);
    let single_line = dip(30).max(line_height(dc) + dip(10));
    let (width, height) = match view.kind {
        Control::Text if view.props.ellipsize => (max_width, line_height(dc)),
        Control::Text => (
            max_width,
            wrap_height(dc, &view.props.caption, max_width).max(dip(20)),
        ),
        Control::Button | Control::FileButton | Control::FolderButton => (
            (text_width(dc, &view.props.caption) + dip(30) + view.button_icon.width(dpi))
                .max(dip(80)),
            single_line,
        ),
        Control::Checkbox | Control::AggregateCheckbox => {
            (text_width(dc, &view.props.caption) + dip(26), single_line)
        }
        Control::Edit | Control::DatePicker | Control::TimePicker | Control::ComboBox => {
            (max_width, single_line)
        }
        Control::Slider => (max_width, dip(34)),
        Control::ProgressBar => (
            max_width,
            if view.progress.steady() {
                0
            } else {
                dip(number(node, "height", 16).max(1))
            },
        ),
        Control::ListBox => {
            // SAFETY: The HWND is a live fixed-height native list box.
            let row_height = unsafe { SendMessageW(view.handle, LB_GETITEMHEIGHT, 0, 0) };
            let row_height = i32::try_from(row_height)
                .ok()
                .filter(|&height| height > 0)
                .unwrap_or(dip(20));
            let rows = number(node, "visible-rows", 6).clamp(1, 20);
            (max_width, rows * row_height + dip(4))
        }
        Control::RadioGroup => (
            max_width,
            detail_height * (view.choices.count() as i32 + 1) + dip(8),
        ),
        Control::TextArea => (
            max_width,
            line_height(dc) * number(node, "rows", 5).clamp(1, 100) + dip(8),
        ),
        Control::Toolbar => (
            max_width,
            view.toolbar
                .as_ref()
                .unwrap()
                .height(view.handle, max_width, dpi),
        ),
        Control::ListView | Control::DataGrid => (
            max_width,
            view.list_view
                .as_ref()
                .or_else(|| view.grid.as_ref().map(|grid| &grid.report))
                .unwrap()
                .height(view.handle, number(node, "visible-rows", 8), dpi)
                + detail_height,
        ),
        Control::Tabs | Control::Panel | Control::FileDropTarget => {
            unreachable!("containers lay out their content separately")
        }
        Control::TreeView => (
            max_width,
            // SAFETY: This measures a live native tree using the current UI font.
            (unsafe { SendMessageW(view.handle, TVM_GETITEMHEIGHT, 0, 0) } as i32).max(dip(16))
                * number(node, "visible-rows", 8).clamp(1, 100)
                + dip(4),
        ),
        Control::Image | Control::ShaderView => (
            dip(number(node, "width", 128)).max(1),
            dip(number(node, "height", 96)).max(1),
        ),
    };
    (width.min(max_width), height)
}

thread_local! {
    static COMPOSING: RefCell<BTreeSet<usize>> = const { RefCell::new(BTreeSet::new()) };
}

pub(crate) fn composing(hwnd: HWND) -> bool {
    COMPOSING.with(|active| active.borrow().contains(&(hwnd as usize)))
}

fn edit_is_active(hwnd: HWND) -> bool {
    // SAFETY: Focus belongs to the UI thread that owns the edit.
    (unsafe { GetFocus() == hwnd }) || composing(hwnd)
}

fn canonical_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn native_lines(text: &str) -> String {
    text.replace('\n', "\r\n")
}

unsafe extern "system" fn edit_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    multiline: usize,
) -> LRESULT {
    match message {
        WM_IME_STARTCOMPOSITION => COMPOSING.with(|active| {
            active.borrow_mut().insert(hwnd as usize);
        }),
        WM_IME_ENDCOMPOSITION | WM_NCDESTROY => COMPOSING.with(|active| {
            active.borrow_mut().remove(&(hwnd as usize));
        }),
        _ => {}
    }
    // SAFETY: This is the installed same-thread subclass and arguments are unmodified.
    unsafe {
        if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(edit_subclass), id);
        }
        let result = DefSubclassProc(hwnd, message, wparam, lparam);
        if multiline != 0 && matches!(message, WM_KILLFOCUS | WM_IME_ENDCOMPOSITION) {
            PostMessageW(GetParent(hwnd), APPLY_PENDING_EDIT, 0, hwnd as LPARAM);
        }
        result
    }
}

pub(crate) fn child(parent: HWND, font: HFONT, ex: u32, class: PCWSTR, style: u32) -> Result<HWND> {
    // SAFETY: The parent lives on our thread and the system class is an
    // internally selected, terminated string.
    unsafe {
        let hwnd = CreateWindowExW(
            ex,
            class,
            w!(""),
            WS_CHILD | WS_CLIPSIBLINGS | style,
            0,
            0,
            0,
            0,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            return Err(format!(
                "cannot create Windows control: {}",
                std::io::Error::last_os_error()
            ));
        }
        SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
        Ok(hwnd)
    }
}

pub(crate) fn position(hwnd: HWND, after: HWND, x: i32, y: i32, width: i32, height: i32) {
    position_visible(hwnd, after, x, y, width, height, true);
}

fn position_visible(
    hwnd: HWND,
    after: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    visible: bool,
) {
    let visibility = if visible {
        SWP_SHOWWINDOW
    } else {
        SWP_HIDEWINDOW
    };
    // SAFETY: The target and its preceding sibling, if any, are live HWNDs.
    unsafe {
        SetWindowPos(
            hwnd,
            after,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | visibility,
        );
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .map(|word| if word == 0 { 0xfffd } else { word })
        .chain([0])
        .collect()
}

pub(crate) fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: SetWindowTextW reads the terminated string before returning.
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

pub(crate) fn window_text(hwnd: HWND) -> String {
    // SAFETY: The child is on this thread. The count includes space for
    // Win32 to terminate the string.
    unsafe {
        let cap = GetWindowTextLengthW(hwnd) + 1;
        let mut buffer = vec![0_u16; cap as usize];
        let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), cap);
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

pub(crate) fn text_width(dc: HDC, text: &str) -> i32 {
    let text = wide(text);
    let mut size = SIZE::default();
    // SAFETY: Text length excludes only the terminator, and size is writable.
    unsafe { GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size) };
    size.cx
}

fn line_height(dc: HDC) -> i32 {
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: The DC contains the control font and metrics is writable.
    unsafe { GetTextMetricsW(dc, &mut metrics) };
    metrics.tmHeight.max(1)
}

fn wrap_height(dc: HDC, text: &str, width: i32) -> i32 {
    let text = wide(text);
    let mut rect = RECT {
        right: width,
        ..RECT::default()
    };
    // SAFETY: The flags do not modify the text. The rect is writable.
    unsafe {
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
        )
    };
    rect.bottom
}

#[cfg(test)]
mod tests {
    use super::{canonical_lines, native_lines};

    #[test]
    fn multiline_values_preserve_spacing_and_empty_lines() {
        let draft = "first\n  second\n\n";
        assert_eq!(native_lines(draft), "first\r\n  second\r\n\r\n");
        assert_eq!(canonical_lines(&native_lines(draft)), draft);
        assert_eq!(
            canonical_lines("first\rsecond\r\nthird"),
            "first\nsecond\nthird"
        );
    }
}
