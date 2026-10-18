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

#[path = "screenshot_keyboard.rs"]
pub(super) mod keyboard;
#[path = "screenshot_size_input.rs"]
pub(super) mod size_input;

struct Recovery {
    document: editor_document::EditorDocument,
    position: slint::PhysicalPosition,
    collapsed: bool,
    lifecycle: WindowLifecycleCallbacks,
}
#[derive(Default)]
struct Flow {
    activation: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    state: ScreenshotState,
    recovery: Option<Recovery>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    toast: window::Slot<crate::ScreenshotToastWindow>,
    toast_timer: slint::Timer,
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
    crate::screen_interaction::pixel_picker::sync(state);
    let feedback = FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let changed = slot.state.error != state.error
            || slot.state.session != state.session
            || slot.state.operation != state.operation;
        slot.state = state.clone();
        changed && state.error.is_some()
    });
    if feedback {
        let state = state.clone();
        slint::Timer::single_shot(Duration::ZERO, move || show_feedback(&state));
    }
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_output_busy(state.phase == ScreenshotPhase::Output);
                main.set_feedback(state.error.as_deref().unwrap_or("").into());
            }
            if r.screenshot.is_some() {
                if state.phase != ScreenshotPhase::Editing {
                    size_input::close(r);
                }
                r.queue_render();
            }
        }
    });
}
pub(super) fn request() {
    pin_editor::finish_now();
    if active() || crate::ocr::screen_busy() {
        return;
    }
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
        emit(ScreenshotEvent::Requested(request));
    }
}
pub(crate) fn suspend(id: u64) {
    if !FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == id && s.state.phase == ScreenshotPhase::Preparing && s.recovery.is_none()
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
pub(crate) fn show(
    id: u64,
    frames: Arc<Vec<CapturedRegion>>,
    capture: Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>,
) {
    if !FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == id && s.state.phase == ScreenshotPhase::Selecting && s.recovery.is_some()
    }) {
        return;
    }
    crate::screen_interaction::size_label::clear();
    crate::screen_interaction::screenshot_help::clear();
    crate::pinned_screenshot::set_frozen(true);
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
        selection_host::SelectionEvent::Cancelled => cancel(),
        selection_host::SelectionEvent::Failed { error, .. } => {
            emit(ScreenshotEvent::Failed { session: id, error })
        }
        selection_host::SelectionEvent::SelectionSubmitted { selection } => {
            let activation = current_focus_origin();
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
                        presented: 0,
                        feedback_placement: None,
                        toolbar_adjustment: 0,
                        keyboard: Default::default(),
                        initial_focus: Some(activation),
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
    selection_host::show_with_origin(id, frames, origin);
}
pub(crate) fn cancel() {
    let id = FLOW.with(|s| s.borrow().state.session);
    emit(ScreenshotEvent::Cancelled { session: id });
}
pub(crate) fn restore(id: u64) {
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
}
pub(crate) fn shutdown() {
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
pub(super) fn output(kind: ScreenshotOutput) {
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
        let bounds = editing.region.bounds()?;
        let snapshot = export::snapshot_at(
            r,
            bounds,
            r.canvas_state
                .canvases
                .first()?
                .window
                .window()
                .scale_factor(),
        )?;
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
    let (px, py) =
        crate::screen_interaction::screen_point(layer.bounds, (x, y), layer.input_scale());
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
        slot.toast.open(toast);
    });
    show_toast(state.session, bounds, prepare, lifecycle, 0);
}
fn show_toast(
    id: u64,
    bounds: Rect,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    attempt: u8,
) {
    FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != id {
            return;
        };
        let revision = slot.toast.revision();
        let Some(toast) = slot.toast.get() else {
            return;
        };
        match prepare(toast.window()) {
            PassiveWindowPreparation::Ready => {
                let scale = toast.window().scale_factor();
                let area = (lifecycle.popup_work_area)(Point {
                    x: bounds.left,
                    y: bounds.top,
                });
                let size = toast.window().size();
                let point = placement::place_toolbar_popup(
                    bounds,
                    size.width,
                    size.height,
                    area,
                    (8. * scale) as i32,
                    placement::Side::Before,
                );
                toast
                    .window()
                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
                let _ = toast.show();
                slot.toast_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_secs(5),
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
            PassiveWindowPreparation::Pending if attempt < 20 => slot
                .toast
                .prepare_after(Duration::from_millis(16), move || {
                    show_toast(id, bounds, prepare, lifecycle.clone(), attempt + 1)
                }),
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
    keyboard::activate_initial();
    let state = FLOW.with(|s| {
        let s = s.borrow();
        if s.state.error.is_some() && s.toast.get().is_none() {
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
    position_region_toolbar(r, false);
}

/// The sole screenshot toolbar placement path, used before show and for live crop feedback.
/// Forced preparation bypasses cached placement after native window preparation or DPI changes.
pub(super) fn position_region_toolbar(r: &mut Registry, force: bool) -> bool {
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
