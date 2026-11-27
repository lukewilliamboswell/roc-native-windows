use crate::rooted::read_asset;
use crate::view::scale;
use std::io::Read;
use std::path::Path;
use std::ptr::null;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    BCM_SETIMAGELIST, BUTTON_IMAGELIST, BUTTON_IMAGELIST_ALIGN_LEFT, HIMAGELIST, ILC_COLOR32,
    ILC_MASK, ImageList_Create, ImageList_Destroy, ImageList_ReplaceIcon,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, HICON, LookupIconIdFromDirectoryEx, SendMessageW,
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
        let name = crate::asset_catalog::resolve(name).ok()?;
        let ext = Path::new(name).extension()?.to_str()?;
        if !ext.eq_ignore_ascii_case("ico") {
            return None;
        }
        const MAX_ENCODED: u64 = 16 * 1024 * 1024;
        let mut bytes = Vec::new();
        read_asset(root, name)
            .ok()?
            .take(MAX_ENCODED + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_ENCODED {
            return None;
        }
        let icon = native_icon(&bytes, scale(dpi, 16).max(1))?;
        // SAFETY: ImageList_ReplaceIcon copies the loaded image before destruction.
        let index = unsafe {
            let index = ImageList_ReplaceIcon(self.0, -1, icon);
            DestroyIcon(icon);
            index
        };
        (index >= 0).then_some(index)
    }
}

fn native_icon(bytes: &[u8], edge: i32) -> Option<HICON> {
    if !bytes.starts_with(&[0, 0, 1, 0]) {
        return None;
    }
    let count = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?);
    if !(1..=256).contains(&count) {
        return None;
    }
    let directory = bytes.get(6..6 + usize::from(count) * 16)?;
    let mut group = vec![0u16, 1, count];
    let mut images = Vec::with_capacity(count.into());
    for (index, entry) in directory.as_chunks::<16>().0.iter().enumerate() {
        let length = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
        if length == 0 || offset < 6 + directory.len() {
            return None;
        }
        images.push(bytes.get(offset..offset.checked_add(length)?)?);
        group.extend(
            entry[..12]
                .as_chunks::<2>()
                .0
                .iter()
                .copied()
                .map(u16::from_le_bytes),
        );
        group.push(index as u16 + 1);
    }
    // SAFETY: The group header and bounded GRPICONDIRENTRY fields are aligned and complete.
    let index = unsafe { LookupIconIdFromDirectoryEx(group.as_ptr().cast(), 1, edge, edge, 0) };
    let image = images.get(usize::try_from(index).ok()?.checked_sub(1)?)?;
    let mut aligned = vec![0u32; image.len().div_ceil(4)];
    // SAFETY: The output holds at least image.len() initialized writable bytes.
    let icon = unsafe {
        std::ptr::copy_nonoverlapping(image.as_ptr(), aligned.as_mut_ptr().cast(), image.len());
        CreateIconFromResourceEx(
            aligned.as_ptr().cast(),
            image.len() as u32,
            1,
            0x00030000,
            edge,
            edge,
            0,
        )
    };
    (!icon.is_null()).then_some(icon)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_native_icon_from_the_packaged_frames() {
        let sample = include_bytes!("../../examples/starter/assets/reading-list.ico");
        for edge in [16, 24, 32] {
            let icon = native_icon(sample, edge).unwrap();
            // SAFETY: The test owns the native icon.
            assert_ne!(unsafe { DestroyIcon(icon) }, 0);
        }
        assert!(native_icon(&sample[..14], 16).is_none());
    }
}
