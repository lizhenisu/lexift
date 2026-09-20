use std::sync::Arc;

use crate::{Result, domain::runtime_config::HotkeyConfig};

pub type HotkeyHandler = Arc<dyn Fn() + Send + Sync + 'static>;

pub trait HotkeyPort: Send + Sync {
    fn register_translate_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler)
    -> Result<()>;
    fn replace_translate_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler) -> Result<()>;
    fn unregister_translate_hotkey(&self) -> Result<()>;
}
