//! One-shot focus transfer while the submitted selection window still owns its foreground.
use super::*;
use crate::screen_interaction::selection as host;

pub(in crate::annotation) fn cancel(r: &mut Registry) {
    if let Some(focus) = r.screenshot.as_mut().and_then(|e| e.initial_focus.take()) {
        tracing::debug!(
            session = focus.session,
            host = focus.host,
            "Screenshot editor pending focus cancelled"
        );
        slint::Timer::single_shot(Duration::ZERO, move || {
            if host::submission_live(focus) {
                host::finish_selection(focus.session);
            }
        });
    }
}

/// Called after the complete editor feedback is presented, independently of toolbar preparation.
pub(in crate::annotation) fn complete(generation: u64, session: u64, canvases: &[u64]) {
    let action = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        let e = r
            .screenshot
            .as_mut()
            .filter(|e| e.id == session && e.pin.is_none())?;
        tracing::debug!(session, generation, pending=e.initial_focus.is_some(), bounds=?e.region.bounds(),
            actual=?r.canvas_state.canvases.iter().map(|c| (c.revision(), c.bootstrap)).collect::<Vec<_>>(),
            expected=?canvases, "Screenshot editor focus readiness considered");
        if !FLOW.with(|s| {
            let s = s.borrow();
            s.state.session == session && s.state.phase == ScreenshotPhase::Editing
        }) || r.canvas_state.canvases.is_empty()
            || !r
                .canvas_state
                .canvases
                .iter()
                .map(|c| c.revision())
                .eq(canvases.iter().copied())
            || !r
                .canvas_state
                .canvases
                .iter()
                .all(|c| c.bootstrap == CanvasBootstrap::Ready)
        {
            return None;
        }
        let focus = e.initial_focus.take()?;
        let bounds = e.region.bounds()?;
        let target = r
            .canvas_state
            .canvases
            .iter()
            .find(|c| c.bounds == focus.screen)
            .or_else(|| {
                r.canvas_state.canvases.iter().find(|c| {
                    bounds.left >= c.bounds.left
                        && bounds.left < c.bounds.right
                        && bounds.top >= c.bounds.top
                        && bounds.top < c.bounds.bottom
                })
            });
        let current = r.lifecycle.annotation_foreground.as_ref().and_then(|q| q());
        let target_token = target.and_then(|target| (r.lifecycle.annotation_window_token)(target.window.window()));
        let permitted = target.is_some() && host::submission_live(focus)
            && (!focus.observed
                || current.is_some_and(|t| Some(t) == focus.source || Some(t) == target_token));
        tracing::debug!(session, generation, host=focus.host, ?current, source=?focus.source,
            ?target_token, permitted, elapsed_ms=focus.started.elapsed().as_secs_f64()*1000.,
            "Screenshot editor focus handoff");
        Some((
            focus,
            permitted,
            target.map(|target| target.window.clone_strong()),
            r.lifecycle.activate_user_requested_window,
        ))
    });
    let Some((focus, permitted, window, activate)) = action else {
        return;
    };
    if permitted && let Some(canvas::Surface::Canvas(canvas)) = &window {
        canvas.invoke_focus_input();
        let success = activate(canvas.window());
        tracing::debug!(
            session,
            success,
            "Screenshot editor focus activation completed"
        );
    }
    // Never destroy the source foreground before the one-shot target activation.
    // This also retires the old picture when external focus revoked authorization.
    if host::submission_live(focus) {
        host::finish_selection(session);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestPlatform, lifecycle};
    use super::*;
    use lexift_core::ports::magnifier::WindowToken;
    use std::cell::Cell;
    thread_local! {
        static FRONT: Cell<isize> = const { Cell::new(0) };
        static ACTIVATIONS: Cell<usize> = const { Cell::new(0) };
        static TARGET: Cell<isize> = const { Cell::new(0) };
    }
    fn token(w: &slint::Window) -> WindowToken {
        WindowToken(w as *const _ as isize)
    }
    fn activate(w: &slint::Window) -> bool {
        let focus = REGISTRY.with(|s| s.borrow().as_ref().unwrap().screenshot.as_ref().unwrap().id);
        assert!(
            host::diagnostic_visible(),
            "source picture must survive through target activation"
        );
        assert_eq!(focus, 51);
        ACTIVATIONS.set(ACTIVATIONS.get() + 1);
        TARGET.set(token(w).0);
        FRONT.set(token(w).0);
        false // Even native failure consumes the one-shot request.
    }
    fn finish(generation: u64, session: u64) {
        let ids = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .canvas_state
                .canvases
                .iter()
                .map(|c| c.revision())
                .collect::<Vec<_>>()
        });
        complete(generation, session, &ids);
    }
    struct Fixture {
        focus: host::SubmissionFocus,
    }
    impl Fixture {
        fn new() -> Self {
            slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
            FRONT.set(0);
            ACTIVATIONS.set(0);
            TARGET.set(0);
            let mut l = lifecycle();
            l.annotation_window_token = Rc::new(|w| Some(token(w)));
            l.annotation_foreground = Some(Rc::new(|| Some(WindowToken(FRONT.get()))));
            l.activate_user_requested_window = activate;
            super::super::super::init(|_| PassiveWindowPreparation::Ready, l.clone());
            host::init(|_| PassiveWindowPreparation::Ready, l);
            host::set_screenshot(true);
            host::sync(51, host::HostPhase::Capturing);
            let screens = [
                Rect {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 200,
                },
                Rect {
                    left: 300,
                    top: 0,
                    right: 600,
                    bottom: 200,
                },
            ];
            let frames = Arc::new(
                screens
                    .iter()
                    .map(|&bounds| CapturedRegion {
                        bounds,
                        pixels: [120, 160, 180, 255].repeat(300 * 200),
                    })
                    .collect::<Vec<_>>(),
            );
            let saved = Rc::new(RefCell::new(None));
            let output = saved.clone();
            host::set_handler(Rc::new(move |event| {
                if let host::SelectionEvent::SelectionSubmitted { focus, .. } = event {
                    *output.borrow_mut() = focus;
                }
            }));
            host::show(51, frames.clone());
            for input in [
                PopupPointerInput::LeftPressed { x: 40., y: 40. },
                PopupPointerInput::Moved { x: 160., y: 140. },
                PopupPointerInput::LeftReleased { x: 160., y: 140. },
            ] {
                host::pointer(51, screens[0], 1., input);
            }
            let focus = saved.borrow().unwrap();
            FRONT.set(focus.source.unwrap().0);
            assert!(host::submission_live(focus));
            FLOW.with(|s| {
                s.borrow_mut().state = ScreenshotState {
                    session: 51,
                    phase: ScreenshotPhase::Editing,
                    ..Default::default()
                }
            });
            REGISTRY.with(|s| {
                let mut slot = s.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.generation = 42;
                r.document.mode = InteractionMode::Mouse;
                for bounds in screens {
                    let w = AnnotationCanvas::new().unwrap();
                    w.window().set_size(slint::PhysicalSize::new(300, 200));
                    let mut layer = CanvasLayer::new(w, bounds);
                    layer.bootstrap = CanvasBootstrap::Ready;
                    r.canvas_state.canvases.push(layer);
                }
                r.screenshot = Some(Editing {
                    pin: None,
                    id: 51,
                    capture: Arc::new(ScreenCaptureFrame {
                        revision: 51,
                        serial: 1,
                        regions: frames.as_ref().clone(),
                    }),
                    frames,
                    region: RegionEditor::new(Rect {
                        left: 340,
                        top: 40,
                        right: 450,
                        bottom: 140,
                    }),
                    preview: None,
                    revision: 0,
                    accepted_after: 0,
                    image_content: None,
                    presented: 0,
                    feedback_placement: None,
                    toolbar_adjustment: 0,
                    keyboard: Default::default(),
                    initial_focus: Some(focus),
                    help: Default::default(),
                    size_input: Default::default(),
                });
            });
            Self { focus }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            super::super::super::shutdown();
            host::close();
        }
    }
    #[test]
    fn editor_handoff_waits_for_all_frames_ignores_toolbar_and_activates_source_screen_once() {
        let f = Fixture::new();
        let mut state = FLOW.with(|s| s.borrow().state.clone());
        state.phase = ScreenshotPhase::Selecting;
        super::super::sync(&state);
        assert!(
            REGISTRY.with(|s| s
                .borrow()
                .as_ref()
                .unwrap()
                .screenshot
                .as_ref()
                .unwrap()
                .initial_focus
                .is_some()),
            "a preparation snapshot must not cancel the editor's pending handoff"
        );
        state.phase = ScreenshotPhase::Editing;
        super::super::sync(&state);
        REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().canvas_state.canvases[1].bootstrap =
                CanvasBootstrap::Present
        });
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 0);
        assert!(host::submission_live(f.focus));
        REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().canvas_state.canvases[1].bootstrap =
                CanvasBootstrap::Ready
        });
        complete(42, 51, &[u64::MAX]);
        finish(41, 51);
        finish(42, 50);
        assert_eq!(ACTIVATIONS.get(), 0);
        // Selection origin is on the other display: input screen still takes priority.
        let expected = REGISTRY.with(|s| {
            token(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                    .window
                    .window(),
            )
            .0
        });
        finish(42, 51);
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 1);
        assert_eq!(TARGET.get(), expected);
        assert!(!host::submission_live(f.focus));
        assert!(
            !REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready),
            "handoff must not wait for toolbar preparation"
        );
        let keys = Rc::new(Cell::new(0));
        let out = keys.clone();
        REGISTRY.with(|s| {
            let slot = s.borrow();
            let canvas::Surface::Canvas(w) =
                &slot.as_ref().unwrap().canvas_state.canvases[0].window
            else {
                unreachable!()
            };
            w.on_region_key(move |_, _, _, _| {
                out.set(out.get() + 1);
                true
            });
            w.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "c".into() });
        });
        assert_eq!(
            keys.get(),
            1,
            "native handoff must prepare internal Slint keyboard focus"
        );
    }
    #[test]
    fn external_foreground_revokes_handoff_and_late_callbacks_cannot_retry() {
        let f = Fixture::new();
        FRONT.set(999);
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 0);
        assert_eq!(FRONT.get(), 999);
        assert!(!host::submission_live(f.focus));
        FRONT.set(f.focus.source.unwrap().0);
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 0);
    }
    #[test]
    fn rebuilt_canvas_falls_back_to_crop_origin_and_stale_host_cannot_activate() {
        let f = Fixture::new();
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .canvas_state
                .canvases
                .remove(0);
        });
        let expected = REGISTRY.with(|s| {
            token(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                    .window
                    .window(),
            )
            .0
        });
        finish(42, 51);
        assert_eq!(TARGET.get(), expected);
        assert!(!host::submission_live(f.focus));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .screenshot
                .as_mut()
                .unwrap()
                .initial_focus = Some(f.focus)
        });
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 1);
    }
    #[test]
    fn removed_input_display_and_unreachable_crop_retire_source_without_activation() {
        let f = Fixture::new();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.canvas_state.canvases.clear();
            let mut layer = CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: -500,
                    top: 0,
                    right: -200,
                    bottom: 200,
                },
            );
            layer.bootstrap = CanvasBootstrap::Ready;
            r.canvas_state.canvases.push(layer);
        });
        finish(42, 51);
        assert_eq!(ACTIVATIONS.get(), 0);
        assert!(
            !host::submission_live(f.focus),
            "missing target must not leave a retired source window alive"
        );
    }
    #[test]
    fn output_or_cancel_before_first_editor_frame_cannot_activate() {
        let f = Fixture::new();
        let mut state = FLOW.with(|s| s.borrow().state.clone());
        state.phase = ScreenshotPhase::Output;
        super::super::sync(&state);
        assert!(REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .screenshot
                .as_ref()
                .unwrap()
                .initial_focus
                .is_none()
        }));
        // Failed output returning to editing must not reauthorize an old focus request.
        FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Editing);
        finish(42, 51);
        slint::platform::update_timers_and_animations();
        assert_eq!(ACTIVATIONS.get(), 0);
        assert!(!host::submission_live(f.focus));
    }
}
