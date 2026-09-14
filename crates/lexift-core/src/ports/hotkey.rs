use crate::Result;

pub trait HotkeyPort {
    fn register_translate_hotkey(&self) -> Result<()>;
}
