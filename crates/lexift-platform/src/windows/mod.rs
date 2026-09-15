//! Windows-specific platform adapters belong in this module.

mod clipboard_selection;
mod hotkey;
mod screen;
mod selection;
mod tray;

pub(crate) use hotkey::WindowsHotkeyPort;
pub(crate) use screen::WindowsScreenPort;
pub(crate) use selection::WindowsSelectionPort;
pub(crate) use tray::WindowsTrayPort;

pub(super) mod popup;
