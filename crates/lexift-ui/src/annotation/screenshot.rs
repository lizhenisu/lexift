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

struct Recovery {
    document: editor_document::EditorDocument,
    position: slint::PhysicalPosition,
    collapsed: bool,
    lifecycle: WindowLifecycleCallbacks,
}
#[derive(Default)]
struct Flow {
    state: ScreenshotState,
    recovery: Option<Recovery>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    label: window::Slot<crate::AnnotationHintWindow>,
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
        if let Some(r) = slot.borrow().as_ref()
            && let Some(main) = r.toolbar_state.main.get()
        {
            main.set_output_busy(state.phase == ScreenshotPhase::Output);
            main.set_feedback(state.error.as_deref().unwrap_or("").into());
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
    crate::pinned_screenshot::set_frozen(true);
    FLOW.with(|s| s.borrow_mut().frames = Some(frames.clone()));
    selection_host::set_screenshot(true);
    selection_host::set_handler(Rc::new(move |event| match event {
        selection_host::SelectionEvent::Cancelled => cancel(),
        selection_host::SelectionEvent::Failed { error, .. } => {
            emit(ScreenshotEvent::Failed { session: id, error })
        }
        selection_host::SelectionEvent::SelectionSubmitted { selection } => {
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
                        feedback_bounds: None,
                    });
                    r.document.mode = InteractionMode::Mouse;
                    r.lifecycle.annotation_screen_capture = Some(capture.clone());
                    r.lifecycle.annotation_renderer = None;
                }
            });
            toggle(Instant::now());
            update_label();
        }
    }));
    selection_host::sync(id, selection_host::HostPhase::Capturing);
    let frames = FLOW.with(|s| s.borrow().frames.clone().unwrap());
    selection_host::show(id, frames);
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
        s.label.clear();
        s.frames = None;
        s.recovery.take()
    });
    let Some(recovery) = recovery else { return };
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
    layer.last_pointer = Some((px, py));
    if editing.pin.is_some() {
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
                update_label();
            });
        }
        return true;
    }
    let bounds = editing.region.bounds().unwrap();
    let inside = point.x >= bounds.left
        && point.x < bounds.right
        && point.y >= bounds.top
        && point.y < bounds.bottom;
    if r.document.mode.is_mouse() {
        let object = r.document.session.hit((px, py), scale).is_some();
        if event == 0 && editing.region.begin(point, scale, !object) {
            return true;
        }
        if !object && r.gesture.is_none() {
            return true;
        }
    }
    event == 0 && !inside
}
pub(super) fn cancel_region(r: &mut Registry) -> bool {
    r.screenshot.as_mut().is_some_and(|e| e.region.cancel())
}

fn update_label() {
    let value = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        let editing = r.screenshot.as_ref()?;
        let b = editing.region.bounds()?;
        Some((editing.id, b, r.prepare, r.lifecycle.clone()))
    });
    let Some((id, b, prepare, lifecycle)) = value else {
        return;
    };
    FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != id {
            return;
        };
        slot.label.clear();
        let Ok(label) =
            crate::screen_interaction::create(&lifecycle, crate::AnnotationHintWindow::new)
        else {
            return;
        };
        crate::theme::apply(&label);
        label.set_text(format!("{} × {} px", b.right - b.left, b.bottom - b.top).into());
        label.set_limit(240.);
        slot.label.open(label);
    });
    show_label(id, prepare, lifecycle, 0);
}
fn show_label(
    id: u64,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    attempt: u8,
) {
    let bounds = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.screenshot.as_ref())
            .and_then(|e| e.region.bounds())
    });
    let Some(bounds) = bounds else { return };
    FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.state.session != id {
            return;
        };
        let Some(label) = slot.label.get() else {
            return;
        };
        match prepare(label.window()) {
            PassiveWindowPreparation::Ready => {
                label.window().set_size(slint::LogicalSize::new(
                    label.get_content_width(),
                    label.get_content_height(),
                ));
                let scale = label.window().scale_factor();
                let area = (lifecycle.popup_work_area)(Point {
                    x: bounds.left,
                    y: bounds.top,
                });
                let size = label.window().size();
                let point = placement::place_toolbar_popup(
                    bounds,
                    size.width,
                    size.height,
                    area,
                    (8. * scale) as i32,
                    placement::Side::Before,
                );
                label
                    .window()
                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
                let _ = label.show();
            }
            PassiveWindowPreparation::Pending if attempt < 20 => slot
                .label
                .prepare_after(Duration::from_millis(16), move || {
                    show_label(id, prepare, lifecycle.clone(), attempt + 1)
                }),
            _ => slot.label.clear(),
        }
    });
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

pub(super) fn toolbar_ready() {
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
    let Some(editing) = r.screenshot.as_mut() else {
        return;
    };
    if editing.pin.is_some() {
        return;
    }
    let Some(bounds) = editing.region.bounds() else {
        return;
    };
    if editing.feedback_bounds == Some(bounds) {
        return;
    }
    editing.feedback_bounds = Some(bounds);
    let _guards = r.pause_native_pointer_inputs();
    FLOW.with(|slot| {
        let slot = slot.borrow();
        if let Some(label) = slot.label.get() {
            label.set_text(
                format!(
                    "{} × {} px",
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top
                )
                .into(),
            );
            label.window().set_size(slint::LogicalSize::new(
                label.get_content_width(),
                label.get_content_height(),
            ));
            let size = label.window().size();
            let area = (r.lifecycle.popup_work_area)(Point {
                x: bounds.left,
                y: bounds.top,
            });
            let point = placement::place_toolbar_popup(
                bounds,
                size.width,
                size.height,
                area,
                (8. * label.window().scale_factor()) as i32,
                placement::Side::Before,
            );
            label
                .window()
                .set_position(slint::PhysicalPosition::new(point.x, point.y));
        }
    });
    if r.toolbar_state.toolbar_drag.is_none()
        && let Some(main) = r.toolbar_state.main.get()
    {
        let size = main.window().size();
        let area = (r.lifecycle.popup_work_area)(Point {
            x: bounds.left,
            y: bounds.bottom,
        });
        let point = placement::place_toolbar_popup(
            bounds,
            size.width,
            size.height,
            area,
            (8. * main.window().scale_factor()) as i32,
            placement::Side::After,
        );
        main.window()
            .set_position(slint::PhysicalPosition::new(point.x, point.y));
    }
}

#[cfg(test)]
#[path = "screenshot_tests.rs"]
mod tests;

#[cfg(debug_assertions)]
pub(super) fn diagnostic_state() -> ScreenshotState {
    FLOW.with(|s| s.borrow().state.clone())
}
