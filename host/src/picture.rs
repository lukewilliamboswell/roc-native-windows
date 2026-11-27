//! Paint the same alpha-composited pixels on screen and into runner captures.

use crate::Result;
use crate::raster::{Pixels, byte_count, destination};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const SUBCLASS: usize = 0x52415354;
const WORKER_SCALE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct Resize {
    pub(crate) source: Arc<Pixels>,
    pub(crate) size: (u32, u32),
}

pub(crate) struct Picture {
    hwnd: HWND,
    state: Box<Paint>,
    desired: Option<(u32, u32)>,
    resize: Option<Resize>,
}

#[derive(Default)]
struct Paint {
    source: Option<Arc<Pixels>>,
    image: Option<Bitmap>,
    contain: bool,
    background: Option<COLORREF>,
}

struct Bitmap {
    pixels: Arc<Pixels>,
    handle: HBITMAP,
}

impl Picture {
    pub(crate) fn new(hwnd: HWND) -> Result<Self> {
        let mut state = Box::new(Paint::default());
        // SAFETY: The boxed paint state stays put until the subclass is removed.
        if unsafe {
            SetWindowSubclass(
                hwnd,
                Some(paint_proc),
                SUBCLASS,
                state.as_mut() as *mut Paint as usize,
            )
        } == 0
        {
            return Err("could not mount image painting".into());
        }
        Ok(Self {
            hwnd,
            state,
            desired: None,
            resize: None,
        })
    }

    pub(crate) fn update(
        &mut self,
        pixels: Option<Arc<Pixels>>,
        contain: bool,
        bounds: (i32, i32),
    ) -> Result<()> {
        let changed = match (&self.state.source, &pixels) {
            (None, None) => false,
            (Some(current), Some(next)) => !Arc::ptr_eq(current, next),
            _ => true,
        };
        if changed {
            self.state.source = pixels;
            self.state.image = None;
        }
        if changed || self.state.contain != contain {
            self.state.contain = contain;
            self.invalidate();
        }
        let Some(source) = &self.state.source else {
            self.desired = None;
            self.resize = None;
            return Ok(());
        };
        let desired = worker_size(source, bounds, contain);
        if changed || self.desired != desired {
            self.desired = desired;
            self.resize = desired.map(|size| Resize {
                source: source.clone(),
                size,
            });
            if desired.is_none() {
                self.state.image = Some(Bitmap::new(source.clone())?);
                self.invalidate();
            }
        }
        Ok(())
    }

    pub(crate) fn take_resize(&mut self) -> Option<Resize> {
        self.resize.take()
    }

    pub(crate) fn accept_resize(&mut self, source: Arc<Pixels>, image: Arc<Pixels>) -> Result<()> {
        if self.desired == Some((image.width, image.height))
            && self
                .state
                .source
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &source))
        {
            self.state.image = Some(Bitmap::new(image)?);
            self.invalidate();
        }
        Ok(())
    }

    fn invalidate(&self) {
        // SAFETY: This is the owned image window on the UI thread.
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    pub(crate) fn set_background(&mut self, color: COLORREF) {
        if self.state.background != Some(color) {
            self.state.background = Some(color);
            self.invalidate();
        }
    }
}

fn worker_size(source: &Pixels, (width, height): (i32, i32), contain: bool) -> Option<(u32, u32)> {
    if source.bgra.len() < WORKER_SCALE_BYTES || width <= 0 || height <= 0 {
        return None;
    }
    let (_, _, width, height) = destination(width, height, (source.width, source.height), contain);
    let (width, height) = (width as u32, height as u32);
    let size = if byte_count(width, height).is_some() {
        (width, height)
    } else {
        (width.min(2048), height.min(2048))
    };
    (size != (source.width, source.height)).then_some(size)
}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: Unregister the callback before its state goes away.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(paint_proc), SUBCLASS);
        }
    }
}

impl Bitmap {
    fn new(pixels: Arc<Pixels>) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: pixels.width as i32,
                biHeight: -(pixels.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = null_mut();
        // SAFETY: Pixels is tightly packed BGRA of these validated dimensions.
        unsafe {
            let handle =
                CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if handle.is_null() || bits.is_null() {
                if !handle.is_null() {
                    DeleteObject(handle);
                }
                return Err(format!(
                    "cannot allocate the native image: {}",
                    std::io::Error::last_os_error()
                ));
            }
            std::ptr::copy_nonoverlapping(pixels.bgra.as_ptr(), bits.cast(), pixels.bgra.len());
            Ok(Self { pixels, handle })
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: The bitmap has been deselected from the paint DC.
        unsafe {
            DeleteObject(self.handle);
        }
    }
}

unsafe extern "system" fn paint_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: The window subclass data points to its live boxed paint state.
    unsafe {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                if !dc.is_null() {
                    paint_image(hwnd, dc, &*(data as *const Paint), true);
                    EndPaint(hwnd, &paint);
                }
                0
            }
            WM_PRINTCLIENT => {
                paint_image(hwnd, wp as HDC, &*(data as *const Paint), false);
                0
            }
            _ => DefSubclassProc(hwnd, message, wp, lp),
        }
    }
}

unsafe fn paint_image(hwnd: HWND, dc: HDC, state: &Paint, fill: bool) {
    // SAFETY: This runs only inside same-thread painting with valid HWND and DC.
    unsafe {
        let mut bounds = RECT::default();
        GetClientRect(hwnd, &mut bounds);
        if fill {
            if let Some(background) = state.background {
                let brush = CreateSolidBrush(background);
                if !brush.is_null() {
                    FillRect(dc, &bounds, brush);
                    DeleteObject(brush);
                }
            } else {
                FillRect(dc, &bounds, GetSysColorBrush(COLOR_WINDOW));
            }
        }
        let (Some(image), Some(original)) = (&state.image, &state.source) else {
            return;
        };
        let (x, y, width, height) = destination(
            bounds.right,
            bounds.bottom,
            (original.width, original.height),
            state.contain,
        );
        if width <= 0 || height <= 0 {
            return;
        }
        let source = CreateCompatibleDC(dc);
        if source.is_null() {
            return;
        }
        let previous = SelectObject(source, image.handle);
        if !previous.is_null() {
            GdiAlphaBlend(
                dc,
                x,
                y,
                width,
                height,
                source,
                0,
                0,
                image.pixels.width as i32,
                image.pixels.height as i32,
                BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                },
            );
            SelectObject(source, previous);
        }
        DeleteDC(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_images_resample_to_physical_bounds_and_fit() {
        let pixels = Pixels {
            width: 2048,
            height: 512,
            bgra: vec![0; 2048 * 512 * 4],
        };
        assert_eq!(worker_size(&pixels, (180, 80), true), Some((180, 45)));
        assert_eq!(worker_size(&pixels, (360, 160), true), Some((360, 90)));
        assert_eq!(worker_size(&pixels, (360, 160), false), Some((360, 160)));
        assert_eq!(worker_size(&pixels, (2048, 512), false), None);
        assert_eq!(
            worker_size(&pixels, (16000, 16000), false),
            Some((2048, 2048))
        );
    }
}
