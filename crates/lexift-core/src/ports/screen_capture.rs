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

pub trait ScreenCapturePort: Send + Sync {
    fn is_available(&self) -> bool;
    /// Replaces outstanding work; must not wait for GPU readback on the UI thread.
    fn request(&self, request: ScreenCaptureRequest) -> Result<()>;
    fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>>;
    /// Cancels sampling and releases capture sessions and retained screen pixels.
    fn clear(&self);
}
