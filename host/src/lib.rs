//! Native host for the Roc signals engine. The wire model is portable, while
//! the allocator ABI and HWNDs are confined to the Windows host.

// The Windows test harness replaces the Roc-linked entry point. Production
// builds still lint the native runtime with the entry point enabled.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(windows)]
mod accessibility;
#[cfg(windows)]
mod accessibility_abi;
#[cfg(windows)]
mod accessibility_grid;

#[cfg(windows)]
mod abi;
#[cfg(windows)]
mod app_resource;
#[cfg(windows)]
mod asset_catalog;
#[cfg(any(test, windows))]
mod capture;
#[cfg(windows)]
mod choice;
#[cfg(any(test, windows))]
mod civil;
#[cfg(any(test, windows))]
mod civil_calendar;
#[cfg(windows)]
mod civil_edit;
#[cfg(any(test, windows))]
mod clipboard;
#[cfg(windows)]
mod command_icon;
#[cfg(windows)]
mod display_preferences;
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
#[cfg(windows)]
mod picture;
#[cfg(any(test, windows))]
mod progress;
#[cfg(any(test, windows))]
mod raster;
#[cfg(windows)]
mod raster_bridge;
#[cfg(any(test, windows))]
mod regional;
#[cfg(windows)]
#[allow(clippy::all)]
mod roc_platform_abi;
#[cfg(windows)]
mod rooted;
#[cfg(windows)]
mod shader;
#[cfg(any(test, windows))]
mod shader_bytecode;
#[cfg(any(test, windows))]
mod slider;
#[cfg(any(test, windows))]
mod spec_effects;
#[cfg(any(test, windows))]
mod spec_plan;
#[cfg(any(test, windows))]
mod spec_regional;
#[cfg(any(test, windows))]
mod style;
#[cfg(any(test, windows))]
mod surface_input;
#[cfg(any(test, windows))]
mod taskbar;
#[cfg(any(test, windows))]
mod toolbar;
#[cfg(any(test, windows))]
mod tree_model;
#[cfg(windows)]
mod tree_view;
#[cfg(windows)]
mod typeface;
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
