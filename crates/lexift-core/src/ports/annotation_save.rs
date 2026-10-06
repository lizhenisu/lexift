//! Saving a frozen annotation image. The caller owns the asynchronous job.
use super::{magnifier::WindowToken, screen_capture::CapturedRegion};
use crate::{Error, Result};
use std::sync::{Arc, atomic::AtomicBool};

pub struct PngSaveRequest {
    /// Tight, premultiplied RGBA in physical desktop coordinates.
    pub image: CapturedRegion,
    pub owner: Option<WindowToken>,
    pub default_name: String,
    pub title: String,
}

impl PngSaveRequest {
    pub fn validate(&self) -> Result<()> {
        let width = i64::from(self.image.bounds.right) - i64::from(self.image.bounds.left);
        let height = i64::from(self.image.bounds.bottom) - i64::from(self.image.bounds.top);
        let pixels = width
            .checked_mul(height)
            .filter(|&n| n > 0 && n <= 40_000_000);
        if width <= 0
            || height <= 0
            || pixels.and_then(|n| usize::try_from(n * 4).ok()) != Some(self.image.pixels.len())
        {
            return Err(Error::new("Invalid annotation image"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    Cancelled,
}

pub trait AnnotationSavePort: Send + Sync {
    fn is_available(&self) -> bool;
    /// Blocking on a dedicated worker, never on the UI thread. Cancellation must
    /// close the dialog and release its native resources without touching new jobs.
    fn save_png(&self, request: PngSaveRequest, cancelled: Arc<AtomicBool>) -> Result<SaveOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geometry::Rect;
    #[test]
    fn image_validation_rejects_truncated_and_overflowing_images() {
        let mut request = PngSaveRequest {
            image: CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: -1,
                    right: 0,
                    bottom: 0,
                },
                pixels: vec![255; 8],
            },
            owner: None,
            default_name: String::new(),
            title: String::new(),
        };
        assert!(request.validate().is_ok());
        request.image.pixels.pop();
        assert!(request.validate().is_err());
        request.image.bounds.left = i32::MIN;
        request.image.bounds.right = i32::MAX;
        assert!(request.validate().is_err());
    }
}
