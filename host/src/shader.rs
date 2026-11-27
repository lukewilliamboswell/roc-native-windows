mod d3d;
mod worker;

use crate::Result;
use crate::picture::Picture;
use crate::raster::Pixels;
use crate::view::wide;
use crate::wire::{Key, Kind, Tree};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::path::PathBuf;
use std::ptr::null;
use std::sync::Arc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use worker::Worker;

const SUBCLASS: usize = 0x5348_4452;

thread_local! {
    static RENDERER: RefCell<Option<Worker>> = const { RefCell::new(None) };
    static NEXT_SURFACE: Cell<u64> = const { Cell::new(1) };
    static DRIVEN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn configure(driven: bool) {
    DRIVEN.set(driven);
}

pub(crate) fn ready() -> bool {
    RENDERER.with_borrow(|worker| worker.as_ref().is_none_or(Worker::idle))
}

pub(crate) fn shutdown() {
    let worker = RENDERER.with_borrow_mut(Option::take);
    drop(worker);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameSize {
    width: u32,
    height: u32,
}

impl FrameSize {
    fn checked(width: i32, height: i32) -> Result<Self> {
        let width = u32::try_from(width).map_err(|_| "negative shader view width")?;
        let height = u32::try_from(height).map_err(|_| "negative shader view height")?;
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || u64::from(width) * u64::from(height) > 8_388_608
        {
            return Err("shader view frame must fit 4096 pixels per edge and 8 Mi pixels".into());
        }
        Ok(Self { width, height })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sampling {
    Nearest,
    Linear,
}

#[derive(Clone)]
enum Source {
    NoTexture,
    Waiting,
    Asset(String),
    Raster(Arc<Pixels>),
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NoTexture, Self::NoTexture) | (Self::Waiting, Self::Waiting) => true,
            (Self::Asset(left), Self::Asset(right)) => left == right,
            (Self::Raster(left), Self::Raster(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct Inputs {
    assets: PathBuf,
    shader: String,
    parameters: String,
    sampling: Sampling,
    sources: [Source; 2],
}

impl Inputs {
    pub(crate) fn read(
        assets: &Path,
        tree: &Tree,
        key: Key,
        raster: impl Fn(Key) -> Option<Arc<Pixels>>,
    ) -> Self {
        let node = tree.node(key);
        let sources = std::array::from_fn(|slot| {
            let attribute = if slot == 0 {
                "texture"
            } else {
                "second-texture"
            };
            match node.attr(attribute).unwrap_or("") {
                "raster" => node
                    .children
                    .iter()
                    .copied()
                    .find(|&child| {
                        let child = tree.node(child);
                        matches!(&child.kind, Kind::Element(tag) if tag == "ui-shader-raster")
                            && child.attr("slot") == Some(if slot == 0 { "0" } else { "1" })
                    })
                    .and_then(&raster)
                    .map_or(Source::Waiting, Source::Raster),
                encoded => encoded
                    .strip_prefix("asset:")
                    .map_or(Source::NoTexture, |path| Source::Asset(path.into())),
            }
        });
        Self {
            assets: assets.to_owned(),
            shader: node.attr("shader").unwrap_or("").into(),
            parameters: node.attr("parameters").unwrap_or("").into(),
            sampling: if node.attr("sampling") == Some("nearest") {
                Sampling::Nearest
            } else {
                Sampling::Linear
            },
            sources,
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    revision: u64,
    size: FrameSize,
    inputs: Inputs,
}

impl std::ops::Deref for Snapshot {
    type Target = Inputs;
    fn deref(&self) -> &Inputs {
        &self.inputs
    }
}

#[derive(Default)]
enum Display {
    #[default]
    Pending,
    Rendered(Arc<Pixels>),
    Failed(String, Option<Arc<Pixels>>),
}

impl Display {
    fn status(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Rendered(_) => "rendered",
            Self::Failed(_, Some(_)) => "fallback",
            Self::Failed(_, None) => "error",
        }
    }

    fn pixels(&self) -> Option<&Arc<Pixels>> {
        match self {
            Self::Rendered(pixels) | Self::Failed(_, Some(pixels)) => Some(pixels),
            _ => None,
        }
    }
}

pub(crate) struct ShaderView {
    hwnd: HWND,
    id: u64,
    display: Box<RefCell<Display>>,
    picture: Picture,
    wanted: Option<Inputs>,
    size: Option<FrameSize>,
    revision: u64,
    sent: Option<(FrameSize, Inputs)>,
}

impl ShaderView {
    pub(crate) fn attach(hwnd: HWND) -> Result<Self> {
        let id = NEXT_SURFACE.with(|next| {
            let current = next.get();
            next.set(
                current
                    .checked_add(1)
                    .ok_or("shader surface identifiers exhausted")?,
            );
            Ok::<_, String>(current)
        })?;
        let picture = Picture::new(hwnd)?;
        let display = Box::new(RefCell::new(Display::Pending));
        // SAFETY: Box keeps the paint data stable until this HWND stops
        // using the subclass. No worker accesses this pointer.
        let address = std::ptr::from_ref(display.as_ref()) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(paint), SUBCLASS, address) } == 0 {
            return Err("could not install the shader view renderer".into());
        }
        Ok(Self {
            hwnd,
            id,
            display,
            picture,
            wanted: None,
            size: None,
            revision: 0,
            sent: None,
        })
    }

    pub(crate) fn set_inputs(&mut self, wanted: Inputs) {
        if self
            .wanted
            .as_ref()
            .is_some_and(|previous| previous.sources != wanted.sources)
        {
            self.publish(Display::Pending);
        }
        self.wanted = Some(wanted);
    }

    pub(crate) fn set_background(&mut self, color: u32) {
        self.picture.set_background(color);
    }

    pub(crate) fn set_size(&mut self, width: i32, height: i32) {
        if width == 0 || height == 0 {
            self.size = None;
            return;
        }
        match FrameSize::checked(width, height) {
            Ok(size) => self.size = Some(size),
            Err(error) => {
                self.size = None;
                self.publish(Display::Failed(error, None));
            }
        }
    }

    pub(crate) fn poll(&mut self, root: HWND) {
        let visible =
            DRIVEN.get() || unsafe { IsWindowVisible(self.hwnd) != 0 && IsIconic(root) == 0 };
        if !visible || self.size.is_none() {
            if self.sent.take().is_some() {
                self.retire();
            }
            return;
        }
        let (Some(size), Some(inputs)) = (self.size, self.wanted.as_ref()) else {
            return;
        };
        let pending = (size, inputs.clone());
        if self.sent.as_ref() != Some(&pending) {
            self.revision = self.revision.wrapping_add(1);
            let request = Snapshot {
                revision: self.revision,
                size,
                inputs: inputs.clone(),
            };
            let result = RENDERER.with_borrow_mut(|worker| {
                if worker.is_none() {
                    *worker = Some(Worker::start()?);
                }
                worker.as_ref().unwrap().submit(self.id, request);
                Ok::<_, String>(())
            });
            self.sent = Some(pending);
            if let Err(error) = result {
                self.publish(Display::Failed(error, None));
            }
        }
        let frame =
            RENDERER.with_borrow(|worker| worker.as_ref().and_then(|worker| worker.frame(self.id)));
        if let Some((revision, display)) = frame
            && revision == self.revision
        {
            self.publish(display);
        }
    }

    pub(crate) fn status(&self) -> &'static str {
        self.display.borrow().status()
    }

    pub(crate) fn pixel(&self, x: u32, y: u32) -> Result<[u8; 3]> {
        let display = self.display.borrow();
        let frame = display
            .pixels()
            .ok_or("shader frame has not been committed")?;
        if x >= frame.width || y >= frame.height {
            return Err("shader pixel is outside the committed frame".into());
        }
        let index = (y as usize * frame.width as usize + x as usize) * 4;
        Ok([
            frame.bgra[index + 2],
            frame.bgra[index + 1],
            frame.bgra[index],
        ])
    }

    fn retire(&self) {
        RENDERER.with_borrow(|worker| {
            if let Some(worker) = worker {
                worker.retire(self.id);
            }
        });
    }

    fn publish(&mut self, display: Display) {
        if let Display::Failed(error, Some(_)) = &display {
            eprintln!("roc-native-windows shader fallback: {error}");
        }
        let frame = display.pixels().cloned();
        let size = frame
            .as_ref()
            .map_or((0, 0), |frame| (frame.width as i32, frame.height as i32));
        let display = if let Err(error) = self.picture.update(frame, false, size) {
            Display::Failed(error, None)
        } else {
            display
        };
        self.display.replace(display);
        // SAFETY: Only painting on the UI thread reads the published pixels.
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }
}

impl Drop for ShaderView {
    fn drop(&mut self) {
        self.retire();
        // SAFETY: Detach before dropping the Box registered as native user data.
        unsafe { RemoveWindowSubclass(self.hwnd, Some(paint), SUBCLASS) };
    }
}

unsafe extern "system" fn paint(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: The view owns the subclass state until explicit detach or
    // WM_NCDESTROY. The storage is read and written only on its UI thread.
    unsafe {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT | WM_PRINTCLIENT => {
                let state = &*(data as *const RefCell<Display>);
                let Ok(display) = state.try_borrow() else {
                    return DefSubclassProc(hwnd, message, wparam, lparam);
                };
                if display.pixels().is_some() {
                    return DefSubclassProc(hwnd, message, wparam, lparam);
                }
                if message == WM_PAINT {
                    let mut painting = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut painting);
                    draw_placeholder(hwnd, dc, &display);
                    EndPaint(hwnd, &painting);
                } else {
                    draw_placeholder(hwnd, wparam as HDC, &display);
                }
                0
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(paint), SUBCLASS);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

fn draw_placeholder(hwnd: HWND, dc: HDC, display: &Display) {
    if dc.is_null() {
        return;
    }
    let mut rect = RECT::default();
    let message = match display {
        Display::Failed(error, None) => format!("Shader preview unavailable\n{error}"),
        _ => "Preparing shader preview…".into(),
    };
    let text = wide(&message);
    // SAFETY: GDI owns the DC throughout this callback and reads live UTF-16.
    unsafe {
        GetClientRect(hwnd, &mut rect);
        FillRect(dc, &rect, GetSysColorBrush(COLOR_WINDOW));
        rect.left += 8;
        rect.top += 8;
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, GetSysColor(COLOR_WINDOWTEXT));
        DrawTextW(dc, text.as_ptr(), -1, &mut rect, DT_WORDBREAK | DT_NOPREFIX);
    }
}
