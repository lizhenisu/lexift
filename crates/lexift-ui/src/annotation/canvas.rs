//! Per-monitor input surface, presentation state and cancellable positioning work.
use super::{frame::PresentedFrame, window};
use crate::AnnotationCanvas;
use crate::screen_interaction::Host;
use lexift_core::domain::geometry::Rect;
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bootstrap {
    Prepare(u8),
    Prepared,
    Present,
    Ready,
}

/// An editor input lease can borrow a pin without creating a second canvas HWND.
pub(super) enum Surface {
    Canvas(AnnotationCanvas),
    Pin(crate::PinnedScreenshotWindow),
}
impl Surface {
    #[cfg(test)]
    pub fn invoke_pointer(&self, event: i32, x: f32, y: f32) {
        match self {
            Self::Canvas(w) => w.invoke_pointer(event, x, y),
            Self::Pin(w) => w.invoke_edit_pointer(event, x, y),
        }
    }
    #[cfg(test)]
    pub fn invoke_escape_requested(&self) {
        match self {
            Self::Canvas(w) => w.invoke_escape_requested(),
            Self::Pin(w) => w.invoke_escape_requested(),
        }
    }
    #[cfg(test)]
    pub fn invoke_undo_requested(&self) {
        match self {
            Self::Canvas(w) => w.invoke_undo_requested(),
            Self::Pin(_) => super::undo(),
        }
    }

    pub fn window(&self) -> &slint::Window {
        match self {
            Self::Canvas(w) => w.window(),
            Self::Pin(w) => w.window(),
        }
    }
    pub fn clone_strong(&self) -> Self {
        match self {
            Self::Canvas(w) => Self::Canvas(w.clone_strong()),
            Self::Pin(w) => Self::Pin(w.clone_strong()),
        }
    }
    pub fn as_weak(&self) -> slint::Weak<AnnotationCanvas> {
        match self {
            Self::Canvas(w) => w.as_weak(),
            Self::Pin(_) => unreachable!("pins retain their own input bridge"),
        }
    }
    pub fn get_cursor(&self) -> crate::AnnotationCursor {
        match self {
            Self::Canvas(w) => w.get_cursor(),
            Self::Pin(w) => w.get_cursor(),
        }
    }
    pub fn set_cursor(&self, c: crate::AnnotationCursor) {
        match self {
            Self::Canvas(w) => w.set_cursor(c),
            Self::Pin(w) => w.set_cursor(c),
        }
    }
}
impl crate::screen_interaction::SurfaceHandle for Surface {
    fn surface_window(&self) -> &slint::Window {
        self.window()
    }
    fn surface_show(&self) -> Result<(), slint::PlatformError> {
        match self {
            Self::Canvas(w) => w.show(),
            Self::Pin(_) => Ok(()),
        }
    }
    fn surface_hide(&self) -> Result<(), slint::PlatformError> {
        match self {
            Self::Canvas(w) => w.hide(),
            Self::Pin(_) => Ok(()),
        }
    }
}
pub(super) struct Layer {
    native_input: Option<Rc<window::InputWork>>,
    source_scale: f32,
    pub last_pointer: Option<(f32, f32)>,
    pub corner_cursor_failed: Cell<bool>,
    pub(super) host: Host<Surface>,
    pub signature: Option<u64>,
    pub presented: PresentedFrame,
    pub gpu_input_cleared: bool,
    pub bootstrap: Bootstrap,
}

impl Layer {
    pub fn new(window: AnnotationCanvas, bounds: Rect) -> Self {
        Self {
            native_input: None,
            source_scale: 1.,
            last_pointer: None,
            corner_cursor_failed: Cell::new(false),
            host: Host::new(Surface::Canvas(window), bounds),
            signature: None,
            presented: PresentedFrame::default(),
            gpu_input_cleared: false,
            bootstrap: Bootstrap::Prepare(0),
        }
    }

    pub fn input(&self) -> Rc<window::InputWork> {
        self.host.input()
    }
    pub fn pin(
        window: crate::PinnedScreenshotWindow,
        bounds: Rect,
        scale: f32,
        input: Option<Rc<window::InputWork>>,
    ) -> Self {
        Self {
            native_input: input,
            source_scale: scale,
            last_pointer: None,
            corner_cursor_failed: Cell::new(false),
            host: Host::new(Surface::Pin(window), bounds),
            signature: None,
            presented: PresentedFrame::default(),
            gpu_input_cleared: false,
            bootstrap: Bootstrap::Ready,
        }
    }
    pub fn scale(&self) -> f32 {
        if matches!(self.window, Surface::Pin(_)) {
            return self.source_scale;
        }
        self.input_scale()
    }
    pub fn pause_pointer(&self) -> window::PointerPause {
        self.native_input
            .as_ref()
            .map_or_else(|| self.input().pause_pointer(), |i| i.pause_pointer())
    }
    pub fn view_zoom(&self) -> f32 {
        self.window.window().scale_factor() / self.input_scale()
    }
    pub fn client_inset(&self) -> f32 {
        match &self.window {
            Surface::Pin(w) => w.get_frame_inset(),
            Surface::Canvas(_) => 0.,
        }
    }
    pub fn input_scale(&self) -> f32 {
        match &self.window {
            Surface::Canvas(w) => w.window().scale_factor().max(0.1),
            Surface::Pin(w) => {
                let native = w.window().scale_factor().max(0.1);
                let width = w.window().size().width as f32 / native - 2. * w.get_frame_inset();
                (self.bounds.right - self.bounds.left) as f32 / width.max(1.)
            }
        }
    }
    pub fn display_bounds(&self) -> Rect {
        match &self.window {
            Surface::Canvas(_) => self.bounds,
            Surface::Pin(w) => {
                let p = w.window().position();
                let s = w.window().size();
                let inset = (w.get_frame_inset() * w.window().scale_factor()).round() as i32;
                Rect {
                    left: p.x + inset,
                    top: p.y + inset,
                    right: p.x + s.width as i32 - inset,
                    bottom: p.y + s.height as i32 - inset,
                }
            }
        }
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
    type Target = Host<Surface>;
    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    #[test]
    fn pin_surface_reuses_window_and_keeps_source_scale_independent_from_zoom() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Backend(Rc<Cell<u32>>);
        impl Platform for Backend {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                self.0.set(self.0.get() + 1);
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        let created = Rc::new(Cell::new(0));
        slint::platform::set_platform(Box::new(Backend(created.clone()))).unwrap();
        let window = crate::PinnedScreenshotWindow::new().unwrap();
        window.window().set_size(slint::PhysicalSize::new(216, 116));
        window.show().unwrap();
        let source = Rect {
            left: -600,
            top: -200,
            right: -200,
            bottom: 0,
        };
        let layer = Layer::pin(window.clone_strong(), source, 1.25, None);
        assert_eq!(
            created.get(),
            1,
            "editing must not create another canvas window"
        );
        assert_eq!(layer.scale(), 1.25);
        assert_eq!(layer.input_scale(), 2.);
        let p = crate::screen_interaction::screen_point(source, (25., 10.), layer.input_scale());
        assert_eq!(p, (-550., -180.));
        window.window().set_size(slint::PhysicalSize::new(416, 216));
        assert_eq!(layer.scale(), 1.25, "zoom must not rewrite document styles");
        assert_eq!(layer.input_scale(), 1.);
        drop(layer);
        assert!(
            window.window().is_visible(),
            "ending an input lease must retain its pin"
        );
        window.hide().unwrap();
    }

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
