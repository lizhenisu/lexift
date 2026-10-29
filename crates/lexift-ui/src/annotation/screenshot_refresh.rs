//! Temporary capture visibility and one-version presentation; never closes the document.
use super::*;
use lexift_core::ports::magnifier::WindowToken;
use lexift_core::{
    Error, Result,
    domain::screenshot::refresh::{Event, Stage},
};

struct Context {
    session: u64,
    operation: u64,
    generation: u64,
    canvases: Vec<u64>,
    old_frames: Arc<Vec<CapturedRegion>>,
    old_capture: Arc<ScreenCaptureFrame>,
    feedback: Vec<super::super::frame::PresentedFrame>,
    target: usize,
    revision: Option<u64>,
    origin: Option<Option<WindowToken>>,
    started: Instant,
}
thread_local! {static BUILDING:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
pub(in crate::annotation) fn render_allowed() -> bool {
    BUILDING.get() || FLOW.with(|f| f.borrow().state.phase != ScreenshotPhase::Refreshing)
}
thread_local! {static CONTEXT:RefCell<Option<Context>>=const {RefCell::new(None)};}
fn send(e: Event) {
    emit(ScreenshotEvent::Refresh(e));
}
pub(crate) fn clear() {
    CONTEXT.with(|s| s.borrow_mut().take());
}
fn current(r: &Registry, session: u64, operation: u64) -> bool {
    CONTEXT.with(|s| {
        s.borrow().as_ref().is_some_and(|c| {
            c.session == session
                && c.operation == operation
                && c.generation == r.generation
                && r.screenshot.as_ref().is_some_and(|e| e.id == session)
                && r.canvas_state
                    .canvases
                    .iter()
                    .map(|c| c.revision())
                    .eq(c.canvases.iter().copied())
        })
    })
}
pub(super) fn available(r: &Registry) -> bool {
    FLOW.with(|f| {
        let f = f.borrow();
        f.state.phase == ScreenshotPhase::Editing && f.state.long.is_none()
    }) && r
        .screenshot
        .as_ref()
        .is_some_and(|e| e.pin.is_none() && !e.region.dragging())
        && r.gesture.is_none()
        && r.pending_drawing.is_none()
        && r.toolbar_state.toolbar_drag.is_none()
        && !r.export.pending()
        && !image_transform::busy()
        && !image_crop::active()
}
pub(in crate::annotation) fn request() {
    let allowed = REGISTRY.with(|s| s.borrow().as_ref().is_some_and(available));
    if !allowed {
        return;
    }
    FLOW.with(|f| {
        let mut f = f.borrow_mut();
        f.toast_timer.stop();
        f.toast.clear();
    });
    keyboard::commit_current();
    let request = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        if !available(r) {
            return None;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        size_input::close(r);
        r.close_panel();
        r.close_watermark_input();
        let e = r.screenshot.as_ref()?;
        let snapshot = export::snapshot_at(
            r,
            e.region.bounds()?,
            r.canvas_state.canvases.first()?.scale(),
        )?;
        Some(ExportRequest {
            session: e.id,
            request: FLOW.with(|f| f.borrow().state.operation + 1),
            snapshot: Arc::new(snapshot),
        })
    });
    if let Some(request) = request {
        emit(ScreenshotEvent::Refresh(Event::Requested(request)));
    }
}
pub(crate) fn suspend(session: u64, operation: u64) -> Result<()> {
    let result = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s
            .as_mut()
            .ok_or_else(|| Error::new("Screenshot refresh failed"))?;
        let state = FLOW.with(|f| f.borrow().state.clone());
        let pending = state
            .refresh
            .as_ref()
            .filter(|p| {
                state.session == session && p.operation == operation && p.stage == Stage::Suspending
            })
            .ok_or_else(|| Error::new("Screenshot refresh cancelled"))?;
        let e = r
            .screenshot
            .as_ref()
            .filter(|e| e.id == session && e.pin.is_none())
            .ok_or_else(|| Error::new("Screenshot refresh failed"))?;
        if r.gesture.is_some() || e.region.dragging() {
            return Err(Error::new("Screenshot refresh failed"));
        }
        if !lexift_core::domain::screenshot::refresh::layout_matches(
            &pending.request.snapshot.monitors,
            &(r.lifecycle.screenshot_displays)(),
        ) {
            return Err(Error::new(
                "Display layout changed. Start a new screenshot.",
            ));
        }
        let origin = r.lifecycle.annotation_foreground.as_ref().map(|q| q());
        let owned = origin.flatten().is_some_and(|token| {
            r.canvas_state
                .canvases
                .iter()
                .map(|c| c.window.window())
                .chain(r.toolbar_state.main.get().map(ComponentHandle::window))
                .any(|w| (r.lifecycle.annotation_window_token)(w) == Some(token))
        });
        let retiring = owned
            .then(|| r.lifecycle.annotation_foreground.clone())
            .flatten();
        let point = (r.lifecycle.toolbar_cursor_position)();
        let target = r
            .canvas_state
            .canvases
            .iter()
            .position(|c| {
                origin.flatten().is_some()
                    && (r.lifecycle.annotation_window_token)(c.window.window()) == origin.flatten()
            })
            .or_else(|| {
                point.and_then(|p| {
                    r.canvas_state.canvases.iter().position(|c| {
                        p.x >= c.bounds.left
                            && p.x < c.bounds.right
                            && p.y >= c.bounds.top
                            && p.y < c.bounds.bottom
                    })
                })
            })
            .unwrap_or(0);
        CONTEXT.with(|s| {
            *s.borrow_mut() = Some(Context {
                session,
                operation,
                generation: r.generation,
                canvases: r
                    .canvas_state
                    .canvases
                    .iter()
                    .map(|c| c.revision())
                    .collect(),
                old_frames: e.frames.clone(),
                old_capture: e.capture.clone(),
                feedback: Vec::new(),
                target,
                revision: None,
                origin,
                started: Instant::now(),
            })
        });
        let windows = r
            .canvas_state
            .canvases
            .iter()
            .filter_map(|c| (r.lifecycle.annotation_window_token)(c.window.window()))
            .collect::<Vec<_>>();
        if windows.len() != r.canvas_state.canvases.len()
            || !(r.lifecycle.complete_screenshot_refresh)(&windows, false)
        {
            return Err(Error::new("Screenshot refresh failed"));
        }
        r.hints.clear();
        r.close_panel();
        r.render_state.frame_tasks.cancel_render();
        if let Some(e) = r.screenshot.as_mut() {
            e.accepted_after = e.revision + 1;
            if let Some(p) = &e.preview {
                p.retire_before(e.accepted_after);
            }
        }
        for c in &mut r.canvas_state.canvases {
            (r.lifecycle.cancel_window_interaction)(c.window.window());
            CONTEXT.with(|s| {
                s.borrow_mut()
                    .as_mut()
                    .unwrap()
                    .feedback
                    .push(std::mem::take(&mut c.presented))
            });
            c.window
                .window()
                .hide()
                .map_err(|_| Error::new("Screenshot refresh failed"))?;
        }
        if let Some(w) = r.toolbar_state.main.get() {
            w.hide()
                .map_err(|_| Error::new("Screenshot refresh failed"))?;
        }
        if let Some(q) = retiring {
            CONTEXT.with(|s| {
                if let Some(c) = s.borrow_mut().as_mut() {
                    c.origin = Some(q());
                }
            });
        }
        if r.canvas_state
            .canvases
            .iter()
            .any(|c| c.window.window().is_visible())
            || r.toolbar_state
                .main
                .get()
                .is_some_and(|w| w.window().is_visible())
        {
            return Err(Error::new("Screenshot refresh failed"));
        }
        let windows = r
            .canvas_state
            .canvases
            .iter()
            .filter_map(|c| (r.lifecycle.annotation_window_token)(c.window.window()))
            .collect::<Vec<_>>();
        if windows.len() != r.canvas_state.canvases.len()
            || !(r.lifecycle.complete_screenshot_refresh)(&windows, false)
        {
            return Err(Error::new("Screenshot refresh failed"));
        }
        tracing::debug!(session, operation, "Screenshot refresh windows suspended");
        Ok(())
    });
    if result.is_ok() && !crate::pinned_screenshot::capture_visibility(true) {
        return Err(Error::new("Screenshot refresh failed"));
    }
    result
}
pub(crate) fn install(
    session: u64,
    operation: u64,
    frames: Arc<Vec<CapturedRegion>>,
    rollback: bool,
) {
    let valid = FLOW.with(|f| {
        let f = f.borrow();
        f.state.session == session
            && f.state.refresh.as_ref().is_some_and(|p| {
                p.operation == operation
                    && p.stage
                        == if rollback {
                            Stage::RollingBack
                        } else {
                            Stage::Presenting
                        }
            })
    });
    if !valid {
        return;
    }
    crate::pinned_screenshot::capture_visibility(false);
    if rollback {
        let result = REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let Some(r) = s.as_mut() else {
                return false;
            };
            // A suspension rejected before changing windows needs no rollback raster.
            if CONTEXT.with(|c| c.borrow().is_none()) {
                return true;
            }
            if !current(r, session, operation) {
                return false;
            }
            CONTEXT.with(|s| {
                let mut s = s.borrow_mut();
                let c = s.as_mut().unwrap();
                if let Some(e) = r.screenshot.as_mut() {
                    e.frames = c.old_frames.clone();
                    e.capture = c.old_capture.clone();
                    e.accepted_after = e.revision + 1;
                    if let Some(p) = &e.preview {
                        p.retire_before(e.accepted_after);
                    }
                }
                for (layer, old) in r.canvas_state.canvases.iter_mut().zip(c.feedback.drain(..)) {
                    layer.presented = old;
                }
                r.canvas_state.canvases.iter().all(|layer| {
                    layer.presented.replay(|p| {
                        (r.lifecycle.annotation_frame_presenter)(
                            layer.window.window(),
                            p.data(),
                            p.width(),
                            p.height(),
                        )
                    })
                })
            }) && reveal(r)
        });
        if result {
            send(Event::RolledBack { session, operation });
        } else {
            emit(ScreenshotEvent::Failed {
                session,
                error: crate::i18n::tr("Screenshot refresh failed"),
            });
        }
        return;
    }
    let result = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        if !current(r, session, operation) {
            return None;
        }
        // Native show/DPI delivery can finish after the initial editor's asset
        // preparation. Warm the exact installed DPI/theme keys before this feedback.
        for layer in &r.canvas_state.canvases {
            crate::screen_interaction::pixel_picker::prepare(layer.scale(), &r.lifecycle).ok()?;
            crate::screen_interaction::size_label::prepare(layer.scale(), &r.lifecycle).ok()?;
        }
        let e = r.screenshot.as_mut()?;
        e.frames = frames.clone();
        e.capture = Arc::new(ScreenCaptureFrame {
            revision: session,
            serial: operation,
            regions: frames.as_ref().clone(),
        });
        e.accepted_after = e.revision + 1;
        crate::screen_interaction::pixel_picker::replace_frames(session, frames);
        // The shared worker renders exact content/feedback off-thread. Its old
        // queued revisions have already been retired at suspension.
        BUILDING.set(true);
        super::render(r);
        BUILDING.set(false);
        let revision = r.screenshot.as_ref()?.revision;
        CONTEXT.with(|s| s.borrow_mut().as_mut().unwrap().revision = Some(revision));
        Some(())
    });
    if result.is_none() {
        send(Event::Presented {
            session,
            operation,
            result: Err(Error::new("Screenshot refresh failed")),
        });
    }
}
fn reveal(r: &Registry) -> bool {
    let mut okay = true;
    for c in &r.canvas_state.canvases {
        // These are the same prepared HWNDs, temporarily hidden without
        // retiring their host lease. Host::show is only for first preparation.
        okay = c.window.window().show().is_ok() && okay;
    }
    if let Some(w) = r.toolbar_state.main.get() {
        let position = w.window().position();
        okay = w.show().is_ok() && okay;
        w.window().set_position(position);
    }
    // Showing a Slint window can update its native software surface. Reinstall
    // the same owned complete feedback after show, before the desktop barrier.
    for c in &r.canvas_state.canvases {
        okay = c.presented.replay(|p| {
            (r.lifecycle.annotation_frame_presenter)(
                c.window.window(),
                p.data(),
                p.width(),
                p.height(),
            )
        }) && okay;
    }
    let windows = r
        .canvas_state
        .canvases
        .iter()
        .filter_map(|c| (r.lifecycle.annotation_window_token)(c.window.window()))
        .collect::<Vec<_>>();
    okay = windows.len() == r.canvas_state.canvases.len()
        && (r.lifecycle.complete_screenshot_refresh)(&windows, true)
        && okay;
    okay
}
/// Called only after all software feedback frames were installed for the same job.
pub(in crate::annotation) fn presented(
    generation: u64,
    session: u64,
    revision: u64,
    success: bool,
) -> bool {
    let operation = CONTEXT.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|c| {
                c.generation == generation && c.session == session && c.revision == Some(revision)
            })
            .map(|c| c.operation)
    });
    let Some(operation) = operation else {
        return false;
    };
    let layout_valid = REGISTRY.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            FLOW.with(|f| {
                f.borrow().state.refresh.as_ref().is_some_and(|p| {
                    lexift_core::domain::screenshot::refresh::layout_matches(
                        &p.request.snapshot.monitors,
                        &(r.lifecycle.screenshot_displays)(),
                    )
                })
            })
        })
    });
    let success =
        success && layout_valid && REGISTRY.with(|s| s.borrow().as_ref().is_some_and(reveal));
    send(Event::Presented {
        session,
        operation,
        result: if success {
            Ok(())
        } else {
            Err(Error::new(if layout_valid {
                "Screenshot refresh failed"
            } else {
                "Display layout changed. Start a new screenshot."
            }))
        },
    });
    true
}
pub(in crate::annotation) fn render_failed(session: u64) -> bool {
    let operation = CONTEXT.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|c| c.session == session)
            .map(|c| c.operation)
    });
    let Some(operation) = operation else {
        return false;
    };
    send(Event::Presented {
        session,
        operation,
        result: Err(Error::new("Screenshot refresh failed")),
    });
    true
}
pub(crate) fn finish(session: u64, operation: u64, success: bool) {
    if !FLOW.with(|f| {
        let f = f.borrow();
        f.state.session == session
            && f.state.operation == operation
            && f.state.phase == ScreenshotPhase::Editing
            && f.state.refresh.is_none()
    }) {
        return;
    }
    let context = CONTEXT.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref()
            .is_some_and(|c| c.session == session && c.operation == operation)
        {
            s.take()
        } else {
            None
        }
    });
    let Some(c) = context else {
        return;
    };
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut().filter(|r| r.generation == c.generation) else {
            return;
        };
        if !success {
            crate::screen_interaction::pixel_picker::replace_frames(session, c.old_frames);
        }
        let current = r.lifecycle.annotation_foreground.as_ref().and_then(|q| q());
        let owned = r
            .canvas_state
            .canvases
            .iter()
            .any(|w| (r.lifecycle.annotation_window_token)(w.window.window()) == current);
        let allowed =
            c.origin.is_none_or(|origin| origin == current) || (current.is_some() && owned);
        tracing::debug!(session,operation,origin=?c.origin,?current,allowed,target=c.target,"Screenshot refresh focus handoff");
        if allowed && let Some(layer) = r.canvas_state.canvases.get(c.target) {
            if let canvas::Surface::Canvas(w) = &layer.window {
                w.invoke_focus_input();
            }
            (r.lifecycle.activate_user_requested_window)(layer.window.window());
        }
        r.queue_render();
        tracing::debug!(
            session,
            operation,
            success,
            elapsed_ms = c.started.elapsed().as_millis(),
            "Screenshot refresh complete"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{AppCommand, AppState, domain::screenshot::ScreenshotCommand};
    thread_local! {static ACTIVATIONS:Cell<u32>=const{Cell::new(0)};}
    #[test]
    fn refresh_preserves_document_rolls_back_pixels_and_ignores_old_window_callbacks() {
        slint::platform::set_platform(super::super::tests::position_platform()).unwrap();
        let screens = vec![
            (
                Rect {
                    left: -300,
                    top: 0,
                    right: 0,
                    bottom: 200,
                },
                1.25,
            ),
            (
                Rect {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 200,
                },
                1.,
            ),
        ];
        let mut life = super::super::tests::lifecycle();
        let displays = screens.clone();
        life.screenshot_displays = Rc::new(move || displays.clone());
        let foreground = Rc::new(Cell::new(Some(WindowToken(9))));
        let query = foreground.clone();
        life.annotation_foreground = Some(Rc::new(move || query.get()));
        life.annotation_window_token = Rc::new(|_| Some(WindowToken(1)));
        life.complete_screenshot_refresh = Rc::new(|_, _| true);
        ACTIVATIONS.set(0);
        life.activate_user_requested_window = |_| {
            ACTIVATIONS.set(ACTIVATIONS.get() + 1);
            true
        };
        crate::annotation::init(|_| PassiveWindowPreparation::Ready, life.clone());
        crate::screen_interaction::size_label::prepare(1., &life).unwrap();
        let old = Arc::new(
            screens
                .iter()
                .map(|(bounds, _)| CapturedRegion {
                    bounds: *bounds,
                    pixels: [40, 80, 120, 255].repeat(300 * 200),
                })
                .collect::<Vec<_>>(),
        );
        let selection = Rect {
            left: -100,
            top: 50,
            right: 100,
            bottom: 150,
        };
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.generation = 31;
            r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
            for (bounds, scale) in &screens {
                let w = AnnotationCanvas::new().unwrap();
                w.window()
                    .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: *scale,
                    });
                w.window().set_size(slint::PhysicalSize::new(300, 200));
                r.canvas_state.canvases.push(CanvasLayer::new(w, *bounds));
            }
            r.screenshot = Some(Editing::image(7, selection, old.clone()));
            r.document
                .session
                .add(super::super::tests::object(Bounds::from_corners(
                    (-50., 70.),
                    (50., 90.),
                )));
            r.document.session.undo();
        });
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            let saved = r.screenshot.as_ref().unwrap().capture.clone();
            for color in [
                [95, 125, 135, 255],
                [113, 111, 157, 255],
                [86, 132, 124, 255],
            ] {
                let regions = screens
                    .iter()
                    .map(|(b, _)| CapturedRegion {
                        bounds: *b,
                        pixels: color.repeat(300 * 200),
                    })
                    .collect();
                r.screenshot.as_mut().unwrap().capture = Arc::new(ScreenCaptureFrame {
                    revision: 7,
                    serial: 2,
                    regions,
                });
                let pixmap = frame(r, 0).unwrap();
                let at = (80 * 300 + 250) * 4;
                assert_eq!(
                    &pixmap.data()[at..at + 4],
                    &color,
                    "unannotated editor feedback must preserve exact opaque source channels"
                );
            }
            r.screenshot.as_mut().unwrap().capture = saved;
        });
        let core = Rc::new(RefCell::new(AppState::default()));
        {
            let mut c = core.borrow_mut();
            c.screenshot.session = 7;
            c.screenshot.bounds = Some(selection);
            c.screenshot.phase = ScreenshotPhase::Editing;
        }
        let commands = Rc::new(RefCell::new(Vec::new()));
        let out = commands.clone();
        let state = core.clone();
        crate::annotation::set_handler(Rc::new(move |e| {
            let (cmd, s) = {
                let mut c = state.borrow_mut();
                let cmd = c.reduce(e);
                (cmd, c.screenshot.clone())
            };
            out.borrow_mut().extend(cmd);
            super::super::sync(&s);
        }));
        super::super::sync(&core.borrow().screenshot);
        let snapshot = REGISTRY
            .with(|s| export::snapshot_at(s.borrow().as_ref().unwrap(), selection, 1.).unwrap());
        // Failure after real suspension returns the same buffers and preserves redo.
        for operation in 1..=2 {
            emit(ScreenshotEvent::Refresh(Event::Requested(ExportRequest {
                session: 7,
                request: operation,
                snapshot: Arc::new(snapshot.clone()),
            })));
            suspend(7, operation).unwrap();
            assert!(!REGISTRY.with(|s| available(s.borrow().as_ref().unwrap())));
            emit(ScreenshotEvent::Refresh(Event::Suspended {
                session: 7,
                operation,
                result: Ok(()),
            }));
            if operation == 1 {
                emit(ScreenshotEvent::Refresh(Event::Captured {
                    session: 7,
                    operation,
                    result: Err(Error::new("Screenshot refresh failed")),
                }));
                install(7, operation, old.clone(), true);
                assert!(core.borrow().screenshot.editable());
                REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    assert!(Arc::ptr_eq(&r.screenshot.as_ref().unwrap().frames, &old));
                    assert!(r.document.session.can_redo());
                });
                foreground.set(Some(WindowToken(99)));
                finish(7, operation, false);
            } else {
                emit(ScreenshotEvent::Refresh(Event::Captured {
                    session: 7,
                    operation,
                    result: Ok(()),
                }));
                let candidate = Arc::new(
                    screens
                        .iter()
                        .map(|(bounds, _)| CapturedRegion {
                            bounds: *bounds,
                            pixels: [60, 200, 90, 255].repeat(300 * 200),
                        })
                        .collect(),
                );
                install(7, operation, candidate, false);
                let (generation, revision, rasters, labels) = REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    let e = r.screenshot.as_ref().unwrap();
                    (
                        r.generation,
                        e.revision,
                        (0..2).map(|i| frame(r, i).unwrap()).collect::<Vec<_>>(),
                        r.canvas_state
                            .canvases
                            .iter()
                            .map(|c| (c.revision(), None))
                            .collect(),
                    )
                });
                foreground.set(Some(WindowToken(100))); // Explicit user switch while the candidate is being prepared.
                super::super::super::frozen_editor::present(
                    generation,
                    7,
                    revision,
                    Some(rasters),
                    labels,
                );
                assert_eq!(
                    core.borrow().screenshot.refresh.as_ref().unwrap().stage,
                    Stage::Committing
                );
                assert!(!core.borrow().screenshot.editable());
                emit(ScreenshotEvent::Refresh(Event::Committed {
                    session: 7,
                    operation,
                    result: Ok(()),
                }));
                finish(7, operation, true);
                assert_eq!(core.borrow().screenshot.frame_version, 1);
                assert!(!presented(generation, 7, revision, true));
                install(7, 1, old.clone(), true);
            }
        }
        assert_eq!(
            ACTIVATIONS.get(),
            0,
            "external foreground must never be reclaimed"
        );
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert_eq!(
                r.screenshot.as_ref().unwrap().region.bounds(),
                Some(selection)
            );
            assert!(r.document.session.redo());
            assert_eq!(r.document.session.objects.len(), 1);
            assert_eq!(
                &r.screenshot.as_ref().unwrap().frames[0].pixels[..4],
                &[60, 200, 90, 255]
            );
        });
        assert!(commands.borrow().iter().any(|c| matches!(
            c,
            AppCommand::Screenshot(ScreenshotCommand::Refresh(
                lexift_core::domain::screenshot::refresh::Command::Commit { .. }
            ))
        )));
        clear();
        crate::annotation::shutdown();
    }
}
