use crate::Result;
use crate::window::enabled_flag;
use crate::wire::{Key, Kind, Node, ROOT, Tree};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_BITMAP, SS_CENTERIMAGE, SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::{BST_CHECKED, EM_SETCUEBANNER, EM_SETSEL, SetScrollInfo};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Text,
    Button,
    FileButton,
    Checkbox,
    Edit,
    Image,
}

impl Control {
    fn from_tag(tag: &str) -> Result<Option<Self>> {
        let control = match tag {
            "" | "ui-vstack" | "ui-hstack" => return Ok(None),
            "ui-text" => Self::Text,
            "ui-button" => Self::Button,
            "ui-file-button" => Self::FileButton,
            "ui-checkbox" => Self::Checkbox,
            "ui-text-field" => Self::Edit,
            "ui-asset-image" => Self::Image,
            _ => return Err(format!("unsupported native control {tag}")),
        };
        Ok(Some(control))
    }

    fn class(self) -> (u32, PCWSTR, u32) {
        match self {
            Self::Text => (0, w!("STATIC"), SS_LEFT | SS_NOPREFIX),
            Self::Button | Self::FileButton => (0, w!("BUTTON"), WS_TABSTOP | BS_PUSHBUTTON as u32),
            Self::Checkbox => (0, w!("BUTTON"), WS_TABSTOP | BS_AUTOCHECKBOX as u32),
            Self::Edit => (
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
            ),
            Self::Image => (0, w!("STATIC"), SS_BITMAP | SS_CENTERIMAGE),
        }
    }
}

struct View {
    handle: HWND,
    label: HWND,
    kind: Control,
    props: Props,
    image: HBITMAP,
    rendered_image: Option<(String, i32, i32)>,
}

#[derive(Default)]
struct Props {
    caption: String,
    label: String,
    placeholder: String,
    enabled: bool,
    checked: bool,
}

impl View {
    fn new(window: HWND, font: HFONT, kind: Control) -> Result<Self> {
        let (ex, class, style) = kind.class();
        let handle = child(window, font, ex, class, style)?;
        Ok(Self {
            handle,
            label: null_mut(),
            kind,
            props: Props {
                enabled: true,
                ..Props::default()
            },
            image: null_mut(),
            rendered_image: None,
        })
    }

    fn font(&self, font: HFONT) {
        // SAFETY: Both child windows, when present, are owned by this view.
        unsafe {
            SendMessageW(self.handle, WM_SETFONT, font as usize, 0);
            if !self.label.is_null() {
                SendMessageW(self.label, WM_SETFONT, font as usize, 0);
            }
        }
    }

    fn update(&mut self, window: HWND, font: HFONT, desired: Props) -> Result<()> {
        // Compare the controlled edit value with what's on screen before
        // writing. Writing the same string can reset the selection and IME.
        if self.props.caption != desired.caption
            && (self.kind != Control::Edit || window_text(self.handle) != desired.caption)
        {
            set_text(self.handle, &desired.caption);
        }
        if self.kind == Control::Edit && self.props.label != desired.label {
            if self.label.is_null() && !desired.label.is_empty() {
                self.label = child(window, font, 0, w!("STATIC"), SS_LEFT | SS_NOPREFIX)?;
            }
            if !self.label.is_null() {
                set_text(self.label, &desired.label);
            }
        }

        // SAFETY: WM_ENABLE, BM_SETCHECK and EM_SETCUEBANNER operate on this
        // view's live window on its creating thread. The string is retained
        // until the synchronous cue-banner message returns.
        unsafe {
            if self.props.enabled != desired.enabled {
                EnableWindow(self.handle, i32::from(desired.enabled));
            }
            if self.kind == Control::Checkbox && self.props.checked != desired.checked {
                SendMessageW(self.handle, BM_SETCHECK, usize::from(desired.checked), 0);
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
        self.props = desired;
        Ok(())
    }

    fn update_bitmap(&mut self, root: &Path, name: &str, width: i32, height: i32) -> Result<()> {
        if name.is_empty() {
            self.clear_bitmap();
            return Ok(());
        }
        if self
            .rendered_image
            .as_ref()
            .is_some_and(|current| current.0 == name && current.1 == width && current.2 == height)
        {
            return Ok(());
        }
        let path = resolve_bitmap(root, name)?;
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: The path is a live terminated Windows filename. Without
        // LR_SHARED, this view owns the returned bitmap handle.
        let image = unsafe {
            LoadImageW(
                null_mut(),
                wide_path.as_ptr(),
                IMAGE_BITMAP,
                width,
                height,
                LR_LOADFROMFILE,
            )
        };
        if image.is_null() {
            return Err(format!(
                "cannot load bitmap {name}: {}",
                std::io::Error::last_os_error()
            ));
        }

        self.clear_bitmap();
        // SAFETY: The bitmap stays live until we remove it from the control.
        unsafe {
            SendMessageW(
                self.handle,
                STM_SETIMAGE,
                IMAGE_BITMAP as usize,
                image as isize,
            )
        };
        self.image = image;
        self.rendered_image = Some((name.into(), width, height));
        Ok(())
    }

    fn clear_bitmap(&mut self) {
        if self.image.is_null() {
            return;
        }
        // SAFETY: First remove the bitmap from the static control. User32 may
        // have copied it, in which case the returned copy is also ours.
        unsafe {
            let displayed =
                SendMessageW(self.handle, STM_SETIMAGE, IMAGE_BITMAP as usize, 0) as HBITMAP;
            if !displayed.is_null() && displayed != self.image {
                DeleteObject(displayed);
            }
            DeleteObject(self.image);
        }
        self.image = null_mut();
        self.rendered_image = None;
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.clear_bitmap();
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
    font: HFONT,
    dpi: u32,
    assets: PathBuf,
    views: BTreeMap<Key, View>,
    handles: BTreeMap<usize, Key>,
    scroll_position: i32,
    wheel_delta: i32,
}

impl Surface {
    pub(crate) fn new(window: HWND) -> Result<Self> {
        // SAFETY: The HWND has been created and lives on this thread.
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let assets = std::env::current_exe()
            .map_err(|error| format!("cannot locate app assets: {error}"))?
            .with_file_name("assets");
        Ok(Self {
            window,
            font: make_font(dpi)?,
            dpi,
            assets,
            views: BTreeMap::new(),
            handles: BTreeMap::new(),
            scroll_position: 0,
            wheel_delta: 0,
        })
    }

    pub(crate) fn title(&self, title: &str) {
        set_text(self.window, title);
    }

    pub(crate) fn key(&self, handle: HWND) -> Option<Key> {
        self.handles.get(&(handle as usize)).copied()
    }

    pub(crate) fn kind(&self, key: Key) -> Control {
        self.views[&key].kind
    }

    pub(crate) fn input(&self, key: Key) -> String {
        let view = &self.views[&key];
        if view.kind == Control::Edit {
            return window_text(view.handle);
        }
        // SAFETY: The child is a live BUTTON with auto-checkbox semantics.
        if unsafe { SendMessageW(view.handle, BM_GETCHECK, 0, 0) } == BST_CHECKED as isize {
            "true".into()
        } else {
            "false".into()
        }
    }

    pub(crate) fn spec_handle(&self, key: Key) -> Option<HWND> {
        self.views.get(&key).map(|view| view.handle)
    }

    pub(crate) fn spec_label(&self, key: Key, kind: &str) -> Option<String> {
        let view = self.views.get(&key)?;
        if kind == "text-field" {
            return Some(if view.label.is_null() {
                view.props.placeholder.clone()
            } else {
                window_text(view.label)
            });
        }
        if kind == "image" {
            return Some(view.props.label.clone());
        }
        Some(window_text(view.handle))
    }

    pub(crate) fn spec_text(&self, key: Key) -> Result<String> {
        let handle = self
            .spec_handle(key)
            .ok_or("locator is not a native control")?;
        Ok(window_text(handle))
    }

    pub(crate) fn spec_disabled(&self, key: Key) -> Result<bool> {
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
            .filter(|view| view.kind == Control::Edit)
            .ok_or("fill requires a text field")?;
        set_text(view.handle, text);
        Ok(view.handle)
    }

    pub(crate) fn spec_toggle(&self, key: Key) -> Result<HWND> {
        let view = self
            .views
            .get(&key)
            .filter(|view| view.kind == Control::Checkbox)
            .ok_or("toggle requires a checkbox")?;
        let checked = self.input(key) == "true";
        // SAFETY: The HWND is a live checkbox on the UI thread.
        unsafe { SendMessageW(view.handle, BM_SETCHECK, usize::from(!checked), 0) };
        Ok(view.handle)
    }

    pub(crate) fn restore_controlled_value(&self, key: Key) {
        let Some(view) = self.views.get(&key) else {
            return;
        };
        match view.kind {
            Control::Edit if window_text(view.handle) != view.props.caption => {
                set_text(view.handle, &view.props.caption);
            }
            Control::Checkbox if (self.input(key) == "true") != view.props.checked => {
                // SAFETY: The handle is a live check box owned by this thread.
                unsafe {
                    SendMessageW(view.handle, BM_SETCHECK, usize::from(view.props.checked), 0)
                };
            }
            _ => {}
        }
    }

    pub(crate) fn reference(&self, op: u8, key: Key) {
        let Some(view) = self.views.get(&key) else {
            return;
        };
        // SAFETY: Both root and child live on the app's UI thread.
        unsafe {
            match op {
                1 | 3 | 5 => {
                    SetFocus(view.handle);
                    if op == 3 && view.kind == Control::Edit {
                        SendMessageW(view.handle, EM_SETSEL, 0, -1);
                    }
                }
                2 => {
                    SetFocus(self.window);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn dpi_changed(&mut self, tree: &Tree) -> Result<()> {
        // SAFETY: WM_DPICHANGED names our live root HWND.
        let dpi = unsafe { GetDpiForWindow(self.window) }.max(96);
        let font = make_font(dpi)?;
        for view in self.views.values() {
            view.font(font);
        }
        // SAFETY: All windows have selected the new font.
        unsafe { DeleteObject(self.font) };
        self.font = font;
        self.scroll_position =
            ((i64::from(self.scroll_position) * i64::from(dpi)) / i64::from(self.dpi)) as i32;
        self.dpi = dpi;
        self.arrange(tree)
    }

    pub(crate) fn sync(&mut self, tree: &Tree) -> Result<()> {
        self.views.retain(|key, view| {
            if tree.contains(*key) {
                true
            } else {
                self.handles.remove(&(view.handle as usize));
                false
            }
        });
        self.sync_node(tree, ROOT)?;
        self.arrange(tree)
    }

    fn sync_node(&mut self, tree: &Tree, key: Key) -> Result<()> {
        let node = tree.node(key);
        if let Kind::Element(tag) = &node.kind {
            if let Some(kind) = Control::from_tag(tag)? {
                if let std::collections::btree_map::Entry::Vacant(slot) = self.views.entry(key) {
                    let view = View::new(self.window, self.font, kind)?;
                    self.handles.insert(view.handle as usize, key);
                    slot.insert(view);
                }
                let caption = match kind {
                    Control::Checkbox => node.attr("title").unwrap_or("").into(),
                    Control::Edit => node.attr("value").unwrap_or("").into(),
                    Control::Image => node.attr("label").unwrap_or("").into(),
                    _ => tree.caption(key),
                };
                self.views.get_mut(&key).unwrap().update(
                    self.window,
                    self.font,
                    Props {
                        caption,
                        label: node.attr("label").unwrap_or("").into(),
                        placeholder: node.attr("placeholder").unwrap_or("").into(),
                        enabled: node.attr("enabled").is_none_or(enabled_flag),
                        checked: node.attr("checked").is_some_and(enabled_flag),
                    },
                )?;
            }
            for &child in &node.children {
                self.sync_node(tree, child)?;
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
        // SAFETY: These calls use the live app HWND and its selected font.
        let dc = unsafe {
            GetClientRect(self.window, &mut client);
            GetDC(self.window)
        };
        if dc.is_null() {
            return Ok(());
        }
        // SAFETY: Re-select the original font before returning the DC.
        let previous = unsafe { SelectObject(dc, self.font) };
        let height = self
            .layout(tree, ROOT, dc, client.right.max(1), 0, 0, &mut placement)
            .1;
        unsafe {
            SelectObject(dc, previous);
            ReleaseDC(self.window, dc);
        }

        let height = height.max(1);
        self.scroll_position = self
            .scroll_position
            .clamp(0, (height - client.bottom).max(0));
        let scroll = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: height - 1,
            nPage: client.bottom.max(0) as u32,
            nPos: self.scroll_position,
            ..SCROLLINFO::default()
        };
        // SAFETY: The window owns its standard scrollbar.
        unsafe { SetScrollInfo(self.window, SB_VERT, &scroll, 1) };
        let mut after = HWND_TOP;
        for item in placement {
            let view = self.views.get_mut(&item.key).unwrap();
            let y = item.y - self.scroll_position;
            let mut label_height = 0;
            if !view.label.is_null() && !view.props.label.is_empty() {
                label_height = scale(self.dpi, 21);
                position(view.label, after, item.x, y, item.width, label_height);
                after = view.label;
            }
            if view.kind == Control::Image {
                let name = tree.node(item.key).attr("asset").unwrap_or("");
                view.update_bitmap(&self.assets, name, item.width, item.height)?;
            }
            if !view.label.is_null() && view.props.label.is_empty() {
                // SAFETY: The label belongs to this thread and is optional.
                unsafe { ShowWindow(view.label, SW_HIDE) };
            }
            position(
                view.handle,
                after,
                item.x,
                y + label_height,
                item.width,
                item.height - label_height,
            );
            after = view.handle;
        }
        Ok(())
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
        if let Some(view) = self.views.get(&key) {
            let (width, height) = natural(view, node, dc, self.dpi, max_width);
            placements.push(Placement {
                key,
                x,
                y,
                width,
                height,
            });
            return (width, height);
        }

        let padding = scale(self.dpi, number(node, "padding", 0));
        let spacing = scale(self.dpi, number(node, "spacing", 8));
        let inner = (max_width - padding * 2).max(1);
        let children: Vec<Key> = node
            .children
            .iter()
            .copied()
            .filter(|&child| matches!(tree.node(child).kind, Kind::Element(_)))
            .collect();
        let mut width = 0;
        let mut height = 0;
        if tag == "ui-hstack" {
            let count = i32::try_from(children.len()).unwrap_or(i32::MAX);
            let mut room = (inner - spacing.saturating_mul((count - 1).max(0))).max(1);
            // Place fixed-size controls before distributing the leftover
            // width to edit boxes, which benefit from stretching.
            let is_flex = |key| {
                self.views
                    .get(&key)
                    .is_some_and(|view| view.kind == Control::Edit)
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
            for child in children {
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
            height = (height - spacing).max(0);
        }
        (width + padding * 2, height.saturating_add(padding * 2))
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.views.clear();
        // SAFETY: All children using the font have been destroyed.
        unsafe { DeleteObject(self.font) };
    }
}

struct Placement {
    key: Key,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn scale(dpi: u32, value: i32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn number(node: &Node, key: &str, fallback: i32) -> i32 {
    node.attr(key)
        .and_then(|text| text.parse().ok())
        .unwrap_or(fallback)
        .clamp(0, 4096)
}

fn natural(view: &View, node: &Node, dc: HDC, dpi: u32, max_width: i32) -> (i32, i32) {
    let dip = |value| scale(dpi, value);
    let (width, height) = match view.kind {
        Control::Text => (
            max_width,
            wrap_height(dc, &view.props.caption, max_width).max(dip(20)),
        ),
        Control::Button | Control::FileButton => (
            (text_width(dc, &view.props.caption) + dip(30)).max(dip(80)),
            dip(30),
        ),
        Control::Checkbox => (text_width(dc, &view.props.caption) + dip(26), dip(30)),
        Control::Edit => (
            max_width,
            dip(if view.props.label.is_empty() { 30 } else { 51 }),
        ),
        Control::Image => (
            dip(number(node, "width", 128)).max(1),
            dip(number(node, "height", 96)).max(1),
        ),
    };
    (width.min(max_width), height)
}

fn child(parent: HWND, font: HFONT, ex: u32, class: PCWSTR, style: u32) -> Result<HWND> {
    // SAFETY: The parent lives on our thread and the system class is an
    // internally selected, terminated string.
    unsafe {
        let hwnd = CreateWindowExW(
            ex,
            class,
            w!(""),
            WS_CHILD | style,
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

fn position(hwnd: HWND, after: HWND, x: i32, y: i32, width: i32, height: i32) {
    // SAFETY: The target and its preceding sibling, if any, are live HWNDs.
    unsafe {
        SetWindowPos(
            hwnd,
            after,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn make_font(dpi: u32) -> Result<HFONT> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..NONCLIENTMETRICSW::default()
    };
    // SAFETY: The metrics pointer has the size indicated by cbSize.
    unsafe {
        if SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
            0,
            dpi,
        ) == 0
        {
            return Err(format!(
                "cannot read Windows message font: {}",
                std::io::Error::last_os_error()
            ));
        }
        let font = CreateFontIndirectW(&metrics.lfMessageFont);
        if font.is_null() {
            return Err(format!(
                "cannot create Windows message font: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(font)
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .map(|word| if word == 0 { 0xfffd } else { word })
        .chain([0])
        .collect()
}

fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: SetWindowTextW reads the terminated string before returning.
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

fn window_text(hwnd: HWND) -> String {
    // SAFETY: The child is on this thread. The count includes space for
    // Win32 to terminate the string.
    unsafe {
        let cap = GetWindowTextLengthW(hwnd) + 1;
        let mut buffer = vec![0_u16; cap as usize];
        let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), cap);
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

fn text_width(dc: HDC, text: &str) -> i32 {
    let text = wide(text);
    let mut size = SIZE::default();
    // SAFETY: Text length excludes only the terminator, and size is writable.
    unsafe { GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size) };
    size.cx
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

fn resolve_bitmap(root: &Path, name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || !path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bmp"))
    {
        return Err(format!("asset must be a BMP path under assets: {name}"));
    }
    let root = root
        .canonicalize()
        .map_err(|err| format!("cannot find assets directory: {err}"))?;
    let path = root
        .join(path)
        .canonicalize()
        .map_err(|err| format!("cannot find asset {name}: {err}"))?;
    if !path.starts_with(&root) {
        return Err(format!("asset path leaves the assets directory: {name}"));
    }
    Ok(path)
}
