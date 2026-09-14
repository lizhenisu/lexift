use crate::Result;

pub trait ClipboardPort {
    fn read_text(&self) -> Result<Option<String>>;
    fn write_text(&self, text: &str) -> Result<()>;
}
