//! Temporary browse input and result feedback for a live image editor OCR request.
use super::*;
use lexift_core::{
    AppEvent,
    domain::editor_ocr::{Event, Owner},
};
struct View {
    token: u64,
    owner: Owner,
    mode: InteractionMode,
    busy: bool,
    restored: bool,
    notice: Option<u64>,
    request: Option<lexift_core::domain::editor_ocr::Request>,
}
thread_local! { static VIEW: RefCell<Option<View>> = const {RefCell::new(None)}; }
fn emit(event: Event) {
    if let Some(handler) = EXPORT_HANDLER.with(|h| h.borrow().clone()) {
        handler(AppEvent::EditorOcr(event));
    }
}
pub(super) fn clear() {
    let old = VIEW.with(|s| s.borrow_mut().take());
    crate::bridge::image_history::clear_editor_feedback();
    if let Some(v) = old {
        slint::Timer::single_shot(Duration::ZERO, move || {
            emit(Event::Retired {
                owner: v.owner,
                token: v.token,
            })
        });
    }
}
pub(super) fn owner() -> Option<Owner> {
    if let Some((window, editor)) = history_editor::identity() {
        return Some(Owner::Document { window, editor });
    }
    if !screenshot::long::image_editing() {
        return None;
    }
    screenshot::editor_ocr_owner()
}
pub(super) fn busy() -> bool {
    VIEW.with(|s| s.borrow().as_ref().is_some_and(|v| v.busy))
}
pub(super) fn request() {
    if image_transform::busy()
        || busy()
        || image_crop::active()
        || screenshot::long_editor::outputting()
    {
        return;
    }
    let Some(owner) = owner() else {
        return;
    };
    crate::bridge::image_history::clear_editor_feedback();
    let request = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let w = screenshot::long::editor()?;
        if w.get_image_panning()
            || r.gesture().is_some()
            || !r.pending_drawing.is_none()
            || r.toolbar_state.menus.current.is_some()
        {
            return None;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        let source = r
            .document
            .session
            .image_source
            .or_else(|| r.screenshot.as_ref()?.region.bounds())?;
        let bounds = r.document.session.image_crop.unwrap_or(source);
        let mode = r.document.mode;
        if !r.set_mode(InteractionMode::Mouse) {
            return None;
        }
        r.close_panel();
        r.close_watermark_input();
        r.hints.clear();
        let revision = r.render_state.scene_revision;
        let token = window::next_revision();
        VIEW.with(|s| {
            *s.borrow_mut() = Some(View {
                token,
                owner,
                mode,
                busy: true,
                restored: false,
                notice: None,
                request: None,
            })
        });
        apply(r);
        Some(Event::Requested {
            token,
            owner,
            revision,
            color: r.document.session.image_color,
            orientation: r.document.session.image_orientation,
            source,
            bounds,
        })
    });
    if let Some(event) = request {
        emit(event);
    }
}
fn apply(r: &mut Registry) {
    if let Some(w) = screenshot::long::editor() {
        w.set_image_output_busy(
            busy() || image_transform::busy() || screenshot::long_editor::outputting(),
        );
    }
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_recognition_busy(busy());
        main.set_output_busy(
            busy() || image_transform::busy() || screenshot::long_editor::outputting(),
        );
    }
    r.queue_render();
}
pub(super) fn resume() {
    if screenshot::long::editor().is_some_and(|w| w.get_image_panning()) {
        return;
    }
    let mode = VIEW.with(|s| {
        let mut s = s.borrow_mut();
        let v = s.as_mut()?;
        if v.busy || v.restored {
            return None;
        }
        v.restored = true;
        Some((v.owner, v.mode))
    });
    if let Some((_, mode)) = mode.filter(|(id, _)| Some(*id) == owner()) {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                r.set_mode(mode);
                apply(r);
            }
        });
    }
}
pub(super) fn cancel() -> bool {
    let owner = VIEW.with(|s| s.borrow().as_ref().filter(|v| v.busy).map(|v| v.owner));
    if let Some(owner) = owner {
        emit(Event::Cancel { owner });
        true
    } else {
        false
    }
}
pub(crate) fn sync(state: &lexift_core::AppState) {
    let current = owner();
    let result = VIEW.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|v| Some(v.owner) != current) {
            s.take();
            crate::bridge::image_history::clear_editor_feedback();
            return None;
        }
        let v = s.as_mut()?;
        // Earlier queued snapshots cannot release the provisional input lease.
        if v.request.is_none() && state.editor_ocr.received != v.token {
            return None;
        }
        if v.notice.is_some() && state.editor_ocr.result.is_none() {
            crate::bridge::image_history::clear_editor_feedback();
        }
        v.busy = state.editor_ocr.request.is_some_and(|r| r.owner == v.owner);
        v.request = state
            .editor_ocr
            .request
            .filter(|r| r.owner == v.owner)
            .or(v.request);
        let result = state
            .editor_ocr
            .result
            .filter(|(r, _)| r.owner == v.owner && v.notice != Some(r.id));
        if let Some((request, _)) = result {
            v.notice = Some(request.id);
        }
        result
    });
    resume();
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut().filter(|_| current.is_some()) {
            apply(r);
        }
    });
    if let Some((_, outcome)) = result
        && let Some(w) = screenshot::long::editor()
    {
        let p = w.window().position();
        let size = w.window().size();
        crate::bridge::image_history::show_editor_feedback(
            Point {
                x: p.x + size.width as i32 / 2,
                y: p.y + size.height as i32 / 2,
            },
            outcome,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::{
        long_capture::{
            Feedback,
            workflow::{Phase, State as Long},
        },
        screenshot::ScreenshotPhase,
    };
    struct Backend;
    impl slint::platform::Platform for Backend {
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
    #[test]
    fn editor_ocr_freezes_crop_allows_navigation_and_restores_tool_after_release() {
        slint::platform::set_platform(Box::new(Backend)).unwrap();
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
        init(|_| PassiveWindowPreparation::Ready, lifecycle);
        let full = Rect {
            left: -20,
            top: -10,
            right: 580,
            bottom: 1990,
        };
        let crop = Rect {
            left: 0,
            top: 10,
            right: 500,
            bottom: 1800,
        };
        let w = AnnotationCanvas::new().unwrap();
        w.set_image_view(true);
        w.window().set_size(slint::LogicalSize::new(900., 700.));
        w.set_frame(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(600, 2000),
        ));
        w.set_source_left(-20.);
        w.set_source_top(-10.);
        w.show().unwrap();
        image_navigation::bind(&w);
        screenshot::long::set_editor(&w);
        let mut state = lexift_core::AppState::default();
        state.screenshot.session = 7;
        state.screenshot.phase = ScreenshotPhase::Editing;
        state.screenshot.long = Some(Long {
            operation: 4,
            source: full,
            phase: Phase::Editing,
            sequence: 1,
            width: 600,
            height: 2000,
            feedback: Feedback::Accepted,
            target: None,
        });
        screenshot::sync(&state.screenshot);
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.canvas_state
                .canvases
                .push(CanvasLayer::new(w.clone_strong(), full));
            r.document.session.image_source = Some(full);
            r.document.session.set_image_crop(crop, full);
            r.document.mode = InteractionMode::Tool(0);
            r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        EXPORT_HANDLER
            .with(|h| *h.borrow_mut() = Some(Rc::new(move |e| observed.borrow_mut().push(e))));
        request();
        request();
        assert!(busy());
        assert_eq!(events.borrow().len(), 1);
        let event = events.borrow_mut().remove(0);
        state.reduce(event);
        sync(&state);
        let frozen = state.editor_ocr.request.unwrap();
        assert_eq!(frozen.bounds, crop);
        assert_eq!(frozen.source, full);
        assert!(w.get_image_browsing());
        let zoom = w.get_image_zoom();
        image_navigation::scroll(&w, [100., 100.], 0., 120., false);
        assert!(w.get_image_zoom() > zoom);
        assert!(image_navigation::key(" ", true, false, false, false));
        image_crop::begin();
        assert!(!image_crop::active());
        undo();
        image_navigation::pointer(&w, 0, [100., 100.]);
        image_navigation::pointer(&w, 1, [180., 140.]);
        assert!(w.get_image_panning());
        state.reduce(AppEvent::EditorOcr(Event::Completed {
            request: frozen,
            outcome: lexift_core::domain::ocr::OcrOutcome::Empty,
        }));
        sync(&state);
        assert!(!busy());
        assert!(w.get_image_browsing());
        image_navigation::pointer(&w, 2, [180., 140.]);
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert_eq!(r.document.mode, InteractionMode::Tool(0));
            assert_eq!(r.document.session.image_crop, Some(crop));
            assert!(r.document.session.can_undo());
            assert!(!r.toolbar_state.main.get().unwrap().get_output_busy());
        });
        request();
        let event = events.borrow_mut().remove(0);
        state.reduce(event);
        sync(&state);
        assert!(cancel());
        let event = events.borrow_mut().remove(0);
        state.reduce(event);
        sync(&state);
        assert!(!busy());
        assert!(screenshot::long::editor().is_some());
        state.reduce(AppEvent::EditorOcr(Event::Completed {
            request: frozen,
            outcome: lexift_core::domain::ocr::OcrOutcome::Copied,
        }));
        sync(&state);
        assert!(state.editor_ocr.result.is_none());
        clear();
        image_navigation::clear();
    }
}
