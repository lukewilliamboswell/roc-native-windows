use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::UISettings;
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DisplayPreferences {
    pub(crate) text_scale: f64,
    pub(crate) animations: bool,
}

pub(crate) struct DisplaySettings {
    subscription: Option<Subscription>,
    changed: Arc<AtomicBool>,
}

impl DisplaySettings {
    pub(crate) fn new() -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let subscription = Subscription::new(Arc::clone(&changed));
        Self {
            subscription,
            changed,
        }
    }

    pub(crate) fn changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn read(&self) -> DisplayPreferences {
        let text_scale = self
            .subscription
            .as_ref()
            .and_then(|sub| sub.settings.TextScaleFactor().ok())
            .filter(|&scale| scale.is_finite() && scale > 0.0)
            .unwrap_or(1.0);
        let mut animations = 1i32;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one Win32 BOOL.
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut animations as *mut i32).cast(),
                0,
            )
        };
        DisplayPreferences {
            text_scale,
            animations: animations != 0,
        }
    }
}

struct Subscription {
    settings: UISettings,
    token: Option<i64>,
    _apartment: Apartment,
}

impl Subscription {
    fn new(changed: Arc<AtomicBool>) -> Option<Self> {
        let apartment = Apartment::new()?;
        let settings = UISettings::new().ok()?;
        let token = settings
            .TextScaleFactorChanged(&TypedEventHandler::new(move |_, _| {
                // Windows can invoke this on a worker. The existing UI event pump
                // reads and applies settings, even if a window was replaced.
                changed.store(true, Ordering::Release);
                Ok(())
            }))
            .ok();
        Some(Self {
            settings,
            token,
            _apartment: apartment,
        })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            let _ = self.settings.RemoveTextScaleFactorChanged(token);
        }
    }
}

struct Apartment;

impl Apartment {
    fn new() -> Option<Self> {
        // SAFETY: The UI thread uses a single-threaded apartment, including COM dialogs.
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }
            .ok()
            .map(|()| Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Balances successful RoInitialize on this same UI thread.
        unsafe { RoUninitialize() };
    }
}
