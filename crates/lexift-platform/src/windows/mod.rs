//! Windows-specific platform adapters belong in this module.

mod autostart;
mod clipboard;
mod clipboard_selection;
mod hotkey;
mod instance;
mod screen;
mod selection;
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
