//! Frozen screenshots reuse the editor document and window workflows, never live capture.
use super::*;
use crate::screen_interaction::selection as selection_host;
use lexift_core::{
    AppEvent,
    domain::{
        annotation_export::ExportRequest,
        region_selection::RegionEditor,
        screenshot::{ScreenshotEvent, ScreenshotOutput, ScreenshotPhase, ScreenshotState},
    },
    ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;

pub(super) use super::frozen_editor::{Editing, frame, render};

#[path = "screenshot_focus.rs"]
pub(super) mod handoff;
#[path = "screenshot_keyboard.rs"]
pub(super) mod keyboard;
#[path = "screenshot_long.rs"]
pub(crate) mod long;
#[path = "image_editor.rs"]
pub(in crate::annotation) mod long_editor;
#[path = "screenshot_size_input.rs"]
pub(super) mod size_input;

#[path = "screenshot_delay.rs"]
pub(crate) mod delay;

struct Recovery {
    document: editor_document::EditorDocument,
    position: slint::PhysicalPosition,
    collapsed: bool,
    lifecycle: WindowLifecycleCallbacks,
}
struct OcrFeedbackContext {
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    bounds: Rect,
}
#[derive(Default)]
struct Flow {
    activation: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    state: ScreenshotState,
    recovery: Option<Recovery>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    toast: window::Slot<crate::ScreenshotToastWindow>,
    toast_timer: slint::Timer,
    toast_measurement: Option<(u64, slint::PhysicalSize)>,
    toast_operation: Option<u64>,
    ocr_context: Option<OcrFeedbackContext>,
}
thread_local! { static FLOW:RefCell<Flow>=RefCell::new(Flow::default()); }
pub(crate) fn active() -> bool {
    FLOW.with(|s| {
        let s = s.borrow();
        s.state.active() || s.recovery.is_some()
    })
}
pub(crate) fn emit(event: ScreenshotEvent) {
    let handler = EXPORT_HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(AppEvent::Screenshot(event));
    }
}
pub(crate) fn sync(state: &ScreenshotState) {
    delay::sync(state);
    crate::screen_interaction::pixel_picker::sync(state);
    let feedback = FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != state.session {
            slot.toast_timer.stop();
            slot.toast.clear();
            slot.ocr_context = None;
        }
        let changed = slot.state.error != state.error
            || slot.state.session != state.session
            || slot.state.operation != state.operation;
        if slot.state.session != state.session || slot.state.phase != state.phase {
            tracing::debug!(session=state.session, operation=state.operation,
                previous=?slot.state.phase, phase=?state.phase,
                history=state.last_region.is_some(), reusable=state.reusable_region().is_some(),
                "Screenshot view phase changed");
        }
        slot.state = state.clone();
        changed && state.error.is_some()
    });
    if feedback && state.long.is_none() {
        let state = state.clone();
        slint::Timer::single_shot(Duration::ZERO, move || show_feedback(&state));
    }
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_output_busy(matches!(
                    state.phase,
                    ScreenshotPhase::Output | ScreenshotPhase::Recognizing
                ));
                main.set_feedback(crate::i18n::tr(state.error.as_deref().unwrap_or("")).into());
                main.set_long_capture_available(long::available(r));
            }
            if r.screenshot.is_some() {
                if state.phase != ScreenshotPhase::Editing {
                    // Older preparation snapshots can still be delivered during the UI handoff.
                    // Only an actual output for this editor revokes its pending focus request.
                    if matches!(
                        state.phase,
                        ScreenshotPhase::Output | ScreenshotPhase::Recognizing
                    ) && r.screenshot.as_ref().is_some_and(|e| e.id == state.session)
                    {
                        handoff::cancel(r);
                    }
                    size_input::close(r);
                }
                r.queue_render();
            }
        }
    });
    long::sync();
}
pub(super) fn request() {
    request_after(0);
}
pub(super) fn request_after(seconds: u8) {
    if seconds > 0 && !delay::available() {
        return;
    }
    pin_editor::finish_now();
    if active() || crate::ocr::screen_busy() {
        return;
    }
    FLOW.with(|s| {
        let mut s = s.borrow_mut();
        s.toast_timer.stop();
        s.toast.clear();
        s.ocr_context = None;
    });
    let request = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        FLOW.with(|s| {
            s.borrow_mut().activation = r.lifecycle.annotation_foreground.as_ref().map(|q| q())
        });
        if r.export.pending() {
            return None;
        }
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, true);
        }
        r.finish_style_edit();
        cancel_active_gesture(r);
        r.close_panel();
        r.document.session.finish_rectangle_erase();
        let snapshot = export::snapshot(r)?;
        Some(ExportRequest {
            session: r.generation,
            request: 1,
            snapshot: Arc::new(snapshot),
        })
    });
    if let Some(request) = request {
        if seconds == 0 {
            emit(ScreenshotEvent::Requested(request));
        } else {
            emit(ScreenshotEvent::Delayed { request, seconds });
        }
    }
}
pub(crate) fn suspend(id: u64) {
    if !FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == id
            && matches!(
                s.state.phase,
                ScreenshotPhase::Preparing | ScreenshotPhase::CountingDown
            )
            && s.recovery.is_none()
    }) {
        return;
    }
    let recovery = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        let main = r.toolbar_state.main.get()?;
        let position = main.window().position();
        let collapsed = main.get_collapsed();
        let document = std::mem::take(&mut r.document);
        r.close();
        Some(Recovery {
            document,
            position,
            collapsed,
            lifecycle: r.lifecycle.clone(),
        })
    });
    let suspended = recovery.is_some();
    FLOW.with(|s| s.borrow_mut().recovery = recovery);
    if suspended {
        crate::pinned_screenshot::set_annotation_interaction(false);
    }
    if !FLOW.with(|s| s.borrow().recovery.is_some()) {
        emit(ScreenshotEvent::Failed {
            session: id,
            error: crate::i18n::tr("Screenshot unavailable"),
        });
    }
}
pub(crate) fn retired_history_foreground(
    old: lexift_core::ports::magnifier::WindowToken,
    now: Option<lexift_core::ports::magnifier::WindowToken>,
) {
    FLOW.with(|flow| {
        let mut flow = flow.borrow_mut();
        if flow.state.phase == ScreenshotPhase::Preparing && flow.activation == Some(Some(old)) {
            flow.activation = Some(now);
        }
    });
}
pub(crate) fn suspended_pointer(
    id: u64,
) -> lexift_core::Result<Option<lexift_core::domain::geometry::Point>> {
    FLOW.with(|s| {
        let s = s.borrow();
        if s.state.session != id || s.state.phase != ScreenshotPhase::Preparing {
            return Err(lexift_core::Error::new("Screenshot suspension expired"));
        }
        let recovery = s
            .recovery
            .as_ref()
            .ok_or_else(|| lexift_core::Error::new("Screenshot suspension failed"))?;
        Ok((recovery.lifecycle.toolbar_cursor_position)())
    })
}
pub(crate) fn show(
    id: u64,
    frames: Arc<Vec<CapturedRegion>>,
    capture: Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
) {
    if !FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == id && s.state.phase == ScreenshotPhase::Selecting && s.recovery.is_some()
    }) {
        return;
    }
    crate::screen_interaction::size_label::clear();
    crate::screen_interaction::screenshot_help::clear();
    FLOW.with(|s| s.borrow_mut().frames = Some(frames.clone()));
    crate::screen_interaction::pixel_picker::begin(
        id,
        frames.clone(),
        Rc::new(|| {
            selection_host::refresh_picker();
            REGISTRY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut()
                    && r.screenshot.is_some()
                {
                    r.queue_render();
                }
            });
        }),
    );
    selection_host::set_screenshot(true);
    selection_host::set_handler(Rc::new(move |event| match event {
        selection_host::SelectionEvent::Ready { generation } => {
            if generation == id
                && FLOW.with(|s| {
                    let s = s.borrow();
                    s.state.session == id
                        && s.state.phase == ScreenshotPhase::Selecting
                        && s.recovery.is_some()
                })
            {
                crate::pinned_screenshot::set_frozen(true);
            }
        }
        selection_host::SelectionEvent::ReuseRegion {
            session,
            host,
            request,
        } => {
            emit(ScreenshotEvent::ReuseRegion {
                session,
                host,
                request,
            });
        }
        selection_host::SelectionEvent::QueryElements(query) => {
            emit(ScreenshotEvent::QueryElements(query))
        }
        selection_host::SelectionEvent::CancelElements { session, request } => {
            emit(ScreenshotEvent::CancelElements { session, request })
        }
        selection_host::SelectionEvent::Cancelled => cancel(),
        selection_host::SelectionEvent::Failed { error, .. } => {
            emit(ScreenshotEvent::Failed { session: id, error })
        }
        selection_host::SelectionEvent::SelectionSubmitted { selection, focus } => {
            // A quick pointer selection can submit before every monitor reports
            // feedback. Its own input window is now active, so pin retirement is safe.
            crate::pinned_screenshot::set_frozen(true);
            emit(ScreenshotEvent::Selected {
                session: id,
                bounds: selection.bounds,
            });
            REGISTRY.with(|slot| {
                if let Some(r) = slot.borrow_mut().as_mut() {
                    r.screenshot = Some(Editing {
                        pin: None,
                        id,
                        frames: frames.clone(),
                        capture: Arc::new(ScreenCaptureFrame {
                            revision: id,
                            serial: 1,
                            regions: frames.as_ref().clone(),
                        }),
                        region: RegionEditor::new(selection.bounds),
                        preview: None,
                        revision: 0,
                        accepted_after: 0,
                        presented: 0,
                        feedback_placement: None,
                        toolbar_adjustment: 0,
                        keyboard: Default::default(),
                        initial_focus: focus,
                        help: Default::default(),
                        size_input: Default::default(),
                    });
                    r.document.mode = InteractionMode::Mouse;
                    r.sync_edit_scope();
                    r.lifecycle.annotation_screen_capture = Some(capture.clone());
                    r.lifecycle.annotation_renderer = None;
                }
            });
            toggle(Instant::now());
        }
    }));
    selection_host::sync(id, selection_host::HostPhase::Capturing);
    let frames = FLOW.with(|s| s.borrow().frames.clone().unwrap());
    let origin = FLOW.with(|s| s.borrow().activation);
    let reusable = FLOW.with(|s| s.borrow().state.reusable_region().is_some());
    selection_host::show_screenshot_with_candidates(id, frames, origin, windows, reusable);
}
pub(crate) fn cancel() {
    if history_editor::active() {
        history_editor::close();
        return;
    }
    let id = FLOW.with(|s| s.borrow().state.session);
    emit(ScreenshotEvent::Cancelled { session: id });
}
pub(crate) fn restore(id: u64) {
    delay::clear();
    long::clear();
    let recovery = FLOW.with(|s| {
        let mut s = s.borrow_mut();
        if s.state.session != id {
            return None;
        };
        s.frames = None;
        s.recovery.take()
    });
    let Some(recovery) = recovery else { return };
    crate::screen_interaction::pixel_picker::clear();
    crate::screen_interaction::size_label::clear();
    crate::screen_interaction::screenshot_help::clear();
    crate::pinned_screenshot::set_frozen(false);
    selection_host::close();
    selection_host::set_handler(Rc::new(|_| {}));
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            r.screenshot = None;
            r.close();
            r.document = recovery.document;
            r.lifecycle = recovery.lifecycle;
            r.set_mode(InteractionMode::Mouse);
            r.toolbar_state.resume = Some((recovery.position, recovery.collapsed));
        }
    });
    toggle(Instant::now());
    show_ocr_feedback(id);
}
pub(crate) fn shutdown() {
    delay::clear();
    long::clear();
    crate::screen_interaction::pixel_picker::clear();
    crate::screen_interaction::size_label::clear();
    crate::screen_interaction::screenshot_help::clear();
    selection_host::close();
    selection_host::set_handler(Rc::new(|_| {}));
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            r.screenshot = None;
        }
    });
    FLOW.with(|s| *s.borrow_mut() = Flow::default());
}
pub(super) fn editor_ocr_owner() -> Option<lexift_core::domain::editor_ocr::Owner> {
    FLOW.with(|s| {
        let s = s.borrow();
        Some(lexift_core::domain::editor_ocr::Owner::Long {
            session: s.state.session,
            operation: s.state.long.as_ref()?.operation,
        })
    })
}
pub(super) fn recognizing() -> bool {
    image_resize::busy()
        || image_transform::busy()
        || editor_ocr::busy()
        || long::busy()
        || FLOW.with(|s| s.borrow().state.phase == ScreenshotPhase::Recognizing)
}
/// Freeze only crop identity here; OCR pixels are owned by the App capture session.
pub(super) fn recognize_text() {
    if long::image_editing() {
        editor_ocr::request();
        return;
    }
    if image_crop::active() {
        return;
    }
    if long::busy() {
        return;
    }
    if FLOW.with(|s| s.borrow().state.phase != ScreenshotPhase::Editing) {
        return;
    }
    keyboard::commit_current();
    let request = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        let editing = r
            .screenshot
            .as_ref()
            .filter(|e| e.pin.is_none() && !e.region.dragging())?;
        let (id, bounds) = (editing.id, editing.region.bounds()?);
        // The invoking toolbar selects the feedback display even for cross-screen crops.
        let main = r.toolbar_state.main.get()?;
        let p = main.window().position();
        let size = main.window().size();
        let anchor = Rect {
            left: p.x,
            top: p.y,
            right: p.x + size.width as i32,
            bottom: p.y + size.height as i32,
        };
        let point = (r.lifecycle.toolbar_cursor_position)()
            .filter(|p| {
                p.x >= anchor.left && p.x < anchor.right && p.y >= anchor.top && p.y < anchor.bottom
            })
            .unwrap_or(Point {
                x: anchor.left + (anchor.right - anchor.left) / 2,
                y: anchor.top + (anchor.bottom - anchor.top) / 2,
            });
        let context = OcrFeedbackContext {
            prepare: r.prepare,
            lifecycle: r.lifecycle.clone(),
            bounds: Rect {
                left: point.x,
                top: point.y,
                right: point.x.saturating_add(1),
                bottom: point.y.saturating_add(1),
            },
        };
        size_input::close(r);
        finish_text_edit_registry(r, true);
        cancel_active_gesture(r);
        r.finish_style_edit();
        r.close_watermark_input();
        r.close_panel();
        deselect_object(r);
        FLOW.with(|s| s.borrow_mut().ocr_context = Some(context));
        Some(ScreenshotEvent::Recognize {
            session: id,
            bounds,
        })
    });
    if let Some(event) = request {
        // Block queued input immediately, before the Core snapshot returns through the UI queue.
        FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Recognizing);
        emit(event);
    }
}
pub(super) fn output(kind: ScreenshotOutput) {
    if image_resize::busy() || image_transform::busy() || editor_ocr::busy() {
        return;
    }
    if history_editor::active() {
        history_editor::output(kind);
        return;
    }
    if image_crop::active() {
        return;
    }
    if long::busy() && !long::frozen() {
        return;
    }
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            size_input::close(r);
        }
    });
    keyboard::commit_current();
    let state = FLOW.with(|s| s.borrow().state.clone());
    if state.phase != ScreenshotPhase::Editing {
        return;
    }
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        let editing = r.screenshot.as_ref()?;
        if editing.id != state.session {
            return None;
        };
        let bounds = if long::image_editing() {
            r.document.session.image_crop.or(editing.region.bounds())?
        } else {
            editing.region.bounds()?
        };
        let mut snapshot = export::snapshot_at(
            r,
            bounds,
            r.canvas_state
                .canvases
                .first()?
                .window
                .window()
                .scale_factor(),
        )?;
        if long::image_editing() {
            snapshot.scale = 1.;
            snapshot.monitors = vec![(editing.region.bounds()?, 1.)];
        }
        Some(ExportRequest {
            session: state.session,
            request: state.operation + 1,
            snapshot: Arc::new(snapshot),
        })
    });
    if let Some(request) = request {
        emit(ScreenshotEvent::Output {
            session: state.session,
            operation: request.request,
            kind,
            request,
        });
    }
}
pub(super) fn pointer(r: &mut Registry, index: usize, event: i32, x: f32, y: f32) -> bool {
    if recognizing() {
        return true;
    }
    if long::image_editing() {
        if FLOW.with(|f| f.borrow().state.phase == ScreenshotPhase::Output) {
            return true;
        }
        return r.document.mode.is_mouse();
    }
    // Keep the input host/pointer current even while the numeric popup consumes
    // gestures, so dismissed input cannot restore a hint underneath the cursor.
    pointer_help(r, index, event, x, y);
    if size_input::pointer(r, index, event, x, y) {
        return true;
    }
    if event == 0 {
        keyboard::finish(r);
    }
    let Some(editing) = r.screenshot.as_mut() else {
        return false;
    };
    let Some(layer) = r.canvas_state.canvases.get_mut(index) else {
        return true;
    };
    let scale = layer.scale();
    let (px, py) = editing
        .pin
        .and_then(|(id, _)| pin_editor::input_point(id, x, y))
        .unwrap_or_else(|| {
            crate::screen_interaction::screen_point(layer.bounds, (x, y), layer.input_scale())
        });
    let point = Point {
        x: px.round() as i32,
        y: py.round() as i32,
    };
    layer.last_pointer = (event != 4).then_some((px, py));
    if editing.pin.is_some() {
        if r.document.mode.is_mouse() {
            return true;
        }
        let bounds = editing.region.bounds().unwrap();
        return event == 0
            && (point.x < bounds.left
                || point.x >= bounds.right
                || point.y < bounds.top
                || point.y >= bounds.bottom);
    }
    let output = FLOW.with(|s| s.borrow().state.phase == ScreenshotPhase::Output);
    if output {
        return true;
    }
    let mut desktop = editing.frames[0].bounds;
    for frame in editing.frames.iter() {
        desktop.left = desktop.left.min(frame.bounds.left);
        desktop.top = desktop.top.min(frame.bounds.top);
        desktop.right = desktop.right.max(frame.bounds.right);
        desktop.bottom = desktop.bottom.max(frame.bounds.bottom);
    }
    if editing.region.dragging() {
        match event {
            1 => editing.region.update(point, desktop),
            2 => {
                editing.region.update(point, desktop);
                editing.region.finish();
            }
            3 => {
                editing.region.cancel();
            }
            _ => {}
        }
        if event == 2
            && let Some(bounds) = editing.region.bounds()
        {
            let id = editing.id;
            slint::Timer::single_shot(Duration::ZERO, move || {
                emit(ScreenshotEvent::Selected {
                    session: id,
                    bounds,
                });
            });
        }
        if matches!(event, 2 | 3) {
            queue_adjustment_toolbar(r);
        }
        return true;
    }
    let bounds = editing.region.bounds().unwrap();
    let inside = point.x >= bounds.left
        && point.x < bounds.right
        && point.y >= bounds.top
        && point.y < bounds.bottom;
    if r.document.mode.is_mouse() {
        if event == 0 && editing.region.begin(point, scale, true) {
            queue_adjustment_toolbar(r);
            return true;
        }
        return true;
    }
    event == 0 && !inside
}
fn pointer_help(r: &mut Registry, index: usize, event: i32, x: f32, y: f32) {
    let before = help_feedback(r).map(|h| (h.host, h.bounds));
    let Some(e) = r.screenshot.as_mut().filter(|e| e.pin.is_none()) else {
        return;
    };
    let Some(layer) = r.canvas_state.canvases.get(index) else {
        return;
    };
    let (px, py) =
        crate::screen_interaction::screen_point(layer.bounds, (x, y), layer.input_scale());
    let point = Point {
        x: px.round() as i32,
        y: py.round() as i32,
    };
    let picker_changed = if event == 4 {
        crate::screen_interaction::pixel_picker::location().is_some_and(|p| {
            p.x >= layer.bounds.left
                && p.x < layer.bounds.right
                && p.y >= layer.bounds.top
                && p.y < layer.bounds.bottom
        }) && crate::screen_interaction::pixel_picker::point(e.id, None)
    } else {
        crate::screen_interaction::pixel_picker::point(
            e.id,
            Some(Point {
                x: px.floor() as i32,
                y: py.floor() as i32,
            }),
        )
    };
    let host = r
        .canvas_state
        .canvases
        .iter()
        .find(|c| {
            point.x >= c.bounds.left
                && point.x < c.bounds.right
                && point.y >= c.bounds.top
                && point.y < c.bounds.bottom
        })
        .map_or(layer.bounds, |c| c.bounds);
    e.help.pointer(
        if event == 4 { layer.bounds } else { host },
        (event != 4).then_some(point),
    );
    if picker_changed || before != help_feedback(r).map(|h| (h.host, h.bounds)) {
        r.queue_render();
    }
}

/// Freeze help placement, visibility and pixels with the same geometry as the preview job.
pub(super) fn help_feedback(
    r: &Registry,
) -> Option<crate::screen_interaction::screenshot_help::Feedback> {
    if long::busy() || long::image_editing() {
        return None;
    }
    use crate::screen_interaction::screenshot_help::Stage;
    let e = r.screenshot.as_ref().filter(|e| e.pin.is_none())?;
    let screens = r
        .canvas_state
        .canvases
        .iter()
        .map(|c| (c.bounds, c.scale()))
        .collect::<Vec<_>>();
    let host = e.help.host().or_else(|| screens.first().map(|s| s.0))?;
    let area = (r.lifecycle.popup_work_area)(Point {
        x: host.left,
        y: host.top,
    });
    let active = if e.region.dragging() || e.region.keyboard_adjusting() {
        e.region.bounds()
    } else {
        let bounds = gestures::display_draft(&r.gesture, &r.pending_drawing)
            .map(|o| o.bounds)
            .or_else(|| {
                r.gesture.as_ref().and_then(|_| {
                    r.document
                        .session
                        .selected
                        .and_then(|i| r.document.session.objects.get(i).map(|o| o.bounds))
                })
            })
            .or_else(|| {
                r.gesture.as_ref().and_then(|_| {
                    r.document
                        .session
                        .rectangle_erase_geometry()
                        .map(|g| g.bounds)
                })
            });
        bounds.map(|b| Rect {
            left: b.left.floor() as i32,
            top: b.top.floor() as i32,
            right: b.right.ceil() as i32,
            bottom: b.bottom.ceil() as i32,
        })
    };
    let blocked = size_input::busy(r)
        || r.editor.window().is_some()
        || r.parameter_edit.dragging()
        || r.toolbar_state.watermark.get().is_some()
        || r.toolbar_state
            .panel
            .get()
            .is_some_and(|p| p.get_menu() || p.get_parameter_input_active())
        || r.toolbar_state.menus.current.is_some()
        || FLOW.with(|s| {
            let s = s.borrow();
            s.state.session != e.id || s.state.phase != ScreenshotPhase::Editing
        });
    e.help.feedback(
        &screens,
        area,
        if r.document.mode.is_mouse() {
            Stage::Mouse
        } else {
            Stage::Draw
        },
        active,
        blocked,
    )
}
/// Window/control changes need feedback even when the immutable document did not change.
pub(super) fn refresh_help(r: &mut Registry) {
    if r.screenshot.as_ref().is_some_and(|e| e.pin.is_none()) {
        r.queue_render();
    }
}
pub(super) fn picker_feedback(
    r: &Registry,
) -> Option<crate::screen_interaction::pixel_picker::Feedback> {
    if long::busy() || long::image_editing() {
        return None;
    }
    let e = r.screenshot.as_ref().filter(|e| e.pin.is_none())?;
    if !r.document.mode.is_mouse()
        || size_input::busy(r)
        || r.editor.window().is_some()
        || r.parameter_edit.dragging()
        || r.toolbar_state.watermark.get().is_some()
        || r.toolbar_state.panel.get().is_some()
        || r.toolbar_state.menus.current.is_some()
        || FLOW.with(|s| s.borrow().state.phase != ScreenshotPhase::Editing)
    {
        return None;
    }
    let screens = r
        .canvas_state
        .canvases
        .iter()
        .map(|c| (c.bounds, c.scale()))
        .collect::<Vec<_>>();
    let point = crate::screen_interaction::pixel_picker::location()?;
    let _ = e;
    crate::screen_interaction::pixel_picker::feedback(
        &screens,
        (r.lifecycle.popup_work_area)(point),
    )
}
pub(super) fn cancel_region(r: &mut Registry) -> bool {
    let cancelled = r.screenshot.as_mut().is_some_and(|e| e.region.cancel());
    if cancelled {
        queue_adjustment_toolbar(r);
    }
    cancelled
}

/// Defer native visibility changes until the pointer callback releases the Registry.
fn queue_adjustment_toolbar(r: &mut Registry) {
    let Some(editing) = r.screenshot.as_mut().filter(|e| e.pin.is_none()) else {
        return;
    };
    editing.toolbar_adjustment = editing.toolbar_adjustment.wrapping_add(1);
    let (id, epoch, hidden) = (
        editing.id,
        editing.toolbar_adjustment,
        editing.region.dragging(),
    );
    let generation = r.generation;
    let revision = r.toolbar_state.main.revision();
    r.hints.clear();
    slint::Timer::single_shot(Duration::ZERO, move || {
        let restore = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let Some(r) = slot.as_mut().filter(|r| {
                r.generation == generation && r.toolbar_state.main.revision() == revision
            }) else {
                return false;
            };
            if !r.screenshot.as_ref().is_some_and(|e| {
                e.pin.is_none()
                    && e.id == id
                    && e.toolbar_adjustment == epoch
                    && e.region.dragging() == hidden
            }) {
                return false;
            };
            r.hints.clear();
            if hidden {
                let _guards = r.pause_native_pointer_inputs();
                if let Some(main) = r.toolbar_state.main.get() {
                    let _ = main.hide();
                }
                false
            } else {
                true
            }
        });
        if restore {
            toolbar_workflow::show_main(generation, revision, 0);
        }
    });
}

/// Immutable feedback travels with the frame job; it never moves a native window.
pub(super) fn label_feedback(
    r: &Registry,
) -> Option<crate::screen_interaction::size_label::Feedback> {
    if long::busy() || long::image_editing() {
        return None;
    }
    let editing = r.screenshot.as_ref().filter(|e| e.pin.is_none())?;
    let bounds = editing.region.bounds()?;
    let screens = r
        .canvas_state
        .canvases
        .iter()
        .map(|c| (c.bounds, c.scale()))
        .collect::<Vec<_>>();
    let area = (r.lifecycle.popup_work_area)(Point {
        x: bounds.left,
        y: bounds.top,
    });
    crate::screen_interaction::size_label::layout(bounds, &screens, area)
}

pub(crate) fn output_current(session: u64, operation: u64) -> bool {
    FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == session
            && s.state.operation == operation
            && s.state.phase == ScreenshotPhase::Output
    })
}

fn show_ocr_feedback(id: u64) {
    let context = FLOW.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.state.session != id || s.state.phase != ScreenshotPhase::Idle {
            return None;
        }
        let OcrFeedbackContext {
            prepare,
            lifecycle,
            bounds,
        } = s.ocr_context.take()?;
        let outcome = s.state.ocr_result?;
        s.toast_timer.stop();
        s.toast.clear();
        let toast =
            crate::screen_interaction::create(&lifecycle, crate::ScreenshotToastWindow::new)
                .ok()?;
        crate::theme::apply(&toast);
        toast.set_message(crate::i18n::tr(outcome.message()).into());
        toast.set_passive(true);
        toast.set_error(outcome != lexift_core::domain::screenshot::OcrOutcome::Copied);
        s.toast_operation = None;
        s.toast.open(toast);
        Some((prepare, lifecycle, bounds, s.toast.revision()))
    });
    if let Some((prepare, lifecycle, bounds, revision)) = context {
        show_toast(id, bounds, prepare, lifecycle, true, revision, 0);
    }
}

fn show_feedback(state: &ScreenshotState) {
    let Some(message) = state.error.as_ref() else {
        return;
    };
    let context = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let window = r.toolbar_state.main.get()?;
        let p = window.window().position();
        let size = window.window().size();
        Some((
            r.prepare,
            r.lifecycle.clone(),
            Rect {
                left: p.x,
                top: p.y,
                right: p.x + size.width as i32,
                bottom: p.y + size.height as i32,
            },
        ))
    });
    let Some((prepare, lifecycle, bounds)) = context else {
        return;
    };
    FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != state.session {
            return;
        };
        slot.toast.clear();
        let Ok(toast) =
            crate::screen_interaction::create(&lifecycle, crate::ScreenshotToastWindow::new)
        else {
            return;
        };
        crate::theme::apply(&toast);
        toast.set_message(crate::i18n::error(message).into());
        toast.on_dismissed(|| {
            slint::Timer::single_shot(Duration::ZERO, || {
                FLOW.with(|s| s.borrow_mut().toast.clear())
            })
        });
        slot.toast_operation = None;
        slot.toast.open(toast);
    });
    let revision = FLOW.with(|s| s.borrow().toast.revision());
    show_toast(
        state.session,
        bounds,
        prepare,
        lifecycle,
        false,
        revision,
        0,
    );
}
/// Long capture borrows the passive screen toast, with an independent operation guard.
fn clear_long_feedback() {
    FLOW.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.toast_operation.take().is_some() {
            s.toast_timer.stop();
            s.toast.clear();
            s.toast_measurement = None;
        }
    });
}
fn show_long_feedback(session: u64, operation: u64, message: &str, error: bool) {
    let context = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let bounds = long::source_area()?;
        let toast =
            crate::screen_interaction::create(&r.lifecycle, crate::ScreenshotToastWindow::new)
                .ok()?;
        crate::theme::apply(&toast);
        toast.set_message(
            if error {
                crate::i18n::error(message)
            } else {
                crate::i18n::tr(message)
            }
            .into(),
        );
        toast.set_passive(true);
        toast.set_error(error);
        // Register the toast below the live control owner before its first visible frame.
        Some((r.prepare, r.lifecycle.clone(), bounds, toast))
    });
    let Some((prepare, lifecycle, bounds, toast)) = context else {
        return;
    };
    // Native ownership must be assigned after preparation has materialized the HWND.
    let revision = FLOW.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.state.session != session
            || s.state.phase != ScreenshotPhase::Editing
            || !s.state.long.as_ref().is_some_and(|l| {
                l.operation == operation
                    && l.phase != lexift_core::domain::long_capture::workflow::Phase::Editing
            })
        {
            return None;
        }
        s.toast_timer.stop();
        s.toast.clear();
        s.toast_measurement = None;
        s.toast_operation = Some(operation);
        Some(s.toast.open(toast))
    });
    if let Some(revision) = revision {
        show_toast(session, bounds, prepare, lifecycle, true, revision, 0);
    }
}
fn show_toast(
    id: u64,
    bounds: Rect,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    ocr: bool,
    revision: u64,
    attempt: u8,
) {
    FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != id || slot.toast.revision() != revision {
            return;
        };
        let Some(toast) = slot.toast.get() else {
            return;
        };
        if let Some(operation) = slot.toast_operation
            && (slot.state.phase != ScreenshotPhase::Editing
                || !slot.state.long.as_ref().is_some_and(|l| {
                    l.operation == operation
                        && l.phase != lexift_core::domain::long_capture::workflow::Phase::Editing
                }))
        {
            slot.toast.clear();
            return;
        }
        if ocr
            && let Some(area) = (lifecycle.popup_work_area)(Point {
                x: bounds.left,
                y: bounds.top,
            })
        {
            // Establish the destination monitor before measuring logical toast content.
            toast.window().set_position(slint::PhysicalPosition::new(
                area.left + (area.right - area.left) / 2,
                area.bottom - 250,
            ));
        }
        match prepare(toast.window()) {
            PassiveWindowPreparation::Ready => {
                if let Some(operation) = slot.toast_operation {
                    let attached = REGISTRY.with(|r| {
                        r.borrow().as_ref().is_some_and(|r| {
                            long::attach_notification(r, toast.window(), id, operation)
                        })
                    });
                    if !attached {
                        slot.toast.clear();
                        return;
                    }
                }
                // Winit can begin at its 10px fallback width. Measure at the token width
                // while hidden, rather than wrapping text against that native placeholder.
                for _ in 0..3 {
                    let size = slint::LogicalSize::new(
                        toast.get_content_width(),
                        toast.get_content_height(),
                    );
                    toast.window().set_size(size);
                    let _ = toast.window().take_snapshot();
                    if (toast.get_content_height() - size.height).abs() < 0.1 {
                        break;
                    }
                }
                toast.window().set_size(slint::LogicalSize::new(
                    toast.get_content_width(),
                    toast.get_content_height(),
                ));
                let scale = toast.window().scale_factor();
                let area = (lifecycle.popup_work_area)(Point {
                    x: bounds.left,
                    y: bounds.top,
                });
                // Native WM_SIZE can still report the pre-measurement height here.
                // Position from the same settled logical dimensions submitted above.
                let size = slint::PhysicalSize::new(
                    (toast.get_content_width() * scale).round().max(1.) as u32,
                    (toast.get_content_height() * scale).round().max(1.) as u32,
                );
                // A cross-DPI resize reaches Slint through the next event-loop turn.
                // Require a stable hidden layout on consecutive preparation turns.
                if ocr && slot.toast_measurement != Some((revision, size)) {
                    slot.toast_measurement = Some((revision, size));
                    if attempt < 20 {
                        slot.toast
                            .prepare_after(Duration::from_millis(16), move || {
                                show_toast(
                                    id,
                                    bounds,
                                    prepare,
                                    lifecycle.clone(),
                                    ocr,
                                    revision,
                                    attempt + 1,
                                )
                            });
                    } else {
                        slot.toast.clear();
                    }
                    return;
                }
                let point = if ocr {
                    let Some(area) = area else {
                        slot.toast.clear();
                        return;
                    };
                    if !(lifecycle.configure_hint_window)(toast.window()) {
                        slot.toast.clear();
                        return;
                    }
                    Point {
                        x: area.left + ((area.right - area.left - size.width as i32) / 2).max(0),
                        y: (area.bottom
                            - size.height as i32
                            - (toast.get_screen_margin() * scale) as i32)
                            .max(area.top),
                    }
                } else {
                    placement::place_toolbar_popup(
                        bounds,
                        size.width,
                        size.height,
                        area,
                        (8. * scale) as i32,
                        placement::Side::Before,
                    )
                };
                toast
                    .window()
                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
                if slot.toast_operation.is_some() {
                    if !hints::show_capture_feedback(&lifecycle, toast.window()) {
                        slot.toast.clear();
                        return;
                    }
                } else {
                    let _ = toast.show();
                }
                slot.toast_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_secs(if ocr { 3 } else { 5 }),
                    move || {
                        FLOW.with(|s| {
                            let mut s = s.borrow_mut();
                            if s.state.session == id && s.toast.revision() == revision {
                                s.toast.clear();
                            }
                        })
                    },
                );
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                slot.toast
                    .prepare_after(Duration::from_millis(16), move || {
                        show_toast(
                            id,
                            bounds,
                            prepare,
                            lifecycle.clone(),
                            ocr,
                            revision,
                            attempt + 1,
                        )
                    })
            }
            _ => slot.toast.clear(),
        }
    });
}

pub(super) fn toolbar_ready(generation: u64, revision: u64) {
    let live = REGISTRY.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            r.generation == generation
                && r.toolbar_state.main.revision() == revision
                && r.toolbar_state.main.get().is_some()
        })
    });
    if !live {
        return;
    }
    let state = FLOW.with(|s| {
        let s = s.borrow();
        if s.state.error.is_some() && s.state.long.is_none() && s.toast.get().is_none() {
            Some(s.state.clone())
        } else {
            None
        }
    });
    if let Some(state) = state {
        slint::Timer::single_shot(Duration::ZERO, move || show_feedback(&state));
    }
}

pub(super) fn sync_region_feedback(r: &mut Registry) {
    if long::busy() || long::image_editing() {
        return;
    }
    position_region_toolbar(r, false);
}

/// The sole screenshot toolbar placement path, used before show and for live crop feedback.
/// Forced preparation bypasses cached placement after native window preparation or DPI changes.
pub(super) fn position_region_toolbar(r: &mut Registry, force: bool) -> bool {
    if long::busy() || long::image_editing() {
        return true;
    }
    let Some(editing) = r.screenshot.as_ref().filter(|e| e.pin.is_none()) else {
        return false;
    };
    let Some(bounds) = editing.region.bounds() else {
        return true;
    };
    if r.toolbar_state.toolbar_drag.is_some() || editing.region.keyboard_adjusting() {
        return true;
    }
    let Some(main) = r.toolbar_state.main.get() else {
        return true;
    };
    let size = main.window().size();
    let area = (r.lifecycle.popup_work_area)(Point {
        x: bounds.left,
        y: bounds.bottom,
    });
    let point = region_toolbar_position(bounds, size, area, main.window().scale_factor());
    let placement = (point, size);
    if force || editing.feedback_placement != Some(placement) {
        let _guards = r.pause_native_pointer_inputs();
        main.window()
            .set_position(slint::PhysicalPosition::new(point.x, point.y));
        r.screenshot.as_mut().unwrap().feedback_placement = Some(placement);
    }
    true
}

/// Place the movable toolbar independently of the crop caption.
fn region_toolbar_position(
    bounds: Rect,
    size: slint::PhysicalSize,
    area: Option<Rect>,
    scale: f32,
) -> Point {
    placement::place_toolbar_popup(
        bounds,
        size.width,
        size.height,
        area,
        (8. * scale) as i32,
        placement::Side::After,
    )
}

#[cfg(test)]
#[path = "screenshot_tests.rs"]
mod tests;

#[cfg(debug_assertions)]
pub(super) fn diagnostic_state() -> ScreenshotState {
    FLOW.with(|s| s.borrow().state.clone())
}
