//! Windows-specific platform adapters belong in this module.

pub(super) mod annotation_cursor;
pub(super) mod annotation_gpu;
pub(super) mod annotation_magnifier;
mod annotation_overlay;
mod annotation_window_order;
pub(super) mod app_icon;
mod autostart;
mod clipboard;
mod clipboard_selection;
mod d3d_compare;
mod d3d_query;
mod d3d_shader;
#[cfg(test)]
#[path = "annotation_gpu/gpu_timing.rs"]
mod gpu_timing;
mod hook_thread;
mod hotkey;
mod instance;
pub(crate) mod memory;
pub(crate) mod renderer;
pub(super) mod resize_background;
mod screen;
pub(super) mod screen_capture;
mod selection;
pub(crate) mod selection_monitor;
mod speech;
mod tray;

pub(crate) use autostart::WindowsAutostartPort;
pub(crate) use hotkey::WindowsHotkeyPort;
pub(crate) use instance::WindowsInstancePort;
pub(crate) use screen::WindowsScreenPort;
pub(crate) use selection::WindowsSelectionPort;
pub(crate) use speech::WindowsSpeechPort;
pub(crate) use tray::WindowsTrayPort;

pub(super) mod popup;
pub(crate) use clipboard::WindowsClipboardPort;
