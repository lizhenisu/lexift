use crate::Result;

pub trait ClipboardPort: Send + Sync {
    /// Copy a native image into owned premultiplied pixels without modifying the clipboard.
    fn read_image(&self) -> Result<Option<super::screen_capture::CapturedRegion>> {
        Err(crate::Error::new("Image clipboard is unavailable"))
    }

    /// Write a validated, premultiplied RGBA image without producing text history.
    fn write_image(&self, _image: &super::screen_capture::CapturedRegion) -> Result<()> {
        Err(crate::Error::new("Image clipboard is unavailable"))
    }
    fn read_text(&self) -> Result<Option<String>>;
    fn write_text(&self, text: &str) -> Result<()>;
    /// Excludes explicitly sensitive copies from clipboard history.
    fn write_sensitive_text(&self, text: &str) -> Result<()> {
        self.write_text(text)
    }
}
