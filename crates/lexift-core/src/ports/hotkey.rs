use std::sync::Arc;

use crate::Result;

pub type HotkeyHandler = Arc<dyn Fn() + Send + Sync + 'static>;

pub trait HotkeyPort: Send + Sync {
    fn register_translate_hotkey(&self, handler: HotkeyHandler) -> Result<()>;
}
