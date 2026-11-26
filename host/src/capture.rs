//! Capture only the spec runner's own Win32 window.
use crate::Result;
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub(crate) const MAX_RECORDING_DURATION: Duration = Duration::from_secs(60);
pub(crate) const MAX_RECORDING_FPS: u32 = 30;

struct FrameClock {
    interval: Duration,
    last: Option<Duration>,
}

impl FrameClock {
    fn new(fps: u32) -> Result<Self> {
        if !(1..=MAX_RECORDING_FPS).contains(&fps) {
            return Err(format!(
                "recording frame rate must be between 1 and {MAX_RECORDING_FPS}"
            ));
        }
        Ok(Self {
            interval: Duration::from_secs(1) / fps,
            last: None,
        })
    }

    fn due(&mut self, now: Duration) -> Result<bool> {
        if now > MAX_RECORDING_DURATION {
            return Err("recording exceeded 60 seconds".into());
        }
        if let Some(last) = self.last {
            let elapsed = now
                .checked_sub(last)
                .ok_or("recording clock moved backwards")?;
            if elapsed < self.interval {
                return Ok(false);
            }
        }
        self.last = Some(now);
        Ok(true)
    }
}

fn gif_delay(start: Duration, end: Duration) -> u16 {
    // Round absolute times first so the changes in frame duration cannot
    // accumulate rounding errors throughout a recording.
    let centiseconds = |time: Duration| (time.as_micros() + 5_000) / 10_000;
    let elapsed = centiseconds(end).saturating_sub(centiseconds(start));
    elapsed.clamp(1, u128::from(u16::MAX)) as u16
}

#[derive(PartialEq, Eq)]
struct Frame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

enum Sample {
    Frame(Duration, Frame),
    Finish(Duration),
}

fn encode_gif<W: Write>(out: W, samples: Receiver<Sample>) -> Result<()> {
    let Ok(Sample::Frame(mut start, mut current)) = samples.recv() else {
        return Err("recording ended without any frames".into());
    };
    let width = u16::try_from(current.width).map_err(|_| "GIF width exceeds 65535 pixels")?;
    let height = u16::try_from(current.height).map_err(|_| "GIF height exceeds 65535 pixels")?;
    let mut encoder = gif::Encoder::new(out, width, height, &[]).map_err(gif_error)?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(gif_error)?;
    for sample in samples {
        match sample {
            Sample::Frame(at, frame) if frame != current => {
                write_gif_frame(&mut encoder, width, height, current, gif_delay(start, at))?;
                current = frame;
                start = at;
            }
            Sample::Frame(_, _) => {}
            Sample::Finish(end) => {
                write_gif_frame(&mut encoder, width, height, current, gif_delay(start, end))?;
                return encoder
                    .into_inner()
                    .map_err(gif_error)?
                    .flush()
                    .map_err(|err| format!("flush GIF: {err}"));
            }
        }
    }
    Err("GIF recording was interrupted".into())
}

fn write_gif_frame<W: Write>(
    encoder: &mut gif::Encoder<W>,
    width: u16,
    height: u16,
    mut frame: Frame,
    delay: u16,
) -> Result<()> {
    if (frame.width, frame.height) != (u32::from(width), u32::from(height))
        || frame.bgra.len() != width as usize * height as usize * 4
    {
        return Err("frame does not match the GIF canvas".into());
    }
    for pixel in frame.bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let mut image = gif::Frame::from_rgba_speed(width, height, &mut frame.bgra, 10);
    image.delay = delay;
    encoder.write_frame(&image).map_err(gif_error)
}

fn gif_error(error: gif::EncodingError) -> String {
    format!("GIF encoder: {error}")
}

#[cfg(windows)]
pub(crate) use native::{Recorder, screenshot};

#[cfg(windows)]
mod native {
    use super::{Frame, FrameClock, MAX_RECORDING_DURATION, Sample, encode_gif};
    use crate::Result;
    use std::ffi::c_void;
    use std::fs::{self, File, OpenOptions};
    use std::io::BufWriter;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
        IWICBitmapEncoder, IWICBitmapFrameEncode, IWICImagingFactory, WICBitmapEncoderNoCache,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::core::PCWSTR;
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, HGDIOBJ, IntersectClipRect,
        MapWindowPoints, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
        RestoreDC, SRCCOPY, SaveDC, SelectObject, SetViewportOrgEx,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_CHILD, GW_HWNDNEXT, GetClientRect, GetWindow, GetWindowRect, IsIconic, IsWindowVisible,
        PRF_CLIENT, PRF_ERASEBKGND, PRF_NONCLIENT, SendMessageW, WM_PRINT, WM_PRINTCLIENT,
    };

    const MAX_PIXELS: usize = 1920 * 1080;
    const QUEUED_FRAMES: usize = 2;
    static NEXT_ARTIFACT: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn screenshot(hwnd: HWND, destination: &Path) -> Result<()> {
        let frame = grab(hwnd)?;
        let path = destination.to_path_buf();
        let worker = thread::Builder::new()
            .name("roc-spec-png".into())
            .spawn(move || {
                with_artifact(&path, |stage| {
                    let _apartment = Apartment::enter()?;
                    let encoder = open_png_encoder(stage)?;
                    write_png(&encoder, &frame)?;
                    // SAFETY: Encoding and stream access stay on this thread.
                    unsafe { encoder.Commit() }.map_err(wic_error)
                })
            })
            .map_err(io_error)?;
        worker.join().map_err(|_| "PNG encoding thread stopped")?
    }

    pub(crate) struct Recorder {
        clock: FrameClock,
        sender: Option<SyncSender<Sample>>,
        worker: Option<JoinHandle<Result<()>>>,
        canvas: Option<(u32, u32)>,
    }

    impl Recorder {
        pub(crate) fn new(path: &Path, fps: u32) -> Result<Self> {
            let clock = FrameClock::new(fps)?;
            let path = path.to_path_buf();
            let (sender, receiver) = mpsc::sync_channel(QUEUED_FRAMES);
            let worker = thread::Builder::new()
                .name("roc-spec-gif".into())
                .spawn(move || {
                    with_artifact(&path, |stage| {
                        let file = File::create(stage).map_err(io_error)?;
                        encode_gif(BufWriter::new(file), receiver)
                    })
                })
                .map_err(io_error)?;
            Ok(Self {
                clock,
                sender: Some(sender),
                worker: Some(worker),
                canvas: None,
            })
        }

        pub(crate) fn record_sample(&mut self, hwnd: HWND, elapsed: Duration) -> Result<()> {
            if !self.clock.due(elapsed)? {
                return Ok(());
            }
            let frame = grab(hwnd)?;
            let size = (frame.width, frame.height);
            if *self.canvas.get_or_insert(size) != size {
                return Err("the window changed size during GIF recording".into());
            }
            let sender = self.sender.as_ref().ok_or("recording already ended")?;
            match sender.try_send(Sample::Frame(elapsed, frame)) {
                Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
                // Keep the encoder's diagnostic for finish.
                Err(TrySendError::Disconnected(_)) => {
                    let worker = self.worker.take().ok_or("GIF encoding thread stopped")?;
                    worker.join().map_err(|_| "GIF encoding thread stopped")??;
                    Err("GIF encoder closed before recording ended".into())
                }
            }
        }

        pub(crate) fn finish(mut self, elapsed: Duration) -> Result<()> {
            if elapsed > MAX_RECORDING_DURATION {
                return Err("recording exceeded 60 seconds".into());
            }
            let sender = self.sender.take().ok_or("recording already ended")?;
            let _ = sender.send(Sample::Finish(elapsed));
            drop(sender);
            let worker = self.worker.take().ok_or("GIF encoding thread stopped")?;
            worker.join().map_err(|_| "GIF encoding thread stopped")?
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            self.sender.take();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn grab(hwnd: HWND) -> Result<Frame> {
        let mut rect = RECT::default();
        // SAFETY: The runner supplies its own HWND, live on the calling thread.
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return Err("show and restore the app window before capturing it".into());
            }
            if GetClientRect(hwnd, &mut rect) == 0 {
                return Err(last_error("measure the app window"));
            }
        }
        let width = u32::try_from(rect.right).map_err(|_| "invalid capture width")?;
        let height = u32::try_from(rect.bottom).map_err(|_| "invalid capture height")?;
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .filter(|&pixels| (1..=MAX_PIXELS).contains(&pixels))
            .ok_or("capture requires a nonempty client area of at most 2,073,600 pixels")?;
        let bitmap = Dib::new(width, height)?;
        // SAFETY: The compatible DC selects a DIB large enough for this window.
        // Painting is synchronous and GdiFlush completes drawing before reading.
        let mut bgra = unsafe {
            // Initialize the unused fourth byte as well as any pixel that a
            // control chooses not to repaint through WM_PRINT.
            bitmap.bits.cast::<u8>().write_bytes(0, pixels * 4);
            if RedrawWindow(
                hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            ) == 0
            {
                return Err(last_error("redraw the app window before capture"));
            }
            let saved = SaveDC(bitmap.dc);
            if saved == 0 {
                return Err(last_error("save app capture device context"));
            }
            SendMessageW(
                hwnd,
                WM_PRINT,
                bitmap.dc as usize,
                (PRF_CLIENT | PRF_ERASEBKGND) as isize,
            );
            if RestoreDC(bitmap.dc, saved) == 0 {
                return Err(last_error("restore app capture device context"));
            }
            paint_children(hwnd, bitmap.dc, width as i32, height as i32)?;
            if GdiFlush() == 0 {
                return Err(last_error("flush app window capture"));
            }
            std::slice::from_raw_parts(bitmap.bits.cast::<u8>(), pixels * 4).to_vec()
        };
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Ok(Frame {
            width,
            height,
            bgra,
        })
    }

    fn paint_children(window: HWND, target: HDC, width: i32, height: i32) -> Result<()> {
        let scratch = Dib::new(width as u32, height as u32)?;
        // SAFETY: Window and its child HWNDs belong to the runner's UI thread.
        // Child drawing uses a private DIB with a child-relative origin; only
        // the region inside the app is composed back into the result.
        unsafe {
            let mut children = Vec::new();
            let mut child = GetWindow(window, GW_CHILD);
            while !child.is_null() {
                if IsWindowVisible(child) != 0 {
                    children.push(child);
                }
                child = GetWindow(child, GW_HWNDNEXT);
            }
            // The first child is highest in z-order.
            for child in children.into_iter().rev() {
                let mut bounds = RECT::default();
                let mut client = RECT::default();
                if GetWindowRect(child, &mut bounds) == 0 || GetClientRect(child, &mut client) == 0
                {
                    return Err(last_error("measure native child for capture"));
                }
                let mut origin = POINT {
                    x: bounds.left,
                    y: bounds.top,
                };
                MapWindowPoints(null_mut(), window, &mut origin, 1);
                let mut client_origin = POINT::default();
                MapWindowPoints(child, window, &mut client_origin, 1);
                let screen = (width, height);
                let extent = (bounds.right - bounds.left, bounds.bottom - bounds.top);
                if extent != (client.right, client.bottom) {
                    compose(target, &scratch, screen, origin, extent, |dc| {
                        SendMessageW(child, WM_PRINT, dc as usize, PRF_NONCLIENT as isize);
                    })?;
                }
                compose(
                    target,
                    &scratch,
                    screen,
                    client_origin,
                    (client.right, client.bottom),
                    |dc| {
                        SendMessageW(
                            child,
                            WM_PRINTCLIENT,
                            dc as usize,
                            (PRF_CLIENT | PRF_ERASEBKGND) as isize,
                        );
                    },
                )?;
            }
        }
        Ok(())
    }

    fn compose(
        target: HDC,
        scratch: &Dib,
        (width, height): (i32, i32),
        origin: POINT,
        (patch_width, patch_height): (i32, i32),
        draw: impl FnOnce(HDC),
    ) -> Result<()> {
        let left = origin.x.max(0);
        let top = origin.y.max(0);
        let w = origin.x.saturating_add(patch_width).min(width) - left;
        let h = origin.y.saturating_add(patch_height).min(height) - top;
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        // SAFETY: Both device contexts select only our owned bitmaps. The
        // clipping preserves already painted pixels outside this child.
        unsafe {
            let dc = scratch.dc;
            if BitBlt(dc, 0, 0, w, h, target, left, top, SRCCOPY) == 0 {
                return Err(last_error("copy background for native child"));
            }
            let saved = SaveDC(dc);
            if saved == 0 {
                return Err(last_error("save child capture device context"));
            }
            IntersectClipRect(dc, 0, 0, w, h);
            SetViewportOrgEx(dc, origin.x - left, origin.y - top, null_mut());
            draw(dc);
            if RestoreDC(dc, saved) == 0 {
                return Err(last_error("restore child capture device context"));
            }
            if BitBlt(target, left, top, w, h, dc, 0, 0, SRCCOPY) == 0 {
                return Err(last_error("composite native child"));
            }
        }
        Ok(())
    }

    struct Dib {
        dc: HDC,
        image: HBITMAP,
        previous: HGDIOBJ,
        bits: *mut c_void,
    }

    impl Dib {
        fn new(width: u32, height: u32) -> Result<Self> {
            let mut dib = Self {
                dc: null_mut(),
                image: null_mut(),
                previous: null_mut(),
                bits: null_mut(),
            };
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..BITMAPINFOHEADER::default()
                },
                ..BITMAPINFO::default()
            };
            // SAFETY: A memory DC selects an owned, top-down, 32-bit DIB.
            // This object releases both handles, including on partial failure.
            unsafe {
                dib.dc = CreateCompatibleDC(null_mut());
                if dib.dc.is_null() {
                    return Err(last_error("create a capture device context"));
                }
                dib.image =
                    CreateDIBSection(dib.dc, &info, DIB_RGB_COLORS, &mut dib.bits, null_mut(), 0);
                if dib.image.is_null() || dib.bits.is_null() {
                    return Err(last_error("create the capture bitmap"));
                }
                dib.previous = SelectObject(dib.dc, dib.image);
                if dib.previous.is_null() {
                    return Err(last_error("select the capture bitmap"));
                }
            }
            Ok(dib)
        }
    }

    impl Drop for Dib {
        fn drop(&mut self) {
            // SAFETY: The DC is thread-local and the bitmap is owned by self.
            unsafe {
                if !self.previous.is_null() {
                    SelectObject(self.dc, self.previous);
                }
                if !self.image.is_null() {
                    DeleteObject(self.image);
                }
                if !self.dc.is_null() {
                    DeleteDC(self.dc);
                }
            }
        }
    }

    struct Apartment;

    impl Apartment {
        fn enter() -> Result<Self> {
            // SAFETY: Initialize COM on a newly spawned encoder thread.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(wic_error)?;
            Ok(Self)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: Balances CoInitializeEx on this same thread.
            unsafe { CoUninitialize() };
        }
    }

    fn open_png_encoder(path: &Path) -> Result<IWICBitmapEncoder> {
        let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: The encoder thread has an initialized COM apartment, and
        // the path stays live for the call. Initialize retains its stream.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(wic_error)?;
            let stream = factory.CreateStream().map_err(wic_error)?;
            stream
                .InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)
                .map_err(wic_error)?;
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatPng, null())
                .map_err(wic_error)?;
            encoder
                .Initialize(&stream, WICBitmapEncoderNoCache)
                .map_err(wic_error)?;
            Ok(encoder)
        }
    }

    fn write_png(encoder: &IWICBitmapEncoder, frame: &Frame) -> Result<()> {
        // SAFETY: The native capture checked the BGRA buffer length and dimensions.
        unsafe {
            let mut image: Option<IWICBitmapFrameEncode> = None;
            let mut options = None;
            encoder
                .CreateNewFrame(&mut image, &mut options)
                .map_err(wic_error)?;
            let image = image.ok_or("WIC did not create an image frame")?;
            image.Initialize(options.as_ref()).map_err(wic_error)?;
            image
                .SetSize(frame.width, frame.height)
                .map_err(wic_error)?;
            let mut pixel_format = GUID_WICPixelFormat32bppBGRA;
            image.SetPixelFormat(&mut pixel_format).map_err(wic_error)?;
            if pixel_format != GUID_WICPixelFormat32bppBGRA {
                return Err("WIC PNG encoder rejected BGRA pixels".into());
            }
            image
                .WritePixels(frame.height, frame.width * 4, &frame.bgra)
                .map_err(wic_error)?;
            image.Commit().map_err(wic_error)
        }
    }

    fn with_artifact(destination: &Path, encode: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        let parent = destination
            .parent()
            .ok_or("capture output has no directory")?;
        fs::create_dir_all(parent).map_err(io_error)?;
        let stage = reserve_stage(parent)?;
        let result =
            encode(&stage).and_then(|()| fs::rename(&stage, destination).map_err(io_error));
        if result.is_err() {
            let _ = fs::remove_file(&stage);
        }
        result
    }

    fn reserve_stage(parent: &Path) -> Result<PathBuf> {
        loop {
            let id = NEXT_ARTIFACT.fetch_add(1, Ordering::Relaxed);
            let stage = parent.join(format!(".capture-{}-{id}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&stage) {
                Ok(_) => return Ok(stage),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    fn wic_error(error: windows::core::Error) -> String {
        format!("Windows Imaging Component: {error}")
    }

    fn io_error(error: std::io::Error) -> String {
        format!("write capture: {error}")
    }

    fn last_error(operation: &str) -> String {
        format!("{operation}: {}", std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_starts_immediately_and_ignores_early_ticks() {
        let mut clock = FrameClock::new(10).unwrap();
        assert_eq!(clock.due(Duration::ZERO), Ok(true));
        assert_eq!(clock.due(Duration::from_millis(99)), Ok(false));
        assert_eq!(clock.due(Duration::from_millis(105)), Ok(true));
        assert_eq!(clock.due(Duration::from_millis(200)), Ok(false));
        assert_eq!(clock.due(Duration::from_millis(205)), Ok(true));
        assert!(clock.due(Duration::from_millis(200)).is_err());
        assert!(
            clock
                .due(MAX_RECORDING_DURATION + Duration::from_millis(1))
                .is_err()
        );
        assert!(FrameClock::new(MAX_RECORDING_FPS + 1).is_err());
        assert!(FrameClock::new(0).is_err());
    }

    #[test]
    fn gif_coalesces_identical_frames_and_preserves_durations_and_looping() {
        let frame = |value| Frame {
            width: 2,
            height: 1,
            bgra: vec![value, 23, 180, 255, 92, value, 19, 255],
        };
        let (send, receive) = std::sync::mpsc::channel();
        let ms = Duration::from_millis;
        send.send(Sample::Frame(ms(0), frame(5))).unwrap();
        send.send(Sample::Frame(ms(100), frame(5))).unwrap();
        send.send(Sample::Frame(ms(250), frame(75))).unwrap();
        send.send(Sample::Finish(ms(600))).unwrap();
        let mut bytes = Vec::new();
        encode_gif(&mut bytes, receive).unwrap();
        let mut decoder = gif::DecodeOptions::new();
        decoder.set_color_output(gif::ColorOutput::RGBA);
        let mut gif = decoder.read_info(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!((gif.width(), gif.height()), (2, 1));
        assert_eq!(gif.repeat(), gif::Repeat::Infinite);
        let first = gif.read_next_frame().unwrap().unwrap();
        assert_eq!(first.delay, 25);
        let pixels = first.buffer.to_vec();
        let second = gif.read_next_frame().unwrap().unwrap();
        assert_eq!(second.delay, 35);
        assert_ne!(pixels, second.buffer.as_ref());
        assert!(gif.read_next_frame().unwrap().is_none());
    }

    #[test]
    fn gif_rounding_keeps_long_holds_and_does_not_drift() {
        let time = Duration::from_millis;
        assert_eq!(gif_delay(time(0), time(33)), 3);
        assert_eq!(gif_delay(time(33), time(67)), 4);
        assert_eq!(gif_delay(time(67), time(100)), 3);
        assert_eq!(gif_delay(time(100), time(1_600)), 150);
        assert_eq!(gif_delay(time(0), time(0)), 1);
    }
}
