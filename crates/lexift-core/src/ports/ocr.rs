//! Local OCR and its verified, downloadable resource set.
use crate::{Result, ports::screen_capture::CapturedRegion};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrResources {
    pub ready: bool,
    pub download_bytes: u64,
}

pub type DownloadProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub trait OcrPort: Send + Sync {
    fn resources(&self) -> Result<OcrResources>;
    fn download(&self, cancel: &AtomicBool, progress: DownloadProgress) -> Result<()>;
    /// Recognize locally; implementations must not log or persist image/text contents.
    fn recognize(
        &self,
        image: CapturedRegion,
        language: &str,
        cancel: &AtomicBool,
    ) -> Result<String>;
    /// Release inference sessions after the caller's cancellable idle period.
    fn release(&self);
}
