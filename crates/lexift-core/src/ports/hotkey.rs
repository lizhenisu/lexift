use std::sync::Arc;

use crate::{Result, domain::runtime_config::HotkeyConfig};

pub type HotkeyHandler = Arc<dyn Fn() + Send + Sync + 'static>;

pub trait HotkeyPort: Send + Sync {
    fn register_recording_hotkeys(
        &self,
        _scope: crate::domain::recording_hotkeys::Scope,
        _keys: [HotkeyConfig; 2],
        _handler: Arc<dyn Fn(crate::domain::recording_hotkeys::Action) + Send + Sync>,
    ) -> Result<crate::domain::recording_hotkeys::Registration> {
        Err(crate::Error::new("Recording shortcuts unavailable"))
    }
    fn unregister_recording_hotkeys(
        &self,
        _scope: crate::domain::recording_hotkeys::Scope,
    ) -> Result<()> {
        Ok(())
    }
    fn probe_recording_hotkey(&self, _config: HotkeyConfig) -> Result<()> {
        Err(crate::Error::new("Recording shortcuts unavailable"))
    }
    fn register_ocr_hotkey(&self, _config: HotkeyConfig, _handler: HotkeyHandler) -> Result<()> {
        Err(crate::Error::new(
            "OCR shortcut is unavailable on this platform",
        ))
    }
    fn replace_ocr_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler) -> Result<()> {
        self.register_ocr_hotkey(config, handler)
    }
    fn unregister_ocr_hotkey(&self) -> Result<()> {
        Ok(())
    }

    fn register_screenshot_hotkey(
        &self,
        _config: HotkeyConfig,
        _handler: HotkeyHandler,
    ) -> Result<()> {
        Err(crate::Error::new(
            "Screenshot shortcut is unavailable on this platform",
        ))
    }
    fn replace_screenshot_hotkey(
        &self,
        config: HotkeyConfig,
        handler: HotkeyHandler,
    ) -> Result<()> {
        self.register_screenshot_hotkey(config, handler)
    }
    fn unregister_screenshot_hotkey(&self) -> Result<()> {
        Ok(())
    }

    /// Registers the independent annotation toolbar shortcut.
    fn register_annotation_hotkey(
        &self,
        _config: HotkeyConfig,
        _handler: HotkeyHandler,
    ) -> Result<()> {
        Err(crate::Error::new(
            "Annotation shortcut is unavailable on this platform",
        ))
    }
    fn replace_annotation_hotkey(
        &self,
        _config: HotkeyConfig,
        _handler: HotkeyHandler,
    ) -> Result<()> {
        Err(crate::Error::new(
            "Annotation shortcut is unavailable on this platform",
        ))
    }
    fn unregister_annotation_hotkey(&self) -> Result<()> {
        Ok(())
    }
    fn register_translate_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler)
    -> Result<()>;
    fn replace_translate_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler) -> Result<()>;
    fn unregister_translate_hotkey(&self) -> Result<()>;
}
