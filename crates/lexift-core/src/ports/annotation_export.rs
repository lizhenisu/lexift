//! Pure export rendering. Implementations cannot access live views or capture devices.
use super::screen_capture::{CapturedRegion, ScreenCaptureFrame};
use crate::{
    Result,
    domain::{annotation_export::ExportSnapshot, geometry::Rect},
};
use std::sync::{Arc, atomic::AtomicBool};
pub trait AnnotationExportRenderPort: Send + Sync {
    fn regions(&self, snapshot: &ExportSnapshot) -> Vec<Rect>;
    fn compose(
        &self,
        snapshot: &ExportSnapshot,
        capture: Arc<ScreenCaptureFrame>,
        cancelled: &AtomicBool,
    ) -> Result<CapturedRegion>;
}
