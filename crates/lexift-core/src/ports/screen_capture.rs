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

#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Native window identity and physical pointer anchor for one automatic scroll run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongScrollTarget {
    pub hit: WindowToken,
    pub root: WindowToken,
    pub process: u32,
    pub thread: u32,
    pub point: crate::domain::geometry::Point,
    pub dpi: u32,
}

/// Frozen pixels and window metadata owned by one screenshot preparation.
pub struct ScreenshotCapture {
    pub frame: Arc<ScreenCaptureFrame>,
    pub windows: Vec<crate::domain::window_candidate::WindowCandidate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPinCapture {
    pub window: crate::domain::window_candidate::WindowCandidate,
    pub selection: Rect,
}

/// Select the frontmost frozen window touching the region, then require containment.
pub fn window_pin_candidate(
    windows: &[crate::domain::window_candidate::WindowCandidate],
    selection: Rect,
) -> Option<WindowPinCapture> {
    let window = windows
        .iter()
        .copied()
        .filter(|w| {
            w.bounds.left < selection.right
                && w.bounds.right > selection.left
                && w.bounds.top < selection.bottom
                && w.bounds.bottom > selection.top
        })
        .min_by_key(|w| w.order)?;
    (selection.left >= window.bounds.left
        && selection.top >= window.bounds.top
        && selection.right <= window.bounds.right
        && selection.bottom <= window.bounds.bottom
        && selection.left < selection.right
        && selection.top < selection.bottom)
        .then_some(WindowPinCapture { window, selection })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPinPause {
    Minimized,
    OutsideClient,
    BufferLimit,
}

pub enum WindowPinUpdate {
    Frame(CapturedRegion),
    Pause(WindowPinPause),
}

pub trait ScreenCapturePort: Send + Sync {
    /// Runs on a dedicated background worker and publishes window-local immutable crops.
    fn run_window_pin(
        &self,
        _request: WindowPinCapture,
        _stop: &std::sync::atomic::AtomicBool,
        _publish: &mut dyn FnMut(WindowPinUpdate),
    ) -> std::result::Result<(), crate::domain::pinned_image::live::Failure> {
        Err(crate::domain::pinned_image::live::Failure::Unavailable)
    }

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
    fn supports_long_scroll(&self) -> bool {
        false
    }

    /// Observe a foreground target without moving the pointer or changing focus.
    fn long_scroll_target(&self, _bounds: Rect) -> Option<LongScrollTarget> {
        None
    }

    /// Check the current input target against the originally armed target.
    fn long_scroll_matches(&self, _bounds: Rect, _target: LongScrollTarget) -> bool {
        false
    }

    /// Revalidate the target and issue one downward wheel detent.
    fn long_scroll_step(&self, _bounds: Rect, _target: LongScrollTarget) -> Result<()> {
        Err(crate::Error::new("Automatic scrolling is unavailable"))
    }
    fn is_available(&self) -> bool;
    /// Replaces outstanding work; must not wait for GPU readback on the UI thread.
    fn request(&self, request: ScreenCaptureRequest) -> Result<()>;
    fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>>;
    /// Cancels sampling and releases capture sessions and retained screen pixels.
    fn clear(&self);
}

#[cfg(test)]
#[test]
fn window_pin_never_skips_a_partially_intersecting_front_window() {
    use crate::domain::window_candidate::WindowCandidate;
    use crate::ports::magnifier::WindowToken;
    let selection = Rect {
        left: -20,
        top: 4,
        right: 10,
        bottom: 18,
    };
    let back = WindowCandidate {
        window: WindowToken(1),
        process_id: 7,
        order: 1,
        bounds: Rect {
            left: -100,
            top: 0,
            right: 100,
            bottom: 80,
        },
    };
    let front = WindowCandidate {
        window: WindowToken(2),
        order: 0,
        bounds: Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 80,
        },
        ..back
    };
    assert!(window_pin_candidate(&[back, front], selection).is_none());
    assert_eq!(
        window_pin_candidate(&[back], selection).unwrap().window,
        back
    );
}
