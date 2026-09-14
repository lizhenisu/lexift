//! Windows-specific platform adapters belong in this module.

mod hotkey;
mod selection;

pub(crate) use hotkey::WindowsHotkeyPort;
pub(crate) use selection::WindowsSelectionPort;
