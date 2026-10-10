//! Verified local PaddleOCR resources and CPU inference; no UI or translation policy.
mod inference;
mod resources;
use lexift_core::{
    Result,
    ports::{
        ocr::{DownloadProgress, OcrPort, OcrResources},
        screen_capture::CapturedRegion,
    },
};
use std::{
    path::PathBuf,
    sync::{Mutex, atomic::AtomicBool},
};

pub(crate) struct LocalOcr {
    root: PathBuf,
    engine: Mutex<Option<inference::Engine>>,
    download_lock: Mutex<()>,
}

impl LocalOcr {
    pub fn new() -> Result<Self> {
        let root = dirs::data_local_dir()
            .ok_or_else(|| lexift_core::Error::new("Local data directory is unavailable"))?
            .join("Lexift/ocr");
        Ok(Self {
            root,
            engine: Mutex::new(None),
            download_lock: Mutex::new(()),
        })
    }
}

impl OcrPort for LocalOcr {
    fn resources(&self) -> Result<OcrResources> {
        resources::status(&self.root)
    }
    fn download(&self, cancel: &AtomicBool, progress: DownloadProgress) -> Result<()> {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        resources::download(&self.root, cancel, progress)
    }
    fn recognize(
        &self,
        image: CapturedRegion,
        language: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let mut engine = self.engine.lock().unwrap_or_else(|p| p.into_inner());
        if engine.is_none() {
            let folder = resources::verified_directory(&self.root)?;
            *engine = Some(inference::Engine::new(&folder)?);
        }
        engine.as_mut().unwrap().recognize(image, language, cancel)
    }
    fn release(&self) {
        self.engine.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}
