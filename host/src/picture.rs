//! Paint the same alpha-composited pixels on screen and into runner captures.

use crate::Result;
use crate::raster::{Pixels, destination};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const SUBCLASS: usize = 0x52415354;

pub(crate) struct Picture {
    hwnd: HWND,
    state: Box<Paint>,
}

#[derive(Default)]
struct Paint {
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
        Ok(Self { hwnd, state })
    }

    pub(crate) fn update(&mut self, pixels: Option<Arc<Pixels>>, contain: bool) -> Result<()> {
        let unchanged = match (&self.state.image, &pixels) {
            (None, None) => true,
            (Some(current), Some(next)) => Arc::ptr_eq(&current.pixels, next),
            _ => false,
        };
        if unchanged && self.state.contain == contain {
            return Ok(());
        }
        if !unchanged {
            self.state.image = pixels.map(Bitmap::new).transpose()?;
        }
        self.state.contain = contain;
        // SAFETY: This is the owned image window on the UI thread.
        unsafe {
            InvalidateRect(self.hwnd, null(), 0);
        }
        Ok(())
    }

    pub(crate) fn set_background(&mut self, color: COLORREF) {
        if self.state.background != Some(color) {
            self.state.background = Some(color);
            // SAFETY: This is the owned image window on the UI thread.
            unsafe {
                InvalidateRect(self.hwnd, null(), 0);
            }
        }
    }
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
        let Some(image) = &state.image else { return };
        let (x, y, width, height) = destination(
            bounds.right,
            bounds.bottom,
            (image.pixels.width, image.pixels.height),
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
