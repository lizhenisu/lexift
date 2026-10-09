use crate::Result;

pub trait ClipboardPort: Send + Sync {
    fn read_text(&self) -> Result<Option<String>>;
    fn write_text(&self, text: &str) -> Result<()>;
    /// Excludes explicitly sensitive copies from clipboard history.
    fn write_sensitive_text(&self, text: &str) -> Result<()> {
        self.write_text(text)
    }
}
