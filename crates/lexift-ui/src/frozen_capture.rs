//! Pure sampling of immutable screenshot pixels; constructed/injected by App.
use lexift_core::{Result, domain::screen_snapshot::crop_frames, ports::screen_capture::*};
use std::sync::{Arc, Mutex};
pub struct FrozenCapture {
    frames: Arc<Vec<CapturedRegion>>,
    latest: Mutex<Option<Arc<ScreenCaptureFrame>>>,
}
impl FrozenCapture {
    pub fn new(frames: Arc<Vec<CapturedRegion>>) -> Self {
        Self {
            frames,
            latest: Mutex::new(None),
        }
    }
}
impl ScreenCapturePort for FrozenCapture {
    fn is_available(&self) -> bool {
        !self.frames.is_empty()
    }
    fn request(&self, request: ScreenCaptureRequest) -> Result<()> {
        let regions = request
            .regions
            .iter()
            .map(|b| crop_frames(&self.frames, *b))
            .collect::<Result<Vec<_>>>()?;
        *self.latest.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Arc::new(ScreenCaptureFrame {
                revision: request.revision,
                serial: request.revision,
                regions,
            }));
        Ok(())
    }
    fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
        Ok(self
            .latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone())
    }
    fn clear(&self) {
        self.latest.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::geometry::Rect;
    #[test]
    fn frozen_samples_follow_request_identity_and_clear_independently() {
        let b = Rect {
            left: -2,
            top: 0,
            right: 2,
            bottom: 2,
        };
        let frames = Arc::new(vec![CapturedRegion {
            bounds: b,
            pixels: vec![255; 32],
        }]);
        let a = FrozenCapture::new(frames.clone());
        let other = FrozenCapture::new(frames);
        a.request(ScreenCaptureRequest {
            revision: 9,
            regions: vec![b],
            paused_regions: vec![],
            excluded_windows: vec![],
        })
        .unwrap();
        assert_eq!(a.latest().unwrap().unwrap().revision, 9);
        assert!(other.latest().unwrap().is_none());
        a.clear();
        assert!(a.latest().unwrap().is_none());
        assert!(other.is_available());
    }
}
