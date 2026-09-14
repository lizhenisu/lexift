//! Windows-specific platform adapters belong in this module.

mod clipboard_selection;
mod hotkey;
mod selection;

pub(crate) use hotkey::WindowsHotkeyPort;
pub(crate) use selection::WindowsSelectionPort;
