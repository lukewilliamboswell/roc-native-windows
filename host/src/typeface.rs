use crate::Result;
use crate::raster::resolve_asset;
use crate::view::wide;
use crate::wire::Node;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use windows_sys::Win32::Foundation::{HANDLE, HWND, S_FALSE};
use windows_sys::Win32::Globalization::{
    CSTR_EQUAL, CompareStringOrdinal, SIC_COMPLEX, ScriptIsComplex,
};
use windows_sys::Win32::Graphics::Gdi::*;

const MAX_FONT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Typeface {
    asset: String,
    family: String,
    face: String,
}

impl Typeface {
    pub(crate) fn read(node: &Node) -> Option<Self> {
        Some(Self {
            asset: node.attr("font-asset")?.into(),
            family: node.attr("font-family").unwrap_or("").into(),
            face: node.attr("font-face").unwrap_or("").into(),
        })
    }

    fn selection(&self) -> Result<Selection> {
        let name = wide(&self.family);
        if self.family.trim().is_empty()
            || self.family.contains('\0')
            || name.len() > LF_FACESIZE as usize
        {
            return Err("font family must be 1 to 31 UTF-16 units".into());
        }
        let (weight, italic) = match self.face.as_str() {
            "regular" => (FW_NORMAL, false),
            "bold" => (FW_BOLD, false),
            "italic" => (FW_NORMAL, true),
            "bold-italic" => (FW_BOLD, true),
            _ => return Err("unknown packaged face".into()),
        };
        Ok(Selection {
            name,
            weight: weight as i32,
            italic,
        })
    }
}

struct Selection {
    name: Vec<u16>,
    weight: i32,
    italic: bool,
}

impl Selection {
    fn metrics(&self, mut base: LOGFONTW) -> LOGFONTW {
        base.lfWeight = self.weight;
        base.lfItalic = u8::from(self.italic);
        base.lfCharSet = DEFAULT_CHARSET;
        base.lfPitchAndFamily = DEFAULT_PITCH;
        base.lfOutPrecision = OUT_TT_ONLY_PRECIS;
        base.lfFaceName = [0; LF_FACESIZE as usize];
        base.lfFaceName[..self.name.len()].copy_from_slice(&self.name);
        base
    }

    fn check(&self, bytes: &[u8]) -> Result<()> {
        let os2 = table(bytes, b"OS/2").ok_or("font has no OS/2 metrics")?;
        let weight = u16_at(os2, 4).ok_or("font has invalid OS/2 weight")?;
        let flags = u16_at(os2, 62).ok_or("font has invalid OS/2 style")?;
        if i32::from(weight) != self.weight
            || (flags & 0x0001 != 0) != self.italic
            || (flags & 0x0020 != 0) != (self.weight == FW_BOLD as i32)
        {
            return Err("file does not contain the requested weight and slant".into());
        }
        Ok(())
    }

    fn matches(&self, dc: HDC, bytes: &[u8]) -> bool {
        let mut metrics = TEXTMETRICW::default();
        let mut family = [0_u16; LF_FACESIZE as usize];
        // SAFETY: The selected HFONT is live and both buffers are valid.
        unsafe {
            GetTextMetricsW(dc, &mut metrics) != 0
                && metrics.tmWeight == self.weight
                && (metrics.tmItalic != 0) == self.italic
                && GetTextFaceW(dc, family.len() as i32, family.as_mut_ptr()) > 0
                && CompareStringOrdinal(family.as_ptr(), -1, self.name.as_ptr(), -1, 1)
                    == CSTR_EQUAL
                && selected_data_matches(dc, bytes)
        }
    }
}

#[derive(Default)]
pub(crate) struct Registrations {
    files: BTreeMap<PathBuf, Rc<Registration>>,
    claimed: BTreeMap<(String, String), PathBuf>,
    diagnostics: BTreeSet<Typeface>,
}

pub(crate) type FontRegistry = Rc<RefCell<Registrations>>;

impl Registrations {
    pub(crate) fn font(
        &mut self,
        window: HWND,
        assets: &Path,
        requested: &Typeface,
        base: LOGFONTW,
    ) -> Option<PackagedFont> {
        match self.open(window, assets, requested, base) {
            Ok(font) => Some(font),
            Err(error) => {
                if self.diagnostics.insert(requested.clone()) {
                    eprintln!(
                        "packaged font {:?} ({:?}, {}): {error}; using the system text role",
                        requested.asset, requested.family, requested.face
                    );
                }
                None
            }
        }
    }

    fn open(
        &mut self,
        window: HWND,
        assets: &Path,
        requested: &Typeface,
        base: LOGFONTW,
    ) -> Result<PackagedFont> {
        let selection = requested.selection()?;
        let path = resolve_asset(assets, &requested.asset)?;
        let claim = (requested.family.to_uppercase(), requested.face.clone());
        if self.claimed.get(&claim).is_some_and(|owner| owner != &path) {
            return Err("another app asset already supplied this family and face".into());
        }
        let registration = if let Some(registered) = self.files.get(&path) {
            selection.check(&registered.bytes)?;
            Rc::clone(registered)
        } else {
            Rc::new(Registration::open(&path, &selection)?)
        };
        // SAFETY: A complete initialized LOGFONTW is read synchronously.
        let handle = unsafe { CreateFontIndirectW(&selection.metrics(base)) };
        if handle.is_null() {
            return Err("Windows could not create the logical font".into());
        }
        let font = PackagedFont {
            handle,
            _registration: Rc::clone(&registration),
        };
        if !with_font(window, handle, |dc| {
            selection.matches(dc, &registration.bytes)
        })
        .unwrap_or(false)
        {
            return Err("Windows did not select the requested asset face".into());
        }
        self.files.entry(path.clone()).or_insert(registration);
        self.claimed.entry(claim).or_insert(path);
        Ok(font)
    }
}

struct Registration {
    handle: HANDLE,
    bytes: Vec<u8>,
}

impl Registration {
    fn open(path: &Path, selection: &Selection) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_FONT_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read font: {error}"))?;
        if bytes.len() as u64 > MAX_FONT_BYTES {
            return Err("font file exceeds 16 MiB".into());
        }
        if !bytes.starts_with(&[0, 1, 0, 0])
            || table(&bytes, b"glyf").is_none()
            || table(&bytes, b"loca").is_none()
            || table(&bytes, b"fvar").is_some()
        {
            return Err("expected a single static TrueType face".into());
        }
        selection.check(&bytes)?;
        let mut count = 0;
        // SAFETY: Windows copies the bounded buffer on success and writes the face
        // count through the last pointer, which windows-sys declares as const.
        let handle = unsafe {
            AddFontMemResourceEx(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                null(),
                &raw mut count,
            )
        };
        if handle.is_null() {
            return Err("Windows could not register the private font".into());
        }
        let resource = Self { handle, bytes };
        if count != 1 {
            return Err("expected exactly one private font face".into());
        }
        Ok(resource)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: This resource owns the registration and all dependent fonts have been dropped.
        unsafe { RemoveFontMemResourceEx(self.handle) };
    }
}

pub(crate) struct PackagedFont {
    handle: HFONT,
    _registration: Rc<Registration>,
}

impl PackagedFont {
    pub(crate) fn for_caption(&self, window: HWND, caption: &str, fallback: HFONT) -> HFONT {
        // Tabs and line breaks are handled by DrawText, independent of glyph coverage.
        let units = caption
            .encode_utf16()
            .filter(|unit| !matches!(unit, 9 | 10 | 13))
            .collect::<Vec<_>>();
        let Ok(count) = i32::try_from(units.len()) else {
            return fallback;
        };
        if units.is_empty() {
            return self.handle;
        }
        // Complex text, supplementary characters and U+0000 use the whole system run.
        // SAFETY: Windows reads the counted UTF-16 buffer synchronously.
        if unsafe { ScriptIsComplex(units.as_ptr(), count, SIC_COMPLEX) } != S_FALSE
            || caption
                .chars()
                .any(|scalar| scalar == '\0' || u32::from(scalar) > 0xffff)
        {
            return fallback;
        }
        let covers_text = with_font(window, self.handle, |dc| {
            let mut glyphs = vec![0_u16; units.len()];
            // SAFETY: Input and output buffers contain count entries.
            let mapped = unsafe {
                GetGlyphIndicesW(
                    dc,
                    units.as_ptr(),
                    count,
                    glyphs.as_mut_ptr(),
                    GGI_MARK_NONEXISTING_GLYPHS,
                ) == count as u32
            };
            mapped && !glyphs.contains(&u16::MAX)
        });
        if covers_text == Some(true) {
            self.handle
        } else {
            fallback
        }
    }
}

impl Drop for PackagedFont {
    fn drop(&mut self) {
        // SAFETY: All DC selections have been restored and the view detached or destroyed.
        unsafe { DeleteObject(self.handle) };
    }
}

fn with_font<T>(window: HWND, font: HFONT, use_dc: impl FnOnce(HDC) -> T) -> Option<T> {
    // SAFETY: The HWND is live on the calling UI thread and the font is owned by the caller.
    unsafe {
        let dc = GetDC(window);
        if dc.is_null() {
            return None;
        }
        let previous = SelectObject(dc, font);
        let result = (!previous.is_null() && previous as isize != -1).then(|| {
            let result = use_dc(dc);
            SelectObject(dc, previous);
            result
        });
        ReleaseDC(window, dc);
        result
    }
}

fn selected_data_matches(dc: HDC, bytes: &[u8]) -> bool {
    // SAFETY: The DC holds the candidate font. A null buffer queries its file size.
    let len = unsafe { GetFontData(dc, 0, 0, null_mut(), 0) };
    if len as usize != bytes.len() {
        return false;
    }
    let mut block = [0; 8192];
    bytes
        .chunks(block.len())
        .enumerate()
        .all(|(index, expected)| {
            let offset = (index * block.len()) as u32;
            let size = expected.len() as u32;
            // SAFETY: The requested block is within the selected file and the writable buffer.
            let read = unsafe { GetFontData(dc, 0, offset, block.as_mut_ptr().cast(), size) };
            read == size && block[..expected.len()] == *expected
        })
}

fn table<'a>(bytes: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let count = usize::from(u16_at(bytes, 4)?);
    let entry = bytes
        .get(12..12 + count * 16)?
        .as_chunks::<16>()
        .0
        .iter()
        .find(|entry| entry.starts_with(name))?;
    let offset = u32::from_be_bytes(entry[8..12].try_into().ok()?) as usize;
    let size = u32::from_be_bytes(entry[12..16].try_into().ok()?) as usize;
    bytes.get(offset..offset.checked_add(size)?)
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    const SANS: &[u8] = include_bytes!("../../examples/counter/assets/fonts/IBMPlexSans-Bold.ttf");
    const MONO: &[u8] = include_bytes!("../../examples/http/assets/fonts/IBMPlexMono-Regular.ttf");

    fn request(asset: &str, family: &str, face: &str) -> Typeface {
        Typeface {
            asset: asset.into(),
            family: family.into(),
            face: face.into(),
        }
    }

    fn metrics(height: i32) -> LOGFONTW {
        LOGFONTW {
            lfHeight: height,
            ..LOGFONTW::default()
        }
    }

    #[test]
    fn verifies_private_file_identity_across_logical_fonts_and_caption_fallback() {
        let assets = std::env::temp_dir().join(format!("roc-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&assets).unwrap();
        for (name, bytes) in [
            ("sans.ttf", SANS),
            ("mono.ttf", MONO),
            ("collision.ttf", MONO),
            ("corrupt.ttf", b"not a font"),
            ("collection.ttf", b"ttcf"),
        ] {
            std::fs::write(assets.join(name), bytes).unwrap();
        }
        let window = unsafe { GetDesktopWindow() };
        let mut loaded = Registrations::default();
        let sans_request = request("sans.ttf", "IBM Plex Sans", "bold");
        let sans = loaded
            .open(window, &assets, &sans_request, metrics(-28))
            .unwrap();
        let larger = loaded
            .open(window, &assets, &sans_request, metrics(-44))
            .unwrap();
        assert!(Rc::ptr_eq(&sans._registration, &larger._registration));
        assert_ne!(sans.handle, larger.handle);
        let mono_request = request("mono.ttf", "IBM Plex Mono", "regular");
        let mono = loaded
            .open(window, &assets, &mono_request, metrics(-18))
            .unwrap();
        assert_eq!(
            mono.for_caption(window, "HTTP 200\nContent-Type", null_mut()),
            mono.handle
        );
        assert_eq!(
            mono.for_caption(window, "Été · ١٢३", null_mut()),
            null_mut()
        );
        assert_eq!(mono.for_caption(window, "Schwa 𐐀", null_mut()), null_mut());
        assert_eq!(
            mono.for_caption(window, "Cafe\u{301}", null_mut()),
            null_mut()
        );
        assert_eq!(
            mono.for_caption(window, "Hello שלום", null_mut()),
            null_mut()
        );
        assert_eq!(mono.for_caption(window, "漢字", null_mut()), null_mut());
        for invalid in [
            request("sans.ttf", "IBM Plex Sans", "italic"),
            request("sans.ttf", "IBM Plex Sans", "regular"),
            request("sans.ttf", "Arial", "bold"),
            request("collision.ttf", "IBM Plex Mono", "regular"),
            request("mono.ttf", &"𐐀".repeat(16), "regular"),
            request("corrupt.ttf", "IBM Plex Mono", "bold"),
            request("collection.ttf", "IBM Plex Mono", "bold"),
            request("absent.ttf", "IBM Plex Mono", "bold"),
            request("../mono.ttf", "IBM Plex Mono", "bold"),
        ] {
            assert!(
                loaded
                    .open(window, &assets, &invalid, metrics(-18))
                    .is_err(),
                "{invalid:?}"
            );
        }
        assert_eq!(loaded.files.len(), 2);
        let private_face = Rc::downgrade(&mono._registration);
        drop(mono);
        drop(sans);
        drop(larger);
        drop(loaded);
        assert!(private_face.upgrade().is_none());
        std::fs::remove_dir_all(assets).unwrap();
    }
}
