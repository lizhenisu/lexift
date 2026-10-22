//! One instance-owned input path for native and Slint image viewport events.
use super::image_viewport::Viewport;
use super::*;
type PanCursor = Rc<dyn Fn(&slint::Window, Option<bool>) -> bool>;

struct Navigation {
    window: slint::Weak<AnnotationCanvas>,
    view: Viewport,
    route: u8, // idle, drawing, window chrome, panning, cancelled pan awaiting release
    space: bool,
    modifiers: bool,
    forwarding: bool,
    last_tool: Option<usize>,
    cursor: Option<PanCursor>,
    cursor_active: bool,
    over: bool,
}
thread_local! { static NAV: RefCell<Option<Navigation>> = const { RefCell::new(None) }; }
fn matches(n: &Navigation, w: &AnnotationCanvas) -> bool {
    n.window
        .upgrade()
        .is_some_and(|owner| std::ptr::eq(owner.window(), w.window()))
}
fn size(w: &AnnotationCanvas) -> [f32; 2] {
    let s = w.window().size();
    let dpi = w.window().scale_factor();
    [
        s.width as f32,
        (s.height as f32 - (w.get_image_header_height() + w.get_image_footer_height()) * dpi)
            .max(1.),
    ]
}
fn publish(n: &mut Navigation, w: &AnnotationCanvas) {
    let dpi = w.window().scale_factor();
    w.set_image_pixel_scale(dpi);
    w.set_image_zoom(n.view.scale / dpi);
    w.set_image_pan_x((n.view.offset[0] + w.get_source_left() * n.view.scale) / dpi);
    w.set_image_pan_y((n.view.offset[1] + w.get_source_top() * n.view.scale) / dpi);
    w.set_image_panning(n.view.panning());
    if let Some(cursor) = &n.cursor {
        let active =
            !w.get_image_cropping() && w.get_image_browsing() && (n.over || n.view.panning());
        if active || n.cursor_active {
            cursor(w.window(), active.then_some(n.view.panning()));
        }
        n.cursor_active = active;
    }
}
impl Drop for Navigation {
    fn drop(&mut self) {
        if self.cursor_active
            && let (Some(cursor), Some(w)) = (&self.cursor, self.window.upgrade())
        {
            cursor(w.window(), None);
        }
    }
}
pub(super) fn attach_cursor(cursor: PanCursor) {
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut() {
            n.cursor = Some(cursor);
        }
    });
}
pub(super) fn clear() {
    image_crop::clear();
    NAV.with(|s| s.borrow_mut().take());
}
pub(super) fn bind(w: &AnnotationCanvas) {
    image_crop::clear();
    let image = w.get_frame().size();
    NAV.with(|s| {
        *s.borrow_mut() = Some(Navigation {
            window: w.as_weak(),
            view: Viewport::new([image.width as f32, image.height as f32], size(w)),
            route: 0,
            space: false,
            modifiers: false,
            forwarding: false,
            last_tool: None,
            cursor: None,
            cursor_active: false,
            over: false,
        })
    });
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut() {
            n.view.content = [
                w.get_source_left(),
                w.get_source_top(),
                w.get_source_left() + image.width as f32,
                w.get_source_top() + image.height as f32,
            ];
        }
    });
    let weak = w.as_weak();
    w.on_fit_image(move || {
        if let Some(w) = weak.upgrade() {
            fit(&w);
        }
    });
    let weak = w.as_weak();
    w.on_viewport_resized(move || {
        if let Some(w) = weak.upgrade() {
            resize(&w);
        }
    });
    let weak = w.as_weak();
    w.on_viewport_pointer(move |kind, x, y| {
        if let Some(w) = weak.upgrade() {
            if !slint_input(&w) {
                return;
            }
            let dpi = w.window().scale_factor();
            pointer(&w, kind, [x * dpi, y * dpi]);
        }
    });
    let weak = w.as_weak();
    w.on_viewport_scroll(move |x, y, dx, dy, modified| {
        if let Some(w) = weak.upgrade() {
            if !slint_input(&w) {
                return;
            }
            scroll(&w, [x, y], dx, dy, modified);
        }
    });
    fit(w);
}
fn slint_input(w: &AnnotationCanvas) -> bool {
    NAV.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|n| matches(n, w) && !n.forwarding)
    })
}
pub(super) fn fit(w: &AnnotationCanvas) {
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
            n.view.resize(size(w));
            n.view.fit();
            publish(n, w);
        }
    });
}
pub(super) fn resize(w: &AnnotationCanvas) {
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
            n.view.resize(size(w));
            publish(n, w);
        }
    });
}
/// Run after the backend has applied a DPI event; native paint notifications may arrive earlier.
pub(super) fn refresh_geometry() {
    let window = NAV.with(|s| s.borrow().as_ref().and_then(|n| n.window.upgrade()));
    if let Some(w) = window {
        resize(&w);
    }
}
pub(super) fn content(w: &AnnotationCanvas, bounds: Rect, fit: bool) {
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
            n.view.content = [
                (bounds.left as f32)
                    .clamp(w.get_source_left(), w.get_source_left() + n.view.image[0]),
                (bounds.top as f32).clamp(w.get_source_top(), w.get_source_top() + n.view.image[1]),
                (bounds.right as f32)
                    .clamp(w.get_source_left(), w.get_source_left() + n.view.image[0]),
                (bounds.bottom as f32)
                    .clamp(w.get_source_top(), w.get_source_top() + n.view.image[1]),
            ];
            n.view.resize(size(w));
            if fit {
                n.view.fit();
            }
            publish(n, w);
        }
    });
}
fn blocked() -> bool {
    REGISTRY.with(|s| {
        let Ok(s) = s.try_borrow() else {
            return true;
        };
        s.as_ref().is_some_and(|r| {
            r.editor.window().is_some()
                || r.toolbar_state.watermark.get().is_some()
                || r.toolbar_state.menus.current.is_some()
                || r.toolbar_state.toolbar_drag.is_some()
                || screenshot::recognizing()
                || screenshot::long_editor::outputting()
        })
    })
}
fn document_gesture() -> bool {
    REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.gesture().is_some() || !r.pending_drawing.is_none())
    })
}
/// Coordinates here are physical pixels relative to the image viewport, never the source image.
pub(super) fn pointer(w: &AnnotationCanvas, kind: i32, point: [f32; 2]) -> bool {
    if kind == 0 || kind == 1 {
        REGISTRY.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut()
                && let Some(r) = s.as_mut()
            {
                hints::clear_image_editor(r);
            }
        });
    }
    if blocked() && kind != 3 {
        return true;
    }
    if w.get_image_cropping() {
        let Some(q) = map(w, point, true) else {
            return true;
        };
        let tolerance = 6. / f64::from((w.get_image_zoom() * w.window().scale_factor()).max(0.001));
        return image_crop::pointer(w, kind, q, tolerance);
    }
    let mut drawing = None;
    let handled = NAV.with(|s| {
        let mut s = s.borrow_mut();
        let Some(n) = s.as_mut().filter(|n| matches(n, w)) else {
            return false;
        };
        match kind {
            0 => {
                if n.route != 0 {
                    return true;
                }
                if w.get_image_browsing() {
                    n.view.begin_pan(point);
                    n.route = 3;
                } else if n.view.contains(point) {
                    n.route = 1;
                    drawing = Some((0, n.view.source(point)));
                }
            }
            1 => {
                if n.route == 3 {
                    n.view.pan_to(point);
                } else if n.route == 1
                    || (n.route == 0 && !w.get_image_browsing() && n.view.contains(point))
                {
                    drawing = Some((1, n.view.source(point)));
                }
            }
            2 | 3 => {
                if n.route == 1 {
                    drawing = Some((kind, n.view.source(point)));
                }
                n.view.finish_pan();
                n.route = 0;
            }
            4 if n.route == 0 && !w.get_image_browsing() => {
                drawing = Some((4, [0.; 2]));
            }
            _ => {}
        }
        n.over = kind != 4 && (0..2).all(|i| point[i] >= 0. && point[i] < n.view.size[i]);
        publish(n, w);
        true
    });
    if let Some((kind, q)) = drawing {
        w.invoke_image_pointer(kind, q[0], q[1]);
    }
    handled
}
pub(super) fn scroll(w: &AnnotationCanvas, point: [f32; 2], dx: f32, dy: f32, modified: bool) {
    if blocked() || modified || document_gesture() || image_crop::dragging() {
        return;
    }
    let mut brush = false;
    NAV.with(|s| {
        let mut s = s.borrow_mut();
        let Some(n) = s.as_mut().filter(|n| matches(n, w)) else {
            return;
        };
        if n.modifiers || n.route != 0 || dx != 0. {
            return;
        }
        if w.get_image_browsing() || w.get_image_cropping() {
            let dpi = w.window().scale_factor();
            if n.view.wheel(point.map(|p| p * dpi), dy) {
                publish(n, w);
            }
        } else {
            brush = w.get_cursor() == crate::AnnotationCursor::Brush;
        }
    });
    if brush {
        w.invoke_brush_size_scrolled(dy);
    }
}
pub(super) fn sync_mode(r: &Registry) {
    NAV.with(|s| {
        let mut s = s.borrow_mut();
        let Some(n) = s.as_mut() else {
            return;
        };
        let Some(w) = n.window.upgrade() else {
            return;
        };
        if !r
            .canvas_state
            .canvases
            .iter()
            .any(|c| std::ptr::eq(c.window.window(), w.window()))
        {
            return;
        }
        if let InteractionMode::Tool(tool) = r.document.mode {
            n.last_tool = Some(tool);
        }
        w.set_image_browsing(r.document.mode.is_mouse());
        publish(n, &w);
    });
}
pub(super) fn escape() -> bool {
    NAV.with(|s| {
        let mut s = s.borrow_mut();
        let Some(n) = s.as_mut() else {
            return false;
        };
        if n.route == 4 {
            return true;
        }
        if !n.view.cancel_pan() {
            return false;
        }
        n.route = 4;
        if let Some(w) = n.window.upgrade() {
            publish(n, &w);
        }
        true
    })
}
pub(super) fn lost_focus() {
    image_crop::lost_focus();
    let drawing = NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut() {
            n.space = false;
            n.modifiers = false;
            let drawing = n.route == 1;
            n.route = 0;
            n.view.finish_pan();
            if let Some(w) = n.window.upgrade() {
                publish(n, &w);
            }
            return drawing.then(|| n.window.upgrade()).flatten();
        }
        None
    });
    if let Some(w) = drawing {
        w.invoke_image_pointer(3, 0., 0.);
    }
}
pub(super) fn modifiers(value: bool) {
    NAV.with(|s| {
        if let Some(n) = s.borrow_mut().as_mut() {
            n.modifiers = value;
        }
    });
}
/// Only unconsumed root keys reach this function; caller validates window/session and foreground.
pub(super) fn key(text: &str, down: bool, control: bool, shift: bool, repeat: bool) -> bool {
    let space = text == " " && !control && !shift;
    let fit = text == "0" && control && !shift;
    if !space && !fit {
        return false;
    }
    if image_crop::active() {
        return true;
    }
    if blocked() {
        return false;
    }
    let drawing = document_gesture();
    let mut target = None;
    let accepted = NAV.with(|s| {
        let mut s = s.borrow_mut();
        let Some(n) = s.as_mut() else {
            return false;
        };
        if space && !down {
            n.space = false;
            return true;
        }
        if !down || repeat || (space && n.space) {
            return true;
        }
        if space {
            n.space = true;
        }
        if drawing || n.route != 0 {
            return true;
        }
        if let Some(w) = n.window.upgrade() {
            if fit {
                n.view.fit();
                publish(n, &w);
            } else {
                target = Some(if w.get_image_browsing() {
                    n.last_tool.map(InteractionMode::Tool)
                } else {
                    Some(InteractionMode::Mouse)
                });
            }
        }
        true
    });
    if let Some(mode) = target {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut()
                && r.gesture().is_none()
                && r.pending_drawing.is_none()
            {
                if let Some(mode) = mode {
                    r.set_mode(mode);
                } else {
                    r.select_tool_variant(1, 3);
                }
            }
        });
    }
    accepted
}
pub(super) fn map(w: &AnnotationCanvas, point: [f32; 2], reverse: bool) -> Option<[f32; 2]> {
    NAV.with(|s| {
        s.borrow().as_ref().filter(|n| matches(n, w)).map(|n| {
            if reverse {
                n.view.source(point)
            } else {
                n.view.screen(point)
            }
        })
    })
}
pub(super) fn native(w: &AnnotationCanvas, event: PopupPointerInput) -> bool {
    let handled = route_native(w, event);
    if handled {
        // Slint still needs pointer position/pressed state to refresh hover and hand cursors.
        // Its callbacks are suppressed only during this synchronous visual dispatch.
        NAV.with(|s| {
            if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
                n.forwarding = true;
            }
        });
        crate::bridge::dispatch_pointer_input(w.window(), event);
        NAV.with(|s| {
            if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
                n.forwarding = false;
                publish(n, w);
            }
        });
    }
    handled
}
fn route_native(w: &AnnotationCanvas, event: PopupPointerInput) -> bool {
    let dpi = w.window().scale_factor();
    let header = w.get_image_header_height() * dpi;
    let hit = |x: f32, y: f32| x >= 0. && x < size(w)[0] && y >= header && y < header + size(w)[1];
    match event {
        PopupPointerInput::LeftPressed { x, y } => {
            if hit(x, y) {
                w.invoke_focus_input();
                pointer(w, 0, [x, y - header])
            } else {
                NAV.with(|s| {
                    if let Some(n) = s.borrow_mut().as_mut().filter(|n| matches(n, w)) {
                        n.route = 2;
                        n.over = false;
                        publish(n, w);
                    }
                });
                false
            }
        }
        PopupPointerInput::Moved { x, y } => {
            let route = NAV.with(|s| {
                s.borrow()
                    .as_ref()
                    .filter(|n| matches(n, w))
                    .map(|n| n.route)
            });
            if route == Some(2) || (!hit(x, y) && route == Some(0) && !image_crop::dragging()) {
                pointer(w, 4, [0.; 2]);
                false
            } else {
                pointer(w, 1, [x, y - header])
            }
        }
        PopupPointerInput::LeftReleased { x, y } | PopupPointerInput::LeftCancelled { x, y } => {
            let chrome = NAV.with(|s| {
                let mut s = s.borrow_mut();
                if let Some(n) = s.as_mut().filter(|n| matches(n, w))
                    && n.route == 2
                {
                    n.route = 0;
                    return true;
                }
                false
            });
            if chrome {
                false
            } else {
                pointer(
                    w,
                    if matches!(event, PopupPointerInput::LeftCancelled { .. }) {
                        3
                    } else {
                        2
                    },
                    [x, y - header],
                )
            }
        }
        PopupPointerInput::Scrolled {
            x,
            y,
            delta_x,
            delta_y,
        } if hit(x, y) => {
            scroll(
                w,
                [x / dpi, (y - header) / dpi],
                delta_x / dpi,
                delta_y / dpi,
                false,
            );
            true
        }
        PopupPointerInput::Exited => {
            pointer(w, 4, [0.; 2]);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestPlatform;
    impl slint::platform::Platform for TestPlatform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(
                slint::platform::software_renderer::MinimalSoftwareWindow::new(
                    slint::platform::software_renderer::RepaintBufferType::NewBuffer,
                ),
            )
        }
    }
    fn window() -> AnnotationCanvas {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let w = AnnotationCanvas::new().unwrap();
        w.set_image_view(true);
        w.window().set_size(slint::LogicalSize::new(900., 700.));
        w.set_frame(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(600, 2000),
        ));
        w.show().unwrap();
        bind(&w);
        w
    }
    #[test]
    fn negative_source_origin_shares_crop_pan_zoom_and_pointer_coordinates() {
        let w = window();
        clear();
        w.set_source_left(-720.);
        w.set_source_top(-160.);
        bind(&w);
        let full = Rect {
            left: -720,
            top: -160,
            right: -120,
            bottom: 1840,
        };
        content(&w, full, true);
        let p = [240., 320.];
        let before = map(&w, p, true).unwrap();
        assert!(before[0] < 0.);
        let screen = map(&w, before, false).unwrap();
        assert!((screen[0] - p[0]).abs() < 0.01 && (screen[1] - p[1]).abs() < 0.01);
        scroll(&w, p, 0., 60., false);
        let after = map(&w, p, true).unwrap();
        assert!((after[0] - before[0]).abs() < 0.01 && (after[1] - before[1]).abs() < 0.01);
        let scale = w.get_image_zoom();
        let left = map(&w, [-720., -160.], false).unwrap();
        assert!((left[0] - w.get_image_pan_x()).abs() < 0.01);
        assert!((left[1] - w.get_image_pan_y()).abs() < 0.01);
        content(
            &w,
            Rect {
                left: -600,
                top: 100,
                right: -200,
                bottom: 1000,
            },
            true,
        );
        assert!(w.get_image_zoom() >= scale);
        clear();
    }
    #[test]
    fn footer_resize_handle_receives_native_press_after_viewport_routing() {
        let w = window();
        let count = Rc::new(std::cell::Cell::new(0));
        let observed = count.clone();
        w.on_image_resize_requested(move || observed.set(observed.get() + 1));
        native(&w, PopupPointerInput::Moved { x: 892., y: 692. });
        native(&w, PopupPointerInput::LeftPressed { x: 892., y: 692. });
        // Chrome is forwarded by the owner, unlike viewport events handled in native().
        crate::bridge::dispatch_pointer_input(
            w.window(),
            PopupPointerInput::LeftPressed { x: 892., y: 692. },
        );
        assert_eq!(count.get(), 1);
        clear();
    }
    #[test]
    fn native_and_slint_share_pan_zoom_chrome_and_cancel_ownership() {
        let w = window();
        let start = (w.get_image_pan_x(), w.get_image_pan_y());
        assert!(native(
            &w,
            PopupPointerInput::LeftPressed { x: 20., y: 100. }
        ));
        assert!(w.get_image_panning());
        native(&w, PopupPointerInput::Moved { x: 180., y: 200. });
        assert!((w.get_image_pan_x() - start.0 - 160.).abs() < 0.01);
        let zoom = w.get_image_zoom();
        scroll(&w, [200., 200.], 0., 60., false);
        assert_eq!(w.get_image_zoom(), zoom);
        assert!(escape());
        assert!(escape());
        assert_eq!(w.get_image_pan_x(), start.0);
        native(&w, PopupPointerInput::Moved { x: 200., y: 300. });
        assert_eq!(w.get_image_pan_y(), start.1);
        native(&w, PopupPointerInput::LeftReleased { x: 200., y: 300. });
        assert!(!escape());
        assert!(!native(
            &w,
            PopupPointerInput::LeftPressed { x: 100., y: 10. }
        ));
        assert!(!native(&w, PopupPointerInput::Moved { x: 100., y: 200. }));
        assert!(!native(
            &w,
            PopupPointerInput::LeftReleased { x: 100., y: 200. }
        ));
        let point = [200., 200.];
        let q = map(&w, point, true).unwrap();
        scroll(&w, point, 0., 30., false);
        assert_eq!(w.get_image_zoom(), zoom);
        scroll(&w, point, 0., 30., false);
        assert!(w.get_image_zoom() > zoom);
        let next = map(&w, point, true).unwrap();
        for i in 0..2 {
            assert!((q[i] - next[i]).abs() < 0.01);
        }
        let before = w.get_image_zoom();
        scroll(&w, point, 0., 60., true);
        assert_eq!(w.get_image_zoom(), before);
        modifiers(true);
        scroll(&w, point, 0., 60., false);
        assert_eq!(w.get_image_zoom(), before);
        modifiers(false);
        pointer(&w, 0, point);
        pointer(&w, 1, [-900., 1000.]);
        let offset = w.get_image_pan_x();
        lost_focus();
        assert!(!w.get_image_panning());
        assert_eq!(w.get_image_pan_x(), offset);
        clear();
    }
    #[test]
    fn drawing_uses_inverse_transform_and_retired_callbacks_cannot_navigate_new_window() {
        let w = window();
        w.set_image_browsing(false);
        let events = Rc::new(RefCell::new(vec![]));
        let out = events.clone();
        w.on_image_pointer(move |k, x, y| out.borrow_mut().push((k, x, y)));
        let screen = map(&w, [100., 200.], false).unwrap();
        w.invoke_viewport_pointer(0, screen[0], screen[1]);
        w.invoke_viewport_pointer(2, screen[0], screen[1]);
        let recorded = events.borrow();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].0, 0);
        assert!((recorded[0].1 - 100.).abs() < 0.01);
        assert!((recorded[0].2 - 200.).abs() < 0.01);
        drop(recorded);
        let z = w.get_image_zoom();
        scroll(&w, screen, 0., 60., false);
        assert_eq!(w.get_image_zoom(), z);
        let replacement = AnnotationCanvas::new().unwrap();
        replacement.set_image_view(true);
        replacement.set_frame(w.get_frame());
        replacement
            .window()
            .set_size(slint::LogicalSize::new(800., 600.));
        bind(&replacement);
        let before = replacement.get_image_pan_x();
        w.invoke_fit_image();
        w.invoke_viewport_pointer(0, 0., 0.);
        w.invoke_viewport_pointer(1, 600., 600.);
        assert_eq!(replacement.get_image_pan_x(), before);
        assert!(!replacement.get_image_panning());
        clear();
    }
    #[test]
    fn mode_toggle_repeat_text_entry_and_fit_leave_document_unchanged() {
        let w = window();
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.annotation_click_through = Rc::new(|_, _| true);
        crate::annotation::init(|_| PassiveWindowPreparation::Ready, lifecycle);
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.canvas_state.canvases.push(CanvasLayer::new(
                w.clone_strong(),
                Rect {
                    left: 0,
                    top: 0,
                    right: 600,
                    bottom: 2000,
                },
            ));
            r.document.mode = InteractionMode::Mouse;
        });
        assert!(key(" ", true, false, false, false));
        assert!(!w.get_image_browsing());
        REGISTRY.with(|s| assert_eq!(s.borrow().as_ref().unwrap().document.selected[1], 3));
        assert!(key(" ", true, false, false, true));
        assert!(!w.get_image_browsing());
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .pending_drawing
                .start_polyline((0., 0.), Style::default(), &AnnotationValues::default());
        });
        key(" ", false, false, false, false);
        REGISTRY.with(|s| s.borrow_mut().as_mut().unwrap().pending_drawing.cancel());
        key(" ", true, false, false, false);
        assert!(w.get_image_browsing());
        key(" ", false, false, false, false);
        pointer(&w, 0, [100., 100.]);
        pointer(&w, 1, [-1000., 3000.]);
        pointer(&w, 2, [-1000., 3000.]);
        assert!(key("0", true, true, false, false));
        assert!(w.get_image_pan_x() > 0.);
        REGISTRY.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .document
                    .session
                    .objects
                    .is_empty()
            )
        });
        crate::annotation::shutdown();
        clear();
    }
}
