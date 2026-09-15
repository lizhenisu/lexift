use std::sync::Arc;

use lexift_core::ports::{
    hotkey::HotkeyPort, screen::ScreenPort, selection::SelectionPort, tray::TrayPort,
};

#[cfg(feature = "mock")]
use crate::mock::MockSelectionPort;

/// Platform adapters selected by the application composition root.
pub struct PlatformCapabilities {
    selection: Option<Arc<dyn SelectionPort>>,
    hotkey: Option<Arc<dyn HotkeyPort>>,
    screen: Option<Arc<dyn ScreenPort>>,
    tray: Option<Arc<dyn TrayPort>>,
}

impl PlatformCapabilities {
    /// Creates a production capability set without implicit development adapters.
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            selection: Some(Arc::new(crate::windows::WindowsSelectionPort::new())),
            #[cfg(not(target_os = "windows"))]
            selection: None,
            #[cfg(target_os = "windows")]
            hotkey: Some(Arc::new(crate::windows::WindowsHotkeyPort::new())),
            #[cfg(not(target_os = "windows"))]
            hotkey: None,
            #[cfg(target_os = "windows")]
            screen: Some(Arc::new(crate::windows::WindowsScreenPort::new())),
            #[cfg(not(target_os = "windows"))]
            screen: None,
            #[cfg(target_os = "windows")]
            tray: Some(Arc::new(crate::windows::WindowsTrayPort::new())),
            #[cfg(not(target_os = "windows"))]
            tray: None,
        }
    }

    #[cfg(feature = "mock")]
    pub fn mock() -> Self {
        Self {
            selection: Some(Arc::new(MockSelectionPort)),
            hotkey: None,
            screen: None,
            tray: None,
        }
    }

    pub fn selection(&self) -> Option<Arc<dyn SelectionPort>> {
        self.selection.as_ref().map(Arc::clone)
    }

    pub fn hotkey(&self) -> Option<Arc<dyn HotkeyPort>> {
        self.hotkey.as_ref().map(Arc::clone)
    }

    pub fn screen(&self) -> Option<Arc<dyn ScreenPort>> {
        self.screen.as_ref().map(Arc::clone)
    }

    pub fn tray(&self) -> Option<Arc<dyn TrayPort>> {
        self.tray.as_ref().map(Arc::clone)
    }
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_capabilities_match_the_current_platform() {
        #[cfg(target_os = "windows")]
        assert!(PlatformCapabilities::new().selection().is_some());
        #[cfg(target_os = "windows")]
        assert!(PlatformCapabilities::new().screen().is_some());
        #[cfg(target_os = "windows")]
        assert!(PlatformCapabilities::new().tray().is_some());
        #[cfg(not(target_os = "windows"))]
        assert!(PlatformCapabilities::new().selection().is_none());
        #[cfg(not(target_os = "windows"))]
        assert!(PlatformCapabilities::new().screen().is_none());
        #[cfg(not(target_os = "windows"))]
        assert!(PlatformCapabilities::new().tray().is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_production_exposes_hotkey_and_selection() {
        let capabilities = PlatformCapabilities::new();
        assert!(capabilities.hotkey().is_some());
        assert!(capabilities.selection().is_some());
        assert!(capabilities.screen().is_some());
        assert!(capabilities.tray().is_some());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_production_has_no_hotkey_adapter() {
        assert!(PlatformCapabilities::new().hotkey().is_none());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_selection_requires_explicit_construction() {
        let capabilities = PlatformCapabilities::mock();
        assert!(capabilities.selection().is_some());
        assert!(capabilities.hotkey().is_none());
        assert!(capabilities.screen().is_none());
        assert!(capabilities.tray().is_none());
    }
}
