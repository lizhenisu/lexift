//! Windows-specific platform adapters belong in this module.

pub(super) mod annotation_cursor;
pub(super) mod annotation_gpu;
pub(super) mod annotation_magnifier;
mod annotation_overlay;
pub(super) mod annotation_save;
pub(super) mod annotation_window_order;
pub(super) mod app_icon;
mod autostart;
pub(super) mod browser;
mod clipboard;
pub(crate) mod clipboard_history;
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
mod ocr_capture;
pub(super) mod pinned_image;
#[cfg(test)]
mod recording_gate;
pub(crate) mod renderer;
pub(super) mod resize_background;
mod screen;
pub(super) mod screen_capture;
pub(super) mod screen_interaction_window;
mod selection;
pub(crate) mod selection_monitor;
mod speech;
mod tray;

pub(crate) use autostart::WindowsAutostartPort;
pub(crate) use hotkey::WindowsHotkeyPort;
pub(crate) use instance::WindowsInstancePort;
pub(crate) use screen::{WindowsScreenPort, display_metrics as screenshot_display_metrics};
pub(crate) use selection::WindowsSelectionPort;
pub(crate) use speech::WindowsSpeechPort;
pub(crate) use tray::WindowsTrayPort;

pub(super) mod popup;
pub(crate) mod recording_window;
pub(crate) mod window_shell_policy;
pub(crate) use clipboard::WindowsClipboardPort;

#[cfg(debug_assertions)]
pub(crate) mod pin_diagnostic;

pub(crate) mod recording_capture;
pub(crate) mod recording_pixels;

pub(crate) mod screen_recording;
