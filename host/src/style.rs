use crate::Result;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Accent(Option<u32>);

impl Accent {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        if value.is_empty() {
            return Ok(Self::default());
        }
        let channels = value
            .split(',')
            .map(str::parse::<u8>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| "invalid window accent")?;
        let [red, green, blue] = channels[..] else {
            return Err("a window accent requires red, green and blue".into());
        };
        Ok(Self(Some(u32::from_le_bytes([red, green, blue, 0]))))
    }
}

#[cfg(windows)]
pub(crate) mod native {
    use super::Accent;
    use crate::Result;
    use crate::view::{wide, window_text};
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IREADINGLAYOUT};
    use windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::HiDpi::SystemParametersInfoForDpi;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    pub(crate) enum FontRole {
        #[default]
        Body,
        Caption,
        Heading,
        Title,
        Display,
        Monospace,
    }

    impl FontRole {
        const ALL: [Self; 6] = [
            Self::Body,
            Self::Caption,
            Self::Heading,
            Self::Title,
            Self::Display,
            Self::Monospace,
        ];

        pub(crate) fn read(value: &str) -> Self {
            match value {
                "caption" => Self::Caption,
                "heading" => Self::Heading,
                "title" => Self::Title,
                "display" => Self::Display,
                "monospace" => Self::Monospace,
                _ => Self::Body,
            }
        }

        fn metrics(self, mut font: LOGFONTW) -> LOGFONTW {
            let size = match self {
                Self::Caption => 90,
                Self::Heading => 122,
                Self::Title => 166,
                Self::Display => 280,
                _ => 100,
            };
            font.lfHeight = font.lfHeight * size / 100;
            if matches!(self, Self::Heading | Self::Title | Self::Display) {
                font.lfWeight = font.lfWeight.max(FW_SEMIBOLD as i32);
            }
            if self == Self::Monospace {
                font.lfPitchAndFamily = FIXED_PITCH;
                font.lfFaceName = [0; 32];
                let name = wide("Consolas");
                font.lfFaceName[..name.len()].copy_from_slice(&name);
            }
            font
        }
    }

    pub(crate) struct Fonts {
        handles: Vec<HFONT>,
        message_font: LOGFONTW,
    }

    impl Fonts {
        pub(crate) fn new(dpi: u32, text_scale: f64) -> Result<Self> {
            let mut metrics = NONCLIENTMETRICSW {
                cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                ..NONCLIENTMETRICSW::default()
            };

            // SAFETY: The struct size matches the Win32 API and each font is owned by Fonts.
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
                metrics.lfMessageFont.lfHeight =
                    (f64::from(metrics.lfMessageFont.lfHeight) * text_scale).round() as i32;
            }
            let mut fonts = Self {
                handles: Vec::with_capacity(FontRole::ALL.len()),
                message_font: metrics.lfMessageFont,
            };
            for role in FontRole::ALL {
                // SAFETY: GDI reads the role-specific logical metrics synchronously.
                unsafe {
                    let font = CreateFontIndirectW(&role.metrics(metrics.lfMessageFont));
                    if font.is_null() {
                        return Err(format!(
                            "cannot create Windows message font: {}",
                            std::io::Error::last_os_error()
                        ));
                    }
                    fonts.handles.push(font);
                }
            }
            Ok(fonts)
        }

        pub(crate) fn get(&self, role: FontRole) -> HFONT {
            self.handles[role as usize]
        }

        pub(crate) fn packaged_metrics(&self, role: FontRole) -> LOGFONTW {
            role.metrics(self.message_font)
        }
    }

    impl Drop for Fonts {
        fn drop(&mut self) {
            for &font in &self.handles {
                // SAFETY: The callers have discarded all views, or selected the new fonts.
                unsafe { DeleteObject(font) };
            }
        }
    }

    pub(crate) struct Palette {
        background: COLORREF,
        foreground: COLORREF,
        accent: COLORREF,
        high_contrast: bool,
        rtl: bool,
    }

    impl Palette {
        pub(crate) fn new(accent: Accent, window: HWND) -> Self {
            let mut contrast = HIGHCONTRASTW {
                cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
                ..HIGHCONTRASTW::default()
            };
            let mut argb = 0;
            let mut opaque = 0;
            let mut direction = [0; 2];
            // SAFETY: The out pointers reference the documented writable structures.
            unsafe {
                let high_contrast = SystemParametersInfoW(
                    SPI_GETHIGHCONTRAST,
                    contrast.cbSize,
                    (&mut contrast as *mut HIGHCONTRASTW).cast(),
                    0,
                ) != 0
                    && contrast.dwFlags & HCF_HIGHCONTRASTON != 0;
                let accent = accent.0.unwrap_or_else(|| {
                    if DwmGetColorizationColor(&mut argb, &mut opaque) >= 0 {
                        let [blue, green, red, _] = argb.to_le_bytes();
                        u32::from_le_bytes([red, green, blue, 0])
                    } else {
                        GetSysColor(COLOR_HIGHLIGHT)
                    }
                });
                GetLocaleInfoEx(null(), LOCALE_IREADINGLAYOUT, direction.as_mut_ptr(), 2);
                let style = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
                let (background, foreground) = if style & WS_EX_DLGMODALFRAME != 0 {
                    (COLOR_BTNFACE, COLOR_BTNTEXT)
                } else {
                    (COLOR_WINDOW, COLOR_WINDOWTEXT)
                };
                Self {
                    background: GetSysColor(background),
                    foreground: GetSysColor(foreground),
                    accent,
                    high_contrast,
                    rtl: direction[0] == b'1' as u16
                        || style & (WS_EX_LAYOUTRTL | WS_EX_RTLREADING) != 0,
                }
            }
        }

        pub(crate) fn background(&self) -> COLORREF {
            self.background
        }

        pub(crate) fn panel(&self, tinted: bool, outlined: bool) -> Paint {
            let background = if self.high_contrast {
                self.background
            } else if tinted {
                mix(self.background, self.accent, 13)
            } else {
                mix(self.background, self.foreground, 4)
            };
            let border = if outlined {
                Some(if self.high_contrast {
                    self.foreground
                } else {
                    readable(
                        mix(background, self.foreground, 28),
                        self.foreground,
                        background,
                        3.0,
                    )
                })
            } else {
                None
            };
            Paint {
                background,
                foreground: self.foreground,
                border,
                format: 0,
                panel: true,
            }
        }

        pub(crate) fn text(
            &self,
            background: COLORREF,
            tone: &str,
            align: &str,
            ellipsis: bool,
        ) -> Paint {
            let foreground = if self.high_contrast {
                self.foreground
            } else {
                let preferred = match tone {
                    "secondary" => mix(self.foreground, background, 34),
                    "accent" => self.accent,
                    _ => self.foreground,
                };
                readable(preferred, self.foreground, background, 4.5)
            };
            let horizontal = match align {
                "center" => DT_CENTER,
                "end" if !self.rtl => DT_RIGHT,
                "end" => DT_LEFT,
                _ if self.rtl => DT_RIGHT,
                _ => DT_LEFT,
            };
            let direction = if self.rtl { DT_RTLREADING } else { 0 };
            let lines = if ellipsis {
                DT_END_ELLIPSIS | DT_SINGLELINE
            } else {
                DT_WORDBREAK
            };
            Paint {
                background,
                foreground,
                border: None,
                format: horizontal | direction | lines | DT_NOPREFIX,
                panel: false,
            }
        }
    }

    fn mix(from: COLORREF, toward: COLORREF, percent: u16) -> COLORREF {
        let from = from.to_le_bytes();
        let toward = toward.to_le_bytes();
        u32::from_le_bytes(std::array::from_fn(|index| {
            ((u16::from(from[index]) * (100 - percent) + u16::from(toward[index]) * percent) / 100)
                as u8
        }))
    }

    fn luminance(color: COLORREF) -> f32 {
        let color = color.to_le_bytes();
        [0.2126, 0.7152, 0.0722]
            .into_iter()
            .zip(color)
            .map(|(weight, channel)| {
                let value = f32::from(channel) / 255.0;
                weight
                    * if value <= 0.04045 {
                        value / 12.92
                    } else {
                        ((value + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }

    fn readable(
        preferred: COLORREF,
        foreground: COLORREF,
        background: COLORREF,
        minimum: f32,
    ) -> COLORREF {
        let background_luminance = luminance(background);
        let passes = |candidate| {
            let candidate = luminance(candidate);
            (candidate.max(background_luminance) + 0.05)
                / (candidate.min(background_luminance) + 0.05)
                >= minimum
        };
        for percent in (0..=90).step_by(10) {
            let color = mix(preferred, foreground, percent);
            if passes(color) {
                return color;
            }
        }
        if passes(foreground) {
            foreground
        } else if passes(0) {
            0
        } else {
            0x00ff_ffff
        }
    }

    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    pub(crate) struct Paint {
        pub(crate) background: COLORREF,
        foreground: COLORREF,
        border: Option<COLORREF>,
        format: u32,
        panel: bool,
    }

    pub(crate) struct StyledWindow {
        hwnd: HWND,
        paint: Box<Paint>,
    }

    impl StyledWindow {
        pub(crate) fn new(hwnd: HWND, paint: Paint) -> Result<Self> {
            let mut paint = Box::new(paint);
            // SAFETY: The Box address stays live until the subclass is detached.
            if unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(styled_proc),
                    1,
                    (&mut *paint as *mut Paint) as usize,
                )
            } == 0
            {
                return Err("could not attach native style painter".into());
            }
            Ok(Self { hwnd, paint })
        }

        pub(crate) fn update(&mut self, paint: Paint) {
            if *self.paint != paint {
                *self.paint = paint;
                // SAFETY: The subclass is installed on this UI thread.
                unsafe { InvalidateRect(self.hwnd, null(), 0) };
            }
        }
    }

    impl Drop for StyledWindow {
        fn drop(&mut self) {
            // SAFETY: Removal is idempotent if WM_NCDESTROY already detached the subclass.
            unsafe { RemoveWindowSubclass(self.hwnd, Some(styled_proc), 1) };
        }
    }

    unsafe extern "system" fn styled_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        id: usize,
        data: usize,
    ) -> LRESULT {
        // SAFETY: Windows invokes this on the UI thread with valid message payloads.
        unsafe {
            match message {
                WM_NCDESTROY => {
                    RemoveWindowSubclass(hwnd, Some(styled_proc), id);
                }
                WM_ERASEBKGND => return 1,
                WM_NCHITTEST if (*(data as *const Paint)).panel => return HTTRANSPARENT as LRESULT,
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    draw(hwnd, dc, *(data as *const Paint));
                    EndPaint(hwnd, &paint);
                    return 0;
                }
                WM_PRINTCLIENT => {
                    draw(hwnd, wparam as HDC, *(data as *const Paint));
                    return 0;
                }
                _ => {}
            }
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
    }

    unsafe fn draw(hwnd: HWND, dc: HDC, paint: Paint) {
        if dc.is_null() {
            return;
        }
        // SAFETY: The DC is supplied by Win32 or by the recorder and its selected objects are restored.
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let brush = CreateSolidBrush(paint.background);
            if !brush.is_null() {
                FillRect(dc, &rect, brush);
                DeleteObject(brush);
            }
            if let Some(border) = paint.border {
                let brush = CreateSolidBrush(border);
                if !brush.is_null() {
                    FrameRect(dc, &rect, brush);
                    DeleteObject(brush);
                }
            }
            if !paint.panel {
                let prior = SaveDC(dc);
                if prior == 0 {
                    return;
                }
                let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
                if !font.is_null() {
                    SelectObject(dc, font);
                }
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, paint.foreground);
                let text = wide(&window_text(hwnd));
                DrawTextW(dc, text.as_ptr(), -1, &mut rect, paint.format);
                RestoreDC(dc, prior);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn accent_and_secondary_text_remain_readable_on_tinted_panels() {
            for background in [0x0020_2020, 0x00f0_f0f0] {
                for accent in [0, 0x00ff_ffff, 0x0000_80ff, 0x0080_00ff] {
                    let foreground = if background == 0x0020_2020 {
                        0x00ff_ffff
                    } else {
                        0
                    };
                    let palette = Palette {
                        background,
                        foreground,
                        accent,
                        high_contrast: false,
                        rtl: false,
                    };
                    let panel = palette.panel(true, true);
                    for tone in ["primary", "secondary", "accent"] {
                        let fg =
                            luminance(palette.text(panel.background, tone, "", false).foreground);
                        let bg = luminance(panel.background);
                        assert!((fg.max(bg) + 0.05) / (fg.min(bg) + 0.05) >= 4.5);
                    }
                }
            }
        }

        #[test]
        fn contrast_theme_replaces_accent_and_uses_paired_system_colors() {
            let palette = Palette {
                background: 0x0012_3456,
                foreground: 0x0065_4321,
                accent: 0x00ff_ffff,
                high_contrast: true,
                rtl: true,
            };
            let panel = palette.panel(true, true);
            let text = palette.text(panel.background, "accent", "start", false);
            assert_eq!(text.background, palette.background);
            assert_eq!(text.foreground, palette.foreground);
            assert_eq!(panel.border, Some(palette.foreground));
            assert_ne!(text.format & DT_RIGHT, 0);
            assert_ne!(text.format & DT_RTLREADING, 0);
        }
    }
}
