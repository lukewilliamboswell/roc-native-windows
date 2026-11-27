//! Native host for the Roc signals engine. The wire model is portable, while
//! the allocator ABI and HWNDs are confined to the Windows host.

#[cfg(windows)]
mod abi;
#[cfg(any(test, windows))]
mod capture;
#[cfg(windows)]
mod choice;
#[cfg(any(test, windows))]
mod civil;
#[cfg(any(test, windows))]
mod clipboard;
#[cfg(windows)]
mod command_icon;
#[cfg(windows)]
mod effects;
#[cfg(windows)]
mod grid;
#[cfg(windows)]
mod grid_clipboard;
#[cfg(any(test, windows))]
mod list_view;
#[cfg(windows)]
mod modal;
#[cfg(any(test, windows))]
mod progress;
#[cfg(windows)]
#[allow(clippy::all)]
mod roc_platform_abi;
#[cfg(any(test, windows))]
mod spec_effects;
#[cfg(any(test, windows))]
mod spec_plan;
#[cfg(any(test, windows))]
mod taskbar;
#[cfg(any(test, windows))]
mod toolbar;
#[cfg(any(test, windows))]
mod tree_model;
#[cfg(windows)]
mod tree_view;
#[cfg(windows)]
mod view;
#[cfg(windows)]
mod window;
#[cfg(any(test, windows))]
mod window_chrome;
#[cfg(windows)]
mod window_icons;
#[cfg(any(test, windows))]
mod windows;
#[cfg(any(test, windows))]
pub(crate) mod wire;

#[cfg(any(test, windows))]
pub(crate) type Result<T> = std::result::Result<T, String>;

#[cfg(windows)]
pub(crate) enum Event {
    Turn {
        node: u32,
        handler: u32,
        payload: Vec<u8>,
    },
    Files {
        node: u32,
        handler: u32,
        record: Vec<u8>,
        handles: Vec<u64>,
    },
}
