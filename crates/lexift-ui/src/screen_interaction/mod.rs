//! Instance-owned screen surfaces shared by annotation and region selection.
pub(crate) mod window;
use crate::{PassiveWindowPreparation, WindowLifecycleCallbacks};
use lexift_core::{domain::geometry::Rect, ports::screen_capture::CapturedRegion};
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc, time::Duration};

/// Create inside the backend attribute scope before a deferred HWND can exist.
pub(crate) fn create<C: ComponentHandle>(
    lifecycle: &WindowLifecycleCallbacks,
    mut factory: impl FnMut() -> Result<C, slint::PlatformError>,
) -> Result<C, slint::PlatformError> {
    let mut result = None;
    (lifecycle.screen_creation)(&mut || result = Some(factory()));
    result.expect("screen creation callback must invoke its factory exactly once")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Created,
    Prepared,
    Visible,
    Presented,
    Retired,
}

/// Owns view lifetime only; rendering and domain state remain with the caller.
pub(crate) struct Host<C: ComponentHandle> {
    pub window: C,
    pub bounds: Rect,
    input: Rc<window::InputWork>,
    revision: u64,
    placement: slint::Timer,
    phase: Cell<Phase>,
}
impl<C: ComponentHandle> Host<C> {
    pub fn new(window: C, bounds: Rect) -> Self {
        Self {
            window,
            bounds,
            input: window::InputWork::new(),
            revision: window::next_revision(),
            placement: Default::default(),
            phase: Cell::new(Phase::Created),
        }
    }
    pub fn input(&self) -> Rc<window::InputWork> {
        self.input.clone()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn retire_input(&self) {
        self.phase.set(Phase::Retired);
        self.input.retire();
        self.placement.stop();
    }
    pub fn position_after(&self, delay: Duration, work: impl FnMut() + 'static) {
        self.placement
            .start(slint::TimerMode::SingleShot, delay, work);
    }
    pub fn place(&self) -> bool {
        let b = self.bounds;
        let Some(width) = b.right.checked_sub(b.left).filter(|v| *v > 0) else {
            return false;
        };
        let Some(height) = b.bottom.checked_sub(b.top).filter(|v| *v > 0) else {
            return false;
        };
        self.window
            .window()
            .set_position(slint::PhysicalPosition::new(b.left, b.top));
        self.window
            .window()
            .set_size(slint::PhysicalSize::new(width as u32, height as u32));
        true
    }
    pub fn show(&self) -> Result<(), slint::PlatformError> {
        if self.phase.get() != Phase::Prepared {
            return Err("Screen host is not prepared".into());
        }
        self.window.show()?;
        self.phase.set(Phase::Visible);
        Ok(())
    }
    pub fn presented(&self) {
        if self.phase.get() == Phase::Visible {
            self.phase.set(Phase::Presented);
        }
    }
    pub fn prepare(
        &self,
        lifecycle: &WindowLifecycleCallbacks,
        frame: Option<&CapturedRegion>,
    ) -> PassiveWindowPreparation {
        if self.phase.get() == Phase::Retired || !self.place() {
            return PassiveWindowPreparation::Failed;
        }
        let status = (lifecycle.prepare_screen_window)(self.window.window(), self.bounds, frame);
        if status == PassiveWindowPreparation::Ready {
            self.phase.set(Phase::Prepared);
        }
        status
    }
}
impl<C: ComponentHandle> Drop for Host<C> {
    fn drop(&mut self) {
        self.retire_input();
        let _ = self.window.hide();
    }
}

/// Input arrives in client physical pixels; Slint callbacks use logical pixels.
pub(crate) fn screen_point(bounds: Rect, point: (f32, f32), scale: f32) -> (f32, f32) {
    (
        bounds.left as f32 + point.0 * scale,
        bounds.top as f32 + point.1 * scale,
    )
}
pub(crate) fn local_point(bounds: Rect, point: (f32, f32), scale: f32) -> (f32, f32) {
    (
        (point.0 - bounds.left as f32) / scale,
        (point.1 - bounds.top as f32) / scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_dpi_and_negative_origin_round_trip() {
        let bounds = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        for scale in [1., 1.25, 1.5] {
            let point = (123.5, 42.25);
            let physical = screen_point(bounds, point, scale);
            let local = local_point(bounds, physical, scale);
            assert!((local.0 - point.0).abs() < 0.001 && (local.1 - point.1).abs() < 0.001);
        }
    }
}
