use crate::raster::resolve_asset;
use crate::view::scale;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    BCM_SETIMAGELIST, BUTTON_IMAGELIST, BUTTON_IMAGELIST_ALIGN_LEFT, HIMAGELIST, ILC_COLOR32,
    ILC_MASK, ImageList_Create, ImageList_Destroy, ImageList_ReplaceIcon,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, IMAGE_ICON, LR_LOADFROMFILE, LoadImageW, SendMessageW,
};

pub(crate) struct Images(HIMAGELIST);

impl Images {
    pub(crate) fn new(dpi: u32, capacity: i32) -> Option<Self> {
        let edge = scale(dpi, 16).max(1);
        // SAFETY: Create a private list owning copies of the loaded images.
        let list = unsafe { ImageList_Create(edge, edge, ILC_COLOR32 | ILC_MASK, capacity, 1) };
        (list != 0).then_some(Self(list))
    }

    pub(crate) fn handle(&self) -> HIMAGELIST {
        self.0
    }

    pub(crate) fn add(&self, root: &Path, name: &str, dpi: u32) -> Option<i32> {
        let ext = Path::new(name).extension()?.to_str()?;
        if !ext.eq_ignore_ascii_case("ico") {
            return None;
        }
        let path = resolve_asset(root, name).ok()?;
        let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let edge = scale(dpi, 16).max(1);
        // SAFETY: Load an owned HICON from a live terminated filename.
        // ImageList_ReplaceIcon copies it before we destroy the loaded handle.
        let index = unsafe {
            let icon = LoadImageW(
                null_mut(),
                path.as_ptr(),
                IMAGE_ICON,
                edge,
                edge,
                LR_LOADFROMFILE,
            ) as HICON;
            if icon.is_null() {
                return None;
            }
            let index = ImageList_ReplaceIcon(self.0, -1, icon);
            DestroyIcon(icon);
            index
        };
        (index >= 0).then_some(index)
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        // SAFETY: The list has been detached or its control has been destroyed.
        unsafe { ImageList_Destroy(self.0) };
    }
}

#[derive(Default)]
pub(crate) struct ButtonIcon {
    requested: Option<(String, u32)>,
    list: Option<Images>,
}

impl ButtonIcon {
    pub(crate) fn sync(&mut self, hwnd: HWND, root: &Path, name: &str, dpi: u32) {
        if self
            .requested
            .as_ref()
            .is_some_and(|(current, scale)| current == name && *scale == dpi)
        {
            return;
        }
        self.requested = Some((name.into(), dpi));
        let next = (!name.is_empty())
            .then(|| Images::new(dpi, 1))
            .flatten()
            .filter(|list| list.add(root, name, dpi).is_some());
        if self.list.is_none() && next.is_none() {
            return;
        }
        let padding = scale(dpi, 4);
        let mut info = BUTTON_IMAGELIST {
            himl: next.as_ref().map_or(0, Images::handle),
            margin: RECT {
                left: padding,
                right: padding,
                top: 0,
                bottom: 0,
            },
            uAlign: BUTTON_IMAGELIST_ALIGN_LEFT,
        };
        let apply = |info: &BUTTON_IMAGELIST| {
            // SAFETY: The live button copies this structure synchronously.
            unsafe {
                SendMessageW(hwnd, BCM_SETIMAGELIST, 0, std::ptr::from_ref(info) as isize) != 0
            }
        };
        // Clear with a null list when accepted. Older Wine treats the Vista
        // BCCL_NOGLYPH sentinel as an image-list pointer during paint.
        let applied = apply(&info)
            || (next.is_none() && {
                info.himl = -1;
                apply(&info)
            });
        if applied {
            self.list = next;
            // SAFETY: The live button now refers to the new list or no list.
            unsafe { InvalidateRect(hwnd, null(), 1) };
        }
    }

    pub(crate) fn width(&self, dpi: u32) -> i32 {
        if self.list.is_some() {
            scale(dpi, 24)
        } else {
            0
        }
    }
}
