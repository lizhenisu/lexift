use std::sync::Arc;

use crate::{Result, domain::runtime_config::HotkeyConfig};

pub type HotkeyHandler = Arc<dyn Fn() + Send + Sync + 'static>;

pub trait HotkeyPort: Send + Sync {
    /// Registers the independent, fixed annotation preview shortcut.
    fn register_annotation_hotkey(&self, _handler: HotkeyHandler) -> Result<()> {
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
