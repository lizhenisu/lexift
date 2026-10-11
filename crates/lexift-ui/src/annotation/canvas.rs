//! Per-monitor input surface, presentation state and cancellable positioning work.
use super::{frame::PresentedFrame, window};
use crate::AnnotationCanvas;
use crate::screen_interaction::Host;
use lexift_core::domain::geometry::Rect;
#[cfg(test)]
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bootstrap {
    Prepare(u8),
    Prepared,
    Present,
    Ready,
}

pub(super) struct Layer {
    pub last_pointer: Option<(f32, f32)>,
    pub corner_cursor_failed: Cell<bool>,
    pub(super) host: Host<AnnotationCanvas>,
    pub signature: Option<u64>,
    pub presented: PresentedFrame,
    pub gpu_input_cleared: bool,
    pub bootstrap: Bootstrap,
}

impl Layer {
    pub fn new(window: AnnotationCanvas, bounds: Rect) -> Self {
        Self {
            last_pointer: None,
            corner_cursor_failed: Cell::new(false),
            host: Host::new(window, bounds),
            signature: None,
            presented: PresentedFrame::default(),
            gpu_input_cleared: false,
            bootstrap: Bootstrap::Prepare(0),
        }
    }

    pub fn input(&self) -> Rc<window::InputWork> {
        self.host.input()
    }

    pub fn revision(&self) -> u64 {
        self.host.revision()
    }

    pub fn retire_input(&self) {
        self.host.retire_input();
    }

    pub fn position_after(&self, delay: Duration, position: impl FnMut() + 'static) {
        self.host.position_after(delay, position);
    }
}

impl std::ops::Deref for Layer {
    type Target = Host<AnnotationCanvas>;
    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn canvas_owner_cancels_positioning_even_when_old_component_is_retained() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let window = AnnotationCanvas::new().unwrap();
        let weak = window.as_weak();
        let retained = window.clone_strong();
        let bounds = Rect {
            left: -100,
            top: -50,
            right: 0,
            bottom: 50,
        };
        let layer = Layer::new(window, bounds);
        let revision = layer.revision();
        assert_eq!(layer.bootstrap, Bootstrap::Prepare(0));
        let input = layer.input();
        input.post(|| panic!("retired canvas must not run queued input"));
        layer.position_after(Duration::from_millis(32), || {
            panic!("retired canvas must not position")
        });
        drop(layer);
        assert!(input.retired() && !input.pending());
        input.post(|| panic!("retained component cannot requeue canvas input"));
        assert!(!input.pending());
        assert!(weak.upgrade().is_some());
        let new_layer = Layer::new(AnnotationCanvas::new().unwrap(), bounds);
        assert!(new_layer.revision() > revision);
        clock.set(Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
        drop(retained);
        assert!(weak.upgrade().is_none());
    }
}
