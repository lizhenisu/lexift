//! Frozen image editing borrows a pin surface; the desktop document owns no pin windows.
use super::*;
use lexift_core::{
    domain::{
        annotation_export::ExportRequest,
        pinned_image::{Event, Output},
        region_selection::RegionEditor,
    },
    ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
};
use std::{collections::BTreeMap, sync::Arc};

struct Stored {
    document: editor_document::EditorDocument,
    base: Arc<CapturedRegion>,
    request: ExportRequest,
    samples: Arc<ScreenCaptureFrame>,
    initial_watermark: Option<Watermark>,
    original_displays: Vec<Rect>,
}
struct Recovery {
    document: editor_document::EditorDocument,
    lifecycle: WindowLifecycleCallbacks,
    position: slint::PhysicalPosition,
    collapsed: bool,
    open: bool,
    pin: u64,
    edit: u64,
    source: Rect,
    display: Rect,
}
thread_local! {
    static DOCUMENTS:RefCell<BTreeMap<u64,Stored>>=const{RefCell::new(BTreeMap::new())};
    static ACTIVE:RefCell<Option<Recovery>>=const{RefCell::new(None)};
}
pub(crate) fn install(
    id: u64,
    base: Arc<CapturedRegion>,
    samples: Arc<ScreenCaptureFrame>,
    mut request: ExportRequest,
) {
    let mut snapshot = (*request.snapshot).clone();
    let x = (base.bounds.left + base.bounds.right) / 2;
    let y = (base.bounds.top + base.bounds.bottom) / 2;
    if let Some((_, scale)) = snapshot
        .monitors
        .iter()
        .find(|(b, _)| x >= b.left && x < b.right && y >= b.top && y < b.bottom)
    {
        snapshot.scale = *scale;
    }
    request.snapshot = Arc::new(snapshot);
    let mut document = REGISTRY
        .with(|s| s.borrow().as_ref().map(|r| r.document.clone()))
        .unwrap_or_default();
    document.session = document.session.freeze_visible(&request.snapshot.session);
    document.mode = InteractionMode::Mouse;
    DOCUMENTS.with(|s| {
        s.borrow_mut().insert(
            id,
            Stored {
                initial_watermark: request.snapshot.session.watermark.clone(),
                original_displays: request.snapshot.session.displays.clone(),
                document,
                base,
                samples,
                request,
            },
        )
    });
}
pub(crate) fn active() -> bool {
    ACTIVE.with(|s| s.borrow().is_some())
}
pub(crate) fn sync_outputs(states: &lexift_core::domain::pinned_image::States) {
    let Some((id, _)) = current() else {
        return;
    };
    let busy = states.get(&id).is_some_and(|p| p.busy);
    REGISTRY.with(|s| {
        if let Some(main) = s.borrow().as_ref().and_then(|r| r.toolbar_state.main.get()) {
            main.set_output_busy(busy);
        }
    });
}
pub(super) fn source_metadata(
    id: u64,
    snapshot: &mut lexift_core::domain::annotation_export::ExportSnapshot,
) {
    DOCUMENTS.with(|s| {
        if let Some(p) = s.borrow().get(&id) {
            snapshot.scale = p.request.snapshot.scale;
            snapshot.monitors = p.request.snapshot.monitors.clone();
        }
    });
}
pub(crate) fn current() -> Option<(u64, u64)> {
    ACTIVE.with(|s| s.borrow().as_ref().map(|a| (a.pin, a.edit)))
}
pub(crate) fn forget(id: u64) {
    DOCUMENTS.with(|s| s.borrow_mut().remove(&id));
}
pub(crate) fn begin(id: u64, edit: u64) {
    if let Some((other, old)) = current() {
        end(other, old);
    }
    let Some(window) = crate::pinned_screenshot::edit_window(id, true) else {
        emit_pin(Event::EditFinished { id, edit });
        return;
    };
    let stored = DOCUMENTS.with(|s| s.borrow_mut().remove(&id));
    let Some(mut stored) = stored else {
        emit_pin(Event::EditFinished { id, edit });
        return;
    };
    let recovery = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        cancel_active_gesture(r);
        r.close_panel();
        let open = r.toolbar_state.main.get().is_some();
        let position = r
            .toolbar_state
            .main
            .get()
            .map(|w| w.window().position())
            .unwrap_or_default();
        let collapsed = r
            .toolbar_state
            .main
            .get()
            .is_some_and(|w| w.get_collapsed());
        let document = std::mem::take(&mut r.document);
        let lifecycle = r.lifecycle.clone();
        r.close();
        std::mem::swap(&mut r.document, &mut stored.document);
        r.document.mode = InteractionMode::Mouse;
        let frames = Arc::new(
            std::iter::once((*stored.base).clone())
                .chain(stored.samples.regions.iter().cloned())
                .collect::<Vec<_>>(),
        );
        r.lifecycle.annotation_screen_capture =
            Some(Arc::new(crate::FrozenCapture::new(frames.clone())));
        r.lifecycle.annotation_renderer = None;
        r.screenshot = Some(screenshot::Editing {
            id: edit,
            pin: Some((id, edit)),
            frames: frames.clone(),
            region: RegionEditor::new(stored.base.bounds),
            capture: Arc::new(ScreenCaptureFrame {
                revision: edit,
                serial: 1,
                regions: frames.as_ref().clone(),
            }),
            preview: None,
            revision: 0,
            presented: 0,
            feedback_bounds: None,
        });
        r.canvas_state.canvases.push(CanvasLayer::pin(
            window.clone_strong(),
            stored.base.bounds,
            stored.request.snapshot.scale,
            crate::pinned_screenshot::input(id),
        ));
        let layer = &r.canvas_state.canvases[0];
        Some(Recovery {
            document,
            lifecycle,
            position,
            collapsed,
            open,
            pin: id,
            edit,
            source: layer.bounds,
            display: layer.display_bounds(),
        })
    });
    DOCUMENTS.with(|s| s.borrow_mut().insert(id, stored));
    let Some(recovery) = recovery else {
        crate::pinned_screenshot::edit_window(id, false);
        emit_pin(Event::EditFinished { id, edit });
        return;
    };
    ACTIVE.with(|s| *s.borrow_mut() = Some(recovery));
    toggle(Instant::now());
}
pub(crate) fn finish() {
    if let Some((id, edit)) = current() {
        emit_pin(Event::EditFinished { id, edit });
    }
}
pub(crate) fn finish_now() {
    if let Some((id, edit)) = current() {
        finish();
        end(id, edit);
    }
}
pub(super) fn view_point(point: (f32, f32), reverse: bool) -> (f32, f32) {
    ACTIVE.with(|s| {
        let s = s.borrow();
        let Some(a) = s.as_ref() else {
            return point;
        };
        let (from, to) = if reverse {
            (a.display, a.source)
        } else {
            (a.source, a.display)
        };
        let z = (to.right - to.left) as f32 / (from.right - from.left) as f32;
        (
            to.left as f32 + (point.0 - from.left as f32) * z,
            to.top as f32 + (point.1 - from.top as f32) * z,
        )
    })
}
pub(super) fn view_zoom() -> f32 {
    ACTIVE.with(|s| {
        s.borrow().as_ref().map_or(1., |a| {
            (a.display.right - a.display.left) as f32 / (a.source.right - a.source.left) as f32
        })
    })
}
pub(crate) fn end(id: u64, edit: u64) {
    if current() == Some((id, edit)) {
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let Some(r) = s.as_mut().filter(|r| {
                r.screenshot
                    .as_ref()
                    .is_some_and(|e| e.pin == Some((id, edit)))
            }) else {
                return;
            };
            let _guards = r.pause_native_pointer_inputs();
            finish_text_edit_registry(r, true);
            r.finish_style_edit();
            sync_watermark_anchor(r);
            if let Some(snapshot) = export::snapshot_at(
                r,
                r.canvas_state.canvases[0].bounds,
                r.canvas_state.canvases[0].scale(),
            ) {
                let frozen = r.document.session.freeze_visible(&snapshot.session);
                r.gesture = None;
                r.pending_drawing.cancel();
                r.document.session = frozen;
            }
        });
    }
    let recovery = ACTIVE.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|a| a.pin == id && a.edit == edit) {
            s.take()
        } else {
            None
        }
    });
    let Some(recovery) = recovery else {
        return;
    };
    publish(id);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else {
            return;
        };
        if r.screenshot
            .as_ref()
            .is_some_and(|e| e.pin == Some((id, edit)))
        {
            finish_text_edit_registry(r, true);
            r.finish_style_edit();
            cancel_active_gesture(r);
            r.document.session.finish_rectangle_erase();
            let document = std::mem::take(&mut r.document);
            r.screenshot = None;
            r.close();
            DOCUMENTS.with(|s| {
                if let Some(p) = s.borrow_mut().get_mut(&id) {
                    p.document = document;
                }
            });
        }
        r.document = recovery.document;
        r.lifecycle = recovery.lifecycle;
        r.document.mode = InteractionMode::Mouse;
        r.document.session.set_edit_scope(EditScope::All);
        r.toolbar_state.resume = Some((recovery.position, recovery.collapsed));
    });
    crate::pinned_screenshot::edit_window(id, false);
    if recovery.open {
        toggle(Instant::now());
    }
}
pub(super) fn retain_closing(r: &mut Registry, id: u64, edit: u64) {
    let document = std::mem::take(&mut r.document);
    DOCUMENTS.with(|s| {
        if let Some(p) = s.borrow_mut().get_mut(&id) {
            p.document = document;
        }
    });
    slint::Timer::single_shot(Duration::ZERO, move || {
        emit_pin(Event::EditFailed {
            id,
            edit,
            error: crate::i18n::tr("Screenshot unavailable"),
        });
        end(id, edit);
    });
}
pub(crate) fn publish(id: u64) {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            sync_watermark_anchor(r);
        }
    });
    let snapshot = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let editing = r
            .screenshot
            .as_ref()
            .filter(|e| e.pin.is_some_and(|p| p.0 == id))?;
        let mut snapshot = export::snapshot_at(
            r,
            editing.region.bounds()?,
            r.canvas_state.canvases.first()?.scale(),
        )?;
        DOCUMENTS.with(|docs| {
            if let Some(p) = docs.borrow().get(&id) {
                snapshot.scale = p.request.snapshot.scale;
                snapshot.monitors = p.request.snapshot.monitors.clone();
            }
        });
        Some((snapshot, editing.pin?.1))
    });
    let Some((snapshot, edit)) = snapshot else {
        return;
    };
    let request = DOCUMENTS.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.get_mut(&id)?;
        p.request = ExportRequest {
            session: edit,
            request: p.request.request + 1,
            snapshot: Arc::new(snapshot),
        };
        Some(p.request.clone())
    });
    if let Some(document) = request {
        emit_pin(Event::DocumentChanged { id, document });
    }
}
/// Imported watermark anchors stay stable; new watermark layouts use the image bounds.
/// Comparing the undoable watermark value also restores its original anchor on undo.
pub(super) fn sync_watermark_anchor(r: &mut Registry) {
    let Some((id, _)) = r.screenshot.as_ref().and_then(|e| e.pin) else {
        return;
    };
    DOCUMENTS.with(|s| {
        if let Some(p) = s.borrow().get(&id) {
            let displays = if r.document.session.watermark == p.initial_watermark {
                p.original_displays.clone()
            } else {
                vec![p.base.bounds]
            };
            r.document.session.displays = displays;
        }
    });
}
pub(crate) fn output(kind: Output) {
    let Some((id, _)) = current() else {
        return;
    };
    publish(id);
    crate::pinned_screenshot::request_output(id, if kind == Output::Copy { 0 } else { 1 });
}
pub(crate) fn pointer(id: u64, event: i32, x: f32, y: f32) {
    if current().is_some_and(|p| p.0 == id) {
        canvas_pointer(0, event, x, y);
    }
}
pub(crate) fn key(id: u64, key: i32) {
    if !current().is_some_and(|p| p.0 == id) {
        return;
    }
    match key {
        0 => undo(),
        1 => delete_selected(),
        2 => finish_polyline(),
        3 => backspace_polyline(),
        _ => {}
    }
}
pub(crate) fn scroll(id: u64, delta: f32) {
    if current().is_some_and(|p| p.0 == id) {
        brush_size_scrolled(0, delta);
    }
}
pub(crate) fn escape(id: u64) {
    if current().is_some_and(|p| p.0 == id) {
        escape_from(current_focus_origin());
    }
}
pub(crate) fn shutdown() {
    ACTIVE.with(|s| s.borrow_mut().take());
    DOCUMENTS.with(|s| s.borrow_mut().clear());
}
