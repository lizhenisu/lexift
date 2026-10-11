//! Verified local PaddleOCR resources and CPU inference; no UI or translation policy.
mod inference;
mod model_policy;
mod recognition;
mod resources;
use lexift_core::{
    Result,
    ports::{
        ocr::{DownloadProgress, OcrPort, OcrResourceKind, OcrResources},
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
    verified: Mutex<Option<Verified>>,
    #[cfg(test)]
    full_checks: std::sync::atomic::AtomicUsize,
}

type Stamp = Vec<(PathBuf, u64, Option<std::time::SystemTime>)>;
struct Verified {
    stamp: Stamp,
    info: OcrResources,
}

impl LocalOcr {
    pub fn new() -> Result<Self> {
        let root = dirs::data_local_dir()
            .ok_or_else(|| lexift_core::Error::new("Local data directory is unavailable"))?
            .join("Lexift/ocr");
        tracing::info!(resource_root = %root.display(), "OCR resource directory selected");
        if let Err(error) = resources::cleanup_pending(&root) {
            tracing::warn!(%error, "OCR startup removal remains pending");
        }
        Ok(Self {
            root,
            engine: Mutex::new(None),
            download_lock: Mutex::new(()),
            verified: Mutex::new(None),
            #[cfg(test)]
            full_checks: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    /// Caller holds the resource operation lock; cache lifetime never outlives release.
    fn inspected(&self) -> Result<OcrResources> {
        let stamp = resources::fingerprint(&self.root)?;
        let mut cache = self.verified.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = cache.as_ref() {
            if v.stamp == stamp {
                return Ok(v.info.clone());
            }
            self.engine.lock().unwrap_or_else(|p| p.into_inner()).take();
        }
        #[cfg(test)]
        self.full_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let info = resources::status(&self.root)?;
        *cache = Some(Verified {
            stamp,
            info: info.clone(),
        });
        Ok(info)
    }
    fn invalidate(&self) {
        self.verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        self.engine.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

impl OcrPort for LocalOcr {
    fn resources(&self) -> Result<OcrResources> {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        let status = self.inspected()?;
        tracing::info!(resource_root = %self.root.display(), status = ?status.status, installed_bytes = status.installed_bytes, "OCR resources inspected");
        Ok(status)
    }
    fn remove_resources(&self) -> Result<OcrResources> {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.invalidate();
        resources::remove(
            &self.root,
            inference::RUNTIME_LOADED.load(std::sync::atomic::Ordering::Acquire),
        )
    }
    fn download(&self, cancel: &AtomicBool, progress: DownloadProgress) -> Result<()> {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.invalidate();
        resources::download(&self.root, cancel, progress)
    }
    fn recognize(
        &self,
        image: CapturedRegion,
        strategy: &lexift_core::domain::ocr::OcrRecognitionStrategy,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(lexift_core::Error::new("OCR recognition: cancelled"));
        }
        let info = self.inspected()?;
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(lexift_core::Error::new("OCR recognition: cancelled"));
        }
        if !info.ready {
            return Err(lexift_core::Error::new("OCR resources are unavailable"));
        }
        if strategy.requires_korean() && !info.korean.as_deref().is_some_and(|r| r.ready) {
            return Err(lexift_core::Error::new("Korean extension is unavailable"));
        }
        let folder = self.root.join(&info.version);
        let mut engine = self.engine.lock().unwrap_or_else(|p| p.into_inner());
        if engine.is_none() {
            *engine = Some(inference::Engine::new(&folder)?);
        }
        let engine = engine.as_mut().unwrap();
        engine.korean_folder = self.root.join("korean-v1");
        if !strategy.requires_korean() {
            engine.release_korean();
        }
        engine.recognize(image, strategy, cancel)
    }
    fn download_kind(
        &self,
        kind: OcrResourceKind,
        cancel: &AtomicBool,
        progress: DownloadProgress,
    ) -> Result<()> {
        if kind == OcrResourceKind::Base {
            return self.download(cancel, progress);
        }
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(engine) = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            engine.release_korean();
        }
        resources::download_korean(&self.root, cancel, progress)
    }
    fn remove_kind(&self, kind: OcrResourceKind) -> Result<OcrResources> {
        if kind == OcrResourceKind::Base {
            return self.remove_resources();
        }
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(engine) = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            engine.release_korean();
        }
        resources::remove_korean(&self.root)?;
        self.inspected()
    }
    fn release_korean(&self) {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(engine) = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            engine.release_korean();
        }
    }
    fn release(&self) {
        let _guard = self.download_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.invalidate();
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::Ordering;
    #[test]
    fn verification_cache_is_invalidated_by_file_changes_and_release() {
        let root = std::env::temp_dir().join(format!(
            "lexift-ocr-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let port = LocalOcr {
            root: root.clone(),
            engine: Mutex::new(None),
            download_lock: Mutex::new(()),
            verified: Mutex::new(None),
            full_checks: std::sync::atomic::AtomicUsize::new(0),
        };
        port.resources().unwrap();
        port.resources().unwrap();
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 1);
        std::fs::create_dir_all(root.join("v1")).unwrap();
        std::fs::write(root.join("v1/det.onnx"), b"damaged").unwrap();
        assert!(!port.resources().unwrap().ready);
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 2);
        port.release();
        port.resources().unwrap();
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 3);
        let cancelled = AtomicBool::new(true);
        assert!(
            port.recognize(
                lexift_core::ports::screen_capture::CapturedRegion {
                    bounds: Default::default(),
                    pixels: vec![],
                },
                &lexift_core::domain::ocr::OcrRecognitionStrategy::Automatic,
                &cancelled,
            )
            .is_err()
        );
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 3);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires verified split resource fixture"]
    fn real_cache_and_optional_reader_lifetimes() {
        use lexift_core::{
            domain::{geometry::Rect, ocr::OcrRecognitionStrategy},
            ports::screen_capture::CapturedRegion,
        };
        let root = PathBuf::from(std::env::var_os("LEXIFT_OCR_RESOURCE_ROOT").unwrap());
        let port = LocalOcr {
            root,
            engine: Mutex::new(None),
            download_lock: Mutex::new(()),
            verified: Mutex::new(None),
            full_checks: std::sync::atomic::AtomicUsize::new(0),
        };
        let rgba = image::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr/common.png"),
        )
        .unwrap()
        .into_rgba8();
        let input = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: rgba.width() as i32,
                bottom: rgba.height() as i32,
            },
            pixels: rgba.into_raw(),
        };
        let cancel = AtomicBool::new(false);
        let check = std::time::Instant::now();
        port.resources().unwrap();
        let cold_check = check.elapsed().as_micros();
        let check = std::time::Instant::now();
        port.resources().unwrap();
        let hot_check = check.elapsed().as_micros();
        for _ in 0..3 {
            port.recognize(input.clone(), &OcrRecognitionStrategy::Automatic, &cancel)
                .unwrap();
        }
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 1);
        assert!(
            !port
                .engine
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .readers
                .contains_key("korean")
        );
        port.recognize(
            input.clone(),
            &OcrRecognitionStrategy::AutomaticKorean,
            &cancel,
        )
        .unwrap();
        assert!(
            port.engine
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .readers
                .contains_key("korean")
        );
        port.release_korean();
        assert!(
            !port
                .engine
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .readers
                .contains_key("korean")
        );
        assert!(
            port.engine
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .readers
                .contains_key("universal")
        );
        port.release();
        port.resources().unwrap();
        assert_eq!(port.full_checks.load(Ordering::Relaxed), 2);
        std::fs::write(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ocr-extension-validation/verification.json"),
            format!("{{\"cold_us\":{cold_check},\"hot_us\":{hot_check}}}"),
        )
        .unwrap();
    }
}
