//! Local OCR and its verified, downloadable resource set.
use crate::{Result, ports::screen_capture::CapturedRegion};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OcrResourceKind {
    #[default]
    Base,
    Korean,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OcrResourceStatus {
    #[default]
    NotInstalled,
    Ready,
    Damaged,
    PendingRemoval,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrResourceEntry {
    pub id: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrResources {
    pub ready: bool,
    pub download_bytes: u64,
    pub status: OcrResourceStatus,
    pub version: String,
    pub installed_bytes: u64,
    pub entries: Vec<OcrResourceEntry>,
    pub korean: Option<Box<OcrResources>>,
}

pub type DownloadProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub trait OcrPort: Send + Sync {
    fn provider(&self) -> crate::domain::service_provider::ServiceProviderId {
        crate::domain::service_provider::ServiceProviderId::PaddleOcr
    }
    fn resources(&self) -> Result<OcrResources>;
    /// Remove only this provider's resources; loaded runtimes defer removal to startup.
    fn remove_resources(&self) -> Result<OcrResources>;
    fn download(&self, cancel: &AtomicBool, progress: DownloadProgress) -> Result<()>;
    fn download_kind(
        &self,
        kind: OcrResourceKind,
        cancel: &AtomicBool,
        progress: DownloadProgress,
    ) -> Result<()> {
        match kind {
            OcrResourceKind::Base => self.download(cancel, progress),
            OcrResourceKind::Korean => Err(crate::Error::new("Korean extension is unavailable")),
        }
    }
    fn remove_kind(&self, kind: OcrResourceKind) -> Result<OcrResources> {
        match kind {
            OcrResourceKind::Base => self.remove_resources(),
            OcrResourceKind::Korean => Err(crate::Error::new("Korean extension is unavailable")),
        }
    }
    /// Drop the optional reader after its current operation; never interrupt inference.
    fn release_korean(&self) {
        self.release();
    }
    /// Recognize locally; implementations must not log or persist image/text contents.
    fn recognize(
        &self,
        image: CapturedRegion,
        strategy: &crate::domain::ocr::OcrRecognitionStrategy,
        cancel: &AtomicBool,
    ) -> Result<String>;
    /// Release inference sessions after the caller's cancellable idle period.
    fn release(&self);
}
