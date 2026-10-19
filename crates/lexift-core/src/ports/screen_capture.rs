//! Asynchronous, clean desktop pixels for live annotation effects.
use crate::{Result, domain::geometry::Rect, ports::magnifier::WindowToken};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCaptureRequest {
    /// Identifies the requested geometry. Callers validate it before confirming a
    /// mask; older samples may supply partial feedback within the same capture epoch.
    pub revision: u64,
    pub regions: Vec<Rect>,
    /// UI-covered pixels retain their last successful sample until uncovered.
    pub paused_regions: Vec<Rect>,
    pub excluded_windows: Vec<WindowToken>,
}

#[derive(Clone, Debug)]
pub struct CapturedRegion {
    pub bounds: Rect,
    /// Premultiplied RGBA, tightly packed, in physical pixels.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ScreenCaptureFrame {
    pub revision: u64,
    /// Increases only when the captured pixels change.
    pub serial: u64,
    pub regions: Vec<CapturedRegion>,
}

/// Frozen pixels and window metadata owned by one screenshot preparation.
pub struct ScreenshotCapture {
    pub frame: Arc<ScreenCaptureFrame>,
    pub windows: Vec<crate::domain::window_candidate::WindowCandidate>,
}

pub trait ScreenCapturePort: Send + Sync {
    /// Schedule point-scoped accessibility work without waiting on the caller.
    /// Unsupported adapters complete with no elements, retaining whole-window selection.
    fn query_screenshot_elements(
        &self,
        query: crate::domain::element_candidate::ElementQuery,
        _cancelled: Arc<std::sync::atomic::AtomicBool>,
        completed: Box<dyn FnOnce(crate::domain::element_candidate::ElementQueryResult) + Send>,
    ) -> Result<()> {
        completed(crate::domain::element_candidate::ElementQueryResult {
            query,
            elements: vec![],
            leaf: false,
        });
        Ok(())
    }
    /// Retire only this screenshot's accessibility work, leaving pixel sampling untouched.
    fn cancel_screenshot_elements(&self, _session: u64) {}

    /// Snapshot window metadata alongside background screenshot capture. Unsupported
    /// adapters return no candidates; callers retain the existing manual selection.
    fn screenshot_windows(
        &self,
        _excluded: &[WindowToken],
    ) -> Result<Vec<crate::domain::window_candidate::WindowCandidate>> {
        Ok(Vec::new())
    }
    /// One-shot physical desktop capture, performed before selection overlays are shown.
    /// Independent instances must not disturb annotation sampling.
    fn freeze_desktop(&self) -> Result<Vec<CapturedRegion>> {
        Err(crate::Error::new("Desktop capture is unavailable"))
    }
    /// One-shot screenshot fast path after view suspension. None means exclusions cannot be honored.
    fn try_freeze_screenshot(
        &self,
        _request: &ScreenCaptureRequest,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<Arc<ScreenCaptureFrame>>> {
        Ok(None)
    }
    fn is_available(&self) -> bool;
    /// Replaces outstanding work; must not wait for GPU readback on the UI thread.
    fn request(&self, request: ScreenCaptureRequest) -> Result<()>;
    fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>>;
    /// Cancels sampling and releases capture sessions and retained screen pixels.
    fn clear(&self);
}
