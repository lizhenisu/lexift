//! Await the native size and Slint layout before rendering a transient hint.
use crate::ScreenshotToastWindow;
use lexift_core::domain::geometry::{Point, Rect};
use slint::{
    ComponentHandle, PhysicalPosition, PhysicalSize,
    winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent},
};
use std::{cell::Cell, rc::Rc, time::Duration};

pub(crate) enum Placement {
    AtPin(Point, Option<Rect>),
    BottomCenter(Rect),
}

fn current_size(
    dpi: f32,
    native_dpi: f32,
    native: PhysicalSize,
    surface: PhysicalSize,
    expected: PhysicalSize,
    layout: (f32, f32),
) -> bool {
    dpi == native_dpi
        && native == expected
        && surface == expected
        && (layout.0 * dpi).round() as u32 == expected.width
        && (layout.1 * dpi).round() as u32 == expected.height
        // SoftwareRenderer truncates scaled layout extents rather than rounding.
        && (layout.0 * dpi) as u32 <= expected.width
        && (layout.1 * dpi) as u32 <= expected.height
}

fn geometry(w: &ScreenshotToastWindow, placement: &Placement) -> bool {
    let window = w.window();
    let read_native = || {
        window
            .with_winit_window(|native| {
                let dims = native.inner_size();
                (
                    native.scale_factor() as f32,
                    PhysicalSize::new(dims.width, dims.height),
                )
            })
            .unwrap_or_else(|| (window.scale_factor(), window.size()))
    };
    let dpi = window.scale_factor();
    let (native_dpi, _) = read_native();
    if dpi != native_dpi || !dpi.is_finite() || dpi <= 0. {
        tracing::trace!(dpi, native_dpi, "Toast scale change pending");
        return false;
    }
    let size = PhysicalSize::new(
        (w.get_content_width() * dpi).round().max(1.) as u32,
        (w.get_content_height() * dpi).round().max(1.) as u32,
    );
    let position = match *placement {
        Placement::AtPin(origin, None) => PhysicalPosition::new(origin.x, origin.y),
        Placement::AtPin(origin, Some(area)) => PhysicalPosition::new(
            origin.x.clamp(
                area.left + 8,
                (area.right - size.width as i32 - 8).max(area.left + 8),
            ),
            origin.y.clamp(
                area.top + 8,
                (area.bottom - size.height as i32 - 8).max(area.top + 8),
            ),
        ),
        Placement::BottomCenter(area) => PhysicalPosition::new(
            area.left + ((area.right - area.left - size.width as i32) / 2).max(0),
            (area.bottom - size.height as i32 - (24. * dpi) as i32).max(area.top),
        ),
    };
    if window.position() != position {
        window.set_position(position);
    }
    let (native_dpi, native) = read_native();
    if dpi != native_dpi {
        return false;
    }
    if window.size() != size || native != size {
        window.set_size(size);
    }
    let (native_dpi, native) = read_native();
    let ready = current_size(
        dpi,
        native_dpi,
        native,
        window.size(),
        size,
        (w.get_layout_width(), w.get_layout_height()),
    ) && window.scale_factor() == dpi;
    tracing::trace!(
        ready, dpi, native_dpi, ?native, expected = ?size, surface = ?window.size(),
        logical_width = w.get_layout_width(), logical_height = w.get_layout_height(),
        "Toast geometry evaluated"
    );
    ready
}

/// Resume only on a native geometry event. A bounded deadline retires a hint
/// whose hidden window never acknowledged its requested size.
pub(crate) fn prepare(
    w: &ScreenshotToastWindow,
    placement: Placement,
    completed: impl Fn(bool) + 'static,
) {
    let done = Rc::new(Cell::new(false));
    let completed = Rc::new(completed);
    let pending = Rc::new(Cell::new(false));
    let weak = w.as_weak();
    let check = Rc::new({
        let done = done.clone();
        let completed = completed.clone();
        move || {
            if !done.get() && weak.upgrade().is_some_and(|w| geometry(&w, &placement)) {
                done.set(true);
                completed(true);
            }
        }
    });
    if w.window().has_winit_window() {
        let check = check.clone();
        let done = done.clone();
        w.window().on_winit_window_event(move |window, event| {
            let stale = matches!(event, WindowEvent::Resized(size)
                if window.with_winit_window(|w| w.inner_size()).is_some_and(|actual| actual != *size));
            if !done.get()
                && matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. })
                && !pending.replace(true)
            {
                let check = check.clone();
                let pending = pending.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    pending.set(false);
                    check();
                });
            }
            if stale { EventResult::PreventDefault } else { EventResult::Propagate }
        });
    }
    check();
    if !done.get() {
        let weak = w.as_weak();
        slint::Timer::single_shot(Duration::from_millis(350), move || {
            if !done.replace(true) && weak.upgrade().is_some() {
                tracing::debug!("Toast retired before native geometry became consistent");
                completed(false);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent as SlintEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };

    struct SoftwarePlatform;
    impl Platform for SoftwarePlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }

    #[test]
    fn toast_snapshot_waits_for_the_scaled_layout_and_buffer_to_agree() {
        slint::platform::set_platform(Box::new(SoftwarePlatform)).unwrap();
        let w = ScreenshotToastWindow::new().unwrap();
        w.set_message(
            "A long message that occupies several lines. "
                .repeat(32)
                .into(),
        );
        let window = w.window();
        window.set_size(PhysicalSize::new(360, 200));
        window.dispatch_event(SlintEvent::ScaleFactorChanged { scale_factor: 1.25 });
        let old_size = window.size();
        assert_eq!(old_size, PhysicalSize::new(360, 200));
        let layout = (w.get_layout_width(), w.get_layout_height());
        assert_eq!(layout, (360., 200.));
        assert!(!current_size(
            1.25,
            1.25,
            old_size,
            old_size,
            PhysicalSize::new(450, 250),
            layout
        ));
        let placement = Placement::AtPin(Point::default(), None);
        assert!(geometry(&w, &placement));
        let snapshot = window.take_snapshot().unwrap();
        assert_eq!((snapshot.width(), snapshot.height()), (450, 250));
        assert!(snapshot.as_slice().iter().any(|p| p.a > 0));

        window.dispatch_event(SlintEvent::ScaleFactorChanged { scale_factor: 1. });
        w.set_message("Short notice".into());
        // A delayed size from the prior monitor must not authorize the next frame.
        let stale_size = PhysicalSize::new(450, 250);
        let expected = PhysicalSize::new(360, w.get_content_height().round() as u32);
        assert!(!current_size(
            1.,
            1.25,
            stale_size,
            stale_size,
            expected,
            (w.get_layout_width(), w.get_layout_height()),
        ));
        assert!(geometry(&w, &placement));
        let smaller = window.take_snapshot().unwrap();
        assert_eq!(smaller.width(), 360);
        assert!(smaller.height() < 200);
        assert!(smaller.as_slice().iter().any(|p| p.a > 0));
    }
}
