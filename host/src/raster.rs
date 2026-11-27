//! Bounded image sources and a shared top-down premultiplied BGRA representation.

use crate::Result;
use std::path::Path;

pub(crate) const MAX_ENCODED_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_DIMENSION: u32 = 8192;
pub(crate) const MAX_PIXELS: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Pixels {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bgra: Vec<u8>,
}

pub(crate) fn byte_count(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    let pixels = (width as usize).checked_mul(height as usize)?;
    (pixels <= MAX_PIXELS).then_some(pixels * 4)
}

impl Pixels {
    pub(crate) fn from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Result<Self> {
        if byte_count(width, height) != Some(rgba.len()) {
            return Err("invalid raster dimensions or byte count".into());
        }
        for px in rgba.as_chunks_mut::<4>().0 {
            let alpha = u16::from(px[3]);
            px.swap(0, 2);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * alpha + 127) / 255) as u8;
            }
        }
        Ok(Self {
            width,
            height,
            bgra: rgba,
        })
    }

    pub(crate) fn resize(&self, width: u32, height: u32) -> Result<Self> {
        let size = byte_count(width, height).ok_or("display raster exceeds the pixel limit")?;
        let columns = (0..width)
            .map(|x| sample_axis(x, width, self.width))
            .collect::<Vec<_>>();
        let mut bgra = Vec::with_capacity(size);
        let row = (self.width * 4) as usize;
        for y in 0..height {
            let (top, bottom, fy) = sample_axis(y, height, self.height);
            for &(left, right, fx) in &columns {
                let tl = top * row + left * 4;
                let tr = top * row + right * 4;
                let bl = bottom * row + left * 4;
                let br = bottom * row + right * 4;
                for channel in 0..4 {
                    let pixel = |offset| u32::from(self.bgra[offset + channel]);
                    let upper = pixel(tl) * (256 - fx) + pixel(tr) * fx;
                    let lower = pixel(bl) * (256 - fx) + pixel(br) * fx;
                    bgra.push(((upper * (256 - fy) + lower * fy + 32768) >> 16) as u8);
                }
            }
        }
        Ok(Self {
            width,
            height,
            bgra,
        })
    }
}

// Interpolate at pixel centers in premultiplied space, clamping the outer edges.
fn sample_axis(i: u32, output: u32, input: u32) -> (usize, usize, u32) {
    let position = ((u64::from(2 * i + 1) * u64::from(input) * 128 / u64::from(output)) as i32
        - 128)
        .clamp(0, ((input - 1) * 256) as i32) as u32;
    let start = position / 256;
    (
        start as usize,
        (start + 1).min(input - 1) as usize,
        position % 256,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Bmp,
    Jpeg,
    Png,
}

impl Format {
    fn extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("bmp") {
            Some(Self::Bmp)
        } else if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
            Some(Self::Jpeg)
        } else if ext.eq_ignore_ascii_case("png") {
            Some(Self::Png)
        } else {
            None
        }
    }

    fn content(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"BM") {
            Some(Self::Bmp)
        } else {
            None
        }
    }
}

#[cfg(windows)]
pub(crate) fn supports(bytes: &[u8]) -> bool {
    Format::content(bytes).is_some()
}

pub(crate) fn destination(
    width: i32,
    height: i32,
    source: (u32, u32),
    contain: bool,
) -> (i32, i32, i32, i32) {
    if !contain || width <= 0 || height <= 0 {
        return (0, 0, width, height);
    }
    let (sw, sh) = (i64::from(source.0), i64::from(source.1));
    let (width, height) = (i64::from(width), i64::from(height));
    let (fit_w, fit_h) = if width * sh <= height * sw {
        (width, (width * sh / sw).max(1))
    } else {
        ((height * sw / sh).max(1), height)
    };
    (
        ((width - fit_w) / 2) as i32,
        ((height - fit_h) / 2) as i32,
        fit_w as i32,
        fit_h as i32,
    )
}

fn orient(mut image: Pixels, orientation: u16) -> Pixels {
    if !(2..=8).contains(&orientation) {
        return image;
    }
    let width = image.width as usize;
    let height = image.height as usize;
    let out_width = if orientation >= 5 { height } else { width };
    let mut bgra = vec![0; image.bgra.len()];
    for y in 0..height {
        for x in 0..width {
            let (ox, oy) = match orientation {
                2 => (width - 1 - x, y),
                3 => (width - 1 - x, height - 1 - y),
                4 => (x, height - 1 - y),
                5 => (y, x),
                6 => (height - 1 - y, x),
                7 => (height - 1 - y, width - 1 - x),
                8 => (y, width - 1 - x),
                _ => unreachable!(),
            };
            let src = (y * width + x) * 4;
            let dst = (oy * out_width + ox) * 4;
            bgra[dst..dst + 4].copy_from_slice(&image.bgra[src..src + 4]);
        }
    }
    if orientation >= 5 {
        std::mem::swap(&mut image.width, &mut image.height);
    }
    image.bgra = bgra;
    image
}

fn jpeg_orientation(bytes: &[u8]) -> u16 {
    fn scan(mut input: &[u8]) -> Option<u16> {
        input = input.strip_prefix(b"\xff\xd8")?;
        while let [0xff, ..] = input {
            input = input
                .iter()
                .position(|&byte| byte != 0xff)
                .and_then(|index| input.get(index..))?;
            let (&marker, rest) = input.split_first()?;
            if matches!(marker, 0xd9 | 0xda) {
                break;
            }
            let size = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize;
            let segment = rest.get(2..size)?;
            if marker == 0xe1
                && let Some(tiff) = segment.strip_prefix(b"Exif\0\0")
                && let Some(orientation) = tiff_orientation(tiff)
            {
                return Some(orientation);
            }
            input = rest.get(size..)?;
        }
        None
    }
    scan(bytes).unwrap_or(1)
}

fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let short = |bytes: &[u8]| {
        let word = bytes.get(..2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(word)
        } else {
            u16::from_be_bytes(word)
        })
    };
    let long = |bytes: &[u8]| {
        let word = bytes.get(..4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(word)
        } else {
            u32::from_be_bytes(word)
        })
    };
    if short(tiff.get(2..)?)? != 42 {
        return None;
    }
    let dir = tiff.get(long(tiff.get(4..)?)? as usize..)?;
    let count = short(dir)? as usize;
    for entry in dir.get(2..)?.as_chunks::<12>().0.iter().take(count) {
        if short(entry)? == 0x0112 && short(&entry[2..])? == 3 && long(&entry[4..])? == 1 {
            return short(&entry[8..]).filter(|value| (1..=8).contains(value));
        }
    }
    None
}

#[cfg(windows)]
pub(crate) use native::{decode_asset, decode_bytes};

#[cfg(windows)]
mod native {
    use super::*;
    use std::io::Read;
    use std::ptr::null;
    use std::sync::Arc;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };

    pub(crate) fn decode_asset(root: &Path, name: &str) -> Result<Arc<Pixels>> {
        let operation = || {
            let name = crate::asset_catalog::resolve(name)?;
            let expected =
                Format::extension(Path::new(name)).ok_or("expected a PNG, JPEG or BMP asset")?;
            let file = crate::rooted::read_asset(root, name)?;
            let size = file.metadata().map_err(|error| error.to_string())?.len();
            if size > MAX_ENCODED_BYTES as u64 {
                return Err("encoded image exceeds 32 MiB".into());
            }
            let mut bytes = Vec::with_capacity(size as usize);
            file.take((MAX_ENCODED_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if Format::content(&bytes) != Some(expected) {
                return Err("extension and image container disagree".into());
            }
            decode_bytes(&bytes)
        };
        operation().map_err(|error| format!("cannot decode image asset {name:?}: {error}"))
    }

    pub(crate) fn decode_bytes(bytes: &[u8]) -> Result<Arc<Pixels>> {
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err("encoded image exceeds 32 MiB".into());
        }
        let format = Format::content(bytes).ok_or("expected a PNG, JPEG or BMP image")?;
        let _com = Apartment::enter()?;
        // SAFETY: WIC uses this apartment only. The stream borrows bytes that outlive all decoder objects.
        let image = unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(wic)?;
            let decoder_class = match format {
                Format::Bmp => &CLSID_WICBmpDecoder,
                Format::Jpeg => &CLSID_WICJpegDecoder,
                Format::Png => &CLSID_WICPngDecoder,
            };
            let stream = factory.CreateStream().map_err(wic)?;
            stream.InitializeFromMemory(bytes).map_err(wic)?;
            let decoder: IWICBitmapDecoder =
                CoCreateInstance(decoder_class, None, CLSCTX_INPROC_SERVER).map_err(wic)?;
            decoder
                .Initialize(&stream, WICDecodeMetadataCacheOnDemand)
                .map_err(wic)?;
            let frame = decoder.GetFrame(0).map_err(wic)?;
            let (mut width, mut height) = (0, 0);
            frame.GetSize(&mut width, &mut height).map_err(wic)?;
            let size = byte_count(width, height)
                .ok_or("image pixel dimensions exceed the supported bounds")?;
            let converter = factory.CreateFormatConverter().map_err(wic)?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None::<&IWICPalette>,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(wic)?;
            let mut bgra = vec![0; size];
            converter
                .CopyPixels(null(), width * 4, &mut bgra)
                .map_err(wic)?;
            Pixels {
                width,
                height,
                bgra,
            }
        };
        let orientation = if format == Format::Jpeg {
            jpeg_orientation(bytes)
        } else {
            1
        };
        Ok(Arc::new(orient(image, orientation)))
    }

    struct Apartment;
    impl Apartment {
        fn enter() -> Result<Self> {
            // SAFETY: This call is balanced on the decoding worker thread.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(wic)?;
            Ok(Self)
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: Matches the successful initialization above.
            unsafe { CoUninitialize() };
        }
    }
    fn wic(error: windows::core::Error) -> String {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_bounds_and_alpha_are_checked() {
        assert_eq!(byte_count(0, 2), None);
        assert_eq!(byte_count(8192, 8192), None);
        assert!(Pixels::from_rgba(1, 1, vec![0; 3]).is_err());
        let pixels = Pixels::from_rgba(1, 1, vec![200, 100, 50, 128]).unwrap();
        assert_eq!(pixels.bgra, [25, 50, 100, 128]);
        assert_eq!(destination(100, 100, (6, 3), true), (0, 25, 100, 50));
        assert_eq!(destination(100, 100, (1, 2), false), (0, 0, 100, 100));
    }

    #[test]
    fn orientation_maps_all_eight_jpeg_layouts() {
        let expected = [
            [1, 2, 3, 4, 5, 6],
            [3, 2, 1, 6, 5, 4],
            [6, 5, 4, 3, 2, 1],
            [4, 5, 6, 1, 2, 3],
            [1, 4, 2, 5, 3, 6],
            [4, 1, 5, 2, 6, 3],
            [6, 3, 5, 2, 4, 1],
            [3, 6, 2, 5, 1, 4],
        ];
        for (index, pixels) in expected.into_iter().enumerate() {
            let result = orient(
                Pixels {
                    width: 3,
                    height: 2,
                    bgra: (1..=6).flat_map(|n| [n, 0, 0, 255]).collect(),
                },
                index as u16 + 1,
            );
            assert_eq!(
                result
                    .bgra
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| p[0])
                    .collect::<Vec<_>>(),
                pixels
            );
            assert_eq!(
                (result.width, result.height),
                if index < 4 { (3, 2) } else { (2, 3) }
            );
        }
        let jpeg = b"\xff\xd8\xff\xe1\x00\x1eExif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\xff\xd9";
        assert_eq!(jpeg_orientation(jpeg), 6);
        assert_eq!(jpeg_orientation(&jpeg[..12]), 1);
    }

    #[test]
    fn resampling_keeps_translucent_edges_premultiplied() {
        let source = Pixels::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();
        assert_eq!(source.resize(1, 1).unwrap().bgra, [0, 0, 128, 128]);
        assert_eq!(
            source.resize(3, 1).unwrap().bgra,
            [0, 0, 255, 255, 0, 0, 128, 128, 0, 0, 0, 0]
        );
        assert!(source.resize(0, 1).is_err());
        assert_eq!(sample_axis(8191, 8192, 8192), (8191, 8191, 0));
    }

    #[cfg(windows)]
    #[test]
    fn wic_decodes_png_with_premultiplied_alpha() {
        // 4 × 2, generated straight RGBA with opaque, translucent and clear pixels.
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x7f, 0xa8, 0x7d, 0x63, 0x00, 0x00, 0x00, 0x16, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x88, 0x1b, 0x18, 0xc0, 0x00, 0xc8, 0x44,
            0x17, 0x00, 0x00, 0xbd, 0xf5, 0x0a, 0xf7, 0x1a, 0x8e, 0x02, 0x8d, 0x00, 0x00, 0x00,
            0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let decoded = decode_bytes(&png).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 2));
        assert_eq!(
            &decoded.bgra[..16],
            &[0, 0, 255, 255, 0, 0, 128, 128, 0, 0, 0, 0, 255, 0, 0, 255],
        );
        assert!(decode_bytes(&png[..20]).is_err());
    }

    #[test]
    fn image_formats_are_restricted() {
        assert_eq!(
            Format::extension(Path::new("Café.JPEG")),
            Some(Format::Jpeg)
        );
        assert_eq!(Format::extension(Path::new("unexpected.tif")), None);
        assert_eq!(Format::content(b"BMtest"), Some(Format::Bmp));
        assert_ne!(
            Format::content(b"BMtest"),
            Format::extension(Path::new("mislabeled.png"))
        );
        assert_eq!(MAX_ENCODED_BYTES, 33_554_432);
    }
}
