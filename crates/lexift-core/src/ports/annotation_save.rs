//! Saving a frozen annotation image. The caller owns the asynchronous job.
use super::{magnifier::WindowToken, screen_capture::CapturedRegion};
use crate::{Error, Result};
use std::sync::{Arc, atomic::AtomicBool};

/// Supported file encodings. JPEG policy is intentionally fixed, not configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}
impl ImageFormat {
    pub const JPEG_QUALITY: f32 = 0.95;
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
    pub fn accepts_extension(self, extension: &str) -> bool {
        match self {
            Self::Png => extension.eq_ignore_ascii_case("png"),
            Self::Jpeg => {
                extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
            }
        }
    }
}

/// Immutable dialog policy and localized feedback; safe to send to a native worker.
#[derive(Clone, Debug)]
pub struct ImageSaveOptions {
    pub formats: Vec<ImageFormat>,
    pub default_format: ImageFormat,
    pub extension_mismatch_message: String,
}
impl ImageSaveOptions {
    pub fn png_and_jpeg(extension_mismatch_message: String) -> Self {
        Self {
            formats: vec![ImageFormat::Png, ImageFormat::Jpeg],
            default_format: ImageFormat::Png,
            extension_mismatch_message,
        }
    }
}

pub struct ImageSaveRequest {
    /// Tight, premultiplied RGBA in physical desktop coordinates.
    pub image: CapturedRegion,
    pub owner: Option<WindowToken>,
    pub default_name: String,
    pub title: String,
    pub options: ImageSaveOptions,
}

impl ImageSaveRequest {
    pub fn validate(&self) -> Result<()> {
        if self.options.formats.is_empty()
            || !self.options.formats.contains(&self.options.default_format)
            || self
                .options
                .formats
                .iter()
                .enumerate()
                .any(|(i, format)| self.options.formats[..i].contains(format))
        {
            return Err(Error::new("Invalid image save formats"));
        }
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
    fn save_image(
        &self,
        request: ImageSaveRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<SaveOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geometry::Rect;
    #[test]
    fn format_extensions_and_jpeg_policy() {
        assert!(ImageFormat::Png.accepts_extension("PNG"));
        assert!(ImageFormat::Jpeg.accepts_extension("JpEg"));
        assert!(ImageFormat::Jpeg.accepts_extension("JPG"));
        assert!(!ImageFormat::Jpeg.accepts_extension("png"));
        assert_eq!(ImageFormat::JPEG_QUALITY, 0.95);
    }
    #[test]
    fn image_validation_rejects_truncated_and_overflowing_images() {
        let mut request = ImageSaveRequest {
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
            options: ImageSaveOptions::png_and_jpeg(String::new()),
        };
        assert!(request.validate().is_ok());
        request.options.formats.clear();
        assert!(request.validate().is_err());
        request.options = ImageSaveOptions::png_and_jpeg(String::new());
        request.options.formats.push(ImageFormat::Png);
        assert!(request.validate().is_err());
        request.options = ImageSaveOptions::png_and_jpeg(String::new());
        request.image.pixels.pop();
        assert!(request.validate().is_err());
        request.image.bounds.left = i32::MIN;
        request.image.bounds.right = i32::MAX;
        assert!(request.validate().is_err());
    }
}
