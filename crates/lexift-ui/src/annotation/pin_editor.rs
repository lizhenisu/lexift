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
    presented_orientation: lexift_core::domain::image_transform::Orientation,
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
}
thread_local! {
    static DOCUMENTS:RefCell<BTreeMap<u64,Stored>>=const{RefCell::new(BTreeMap::new())};
    static ACTIVE:RefCell<Option<Recovery>>=const{RefCell::new(None)};
}
pub(crate) fn install(
    id: u64,
    base: Arc<CapturedRegion>,
    samples: Arc<ScreenCaptureFrame>,
    request: ExportRequest,
) {
    install_owned(id, base, samples, request, false);
}
pub(crate) fn install_history(
    id: u64,
    base: Arc<CapturedRegion>,
    samples: Arc<ScreenCaptureFrame>,
    request: ExportRequest,
) {
    install_owned(id, base, samples, request, true);
}
fn install_owned(
    id: u64,
    base: Arc<CapturedRegion>,
    samples: Arc<ScreenCaptureFrame>,
    mut request: ExportRequest,
    reset_history: bool,
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
    document.session = if reset_history {
        request.snapshot.session.restored_image()
    } else {
        document.session.freeze_visible(&request.snapshot.session)
    };
    document.session.image_source = Some(base.bounds);
    document.mode = InteractionMode::Mouse;
    document.session.set_edit_scope(EditScope::None);
    let mut snapshot = (*request.snapshot).clone();
    snapshot.session = Arc::new(document.session.clone());
    request.snapshot = Arc::new(snapshot);
    request.session = 0;
    DOCUMENTS.with(|s| {
        s.borrow_mut().insert(
            id,
            Stored {
                presented_orientation: document.session.image_orientation,
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
/// Initial Core retention must use the same editable baseline as this pin surface.
pub(crate) fn retained_document(id: u64) -> Option<ExportRequest> {
    DOCUMENTS.with(|s| s.borrow().get(&id).map(|p| p.request.clone()))
}
pub(super) fn recognition_busy() -> bool {
    current().is_some_and(|(id, _)| crate::pinned_screenshot::recognition_input_paused(id))
}
pub(crate) fn cropping(id: u64) -> bool {
    current().is_some_and(|p| p.0 == id) && pin_crop::active()
}
pub(crate) fn active() -> bool {
    ACTIVE.with(|s| s.borrow().is_some())
}
pub(crate) fn refresh_recognition_busy(id: u64, busy: bool) {
    if current().is_some_and(|p| p.0 == id) {
        REGISTRY.with(|s| {
            if let Some(w) = s.borrow().as_ref().and_then(|r| r.toolbar_state.main.get()) {
                w.set_output_busy(busy);
            }
        });
    }
}
pub(super) fn history_allowed(r: &Registry, forward: bool) -> bool {
    let Some((id, _)) = current().filter(|p| crate::pinned_screenshot::policy::locked(p.0)) else {
        return true;
    };
    !r.document
        .session
        .history_changes_image_orientation(forward)
        && source(id).is_some_and(|full| {
            !r.document
                .session
                .history_changes_image_extent(forward, full)
        })
}
pub(crate) fn sync_outputs(states: &lexift_core::domain::pinned_image::States) {
    let Some((id, _)) = current() else {
        return;
    };
    pin_crop::sync_state(states);
    let busy = crate::pinned_screenshot::recognition_input_paused(id)
        || states
            .get(&id)
            .is_some_and(|p| p.busy || p.proposed.is_some() || p.interaction.pending.is_some());
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow().as_ref() {
            sync_history_controls(r);
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_output_busy(busy);
                main.set_crop_locked(crate::pinned_screenshot::policy::locked(id));
            }
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
        r.sync_edit_scope();
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
            region: RegionEditor::new(r.document.session.image_crop.unwrap_or(stored.base.bounds)),
            capture: Arc::new(ScreenCaptureFrame {
                revision: edit,
                serial: 1,
                regions: frames.as_ref().clone(),
            }),
            preview: None,
            revision: 0,
            presented: 0,
            feedback_placement: None,
            toolbar_adjustment: 0,
            keyboard: Default::default(),
            initial_focus: None,
            help: Default::default(),
            size_input: Default::default(),
        });
        r.canvas_state.canvases.push(CanvasLayer::pin(
            window.clone_strong(),
            r.document.session.image_crop.unwrap_or(stored.base.bounds),
            stored.request.snapshot.scale,
            crate::pinned_screenshot::input(id),
        ));
        Some(Recovery {
            document,
            lifecycle,
            position,
            collapsed,
            open,
            pin: id,
            edit,
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
/// Policy changes never reinterpret a live drawing or candidate gesture.
pub(crate) fn interaction_idle(id: u64) -> bool {
    if crate::pinned_screenshot::zoom::panning(id) {
        return false;
    }
    if current().is_none_or(|p| p.0 != id) {
        return true;
    }
    REGISTRY.with(|s| {
        s.try_borrow().ok().is_some_and(|s| {
            s.as_ref().is_some_and(|r| {
                r.gesture().is_none() && r.pending_drawing.is_none() && !pin_crop::active()
            })
        })
    })
}
pub(crate) fn policy_requested(id: u64) {
    if current().is_some_and(|p| p.0 == id) {
        REGISTRY.with(|s| {
            if let Some(m) = s.borrow().as_ref().and_then(|r| r.toolbar_state.main.get()) {
                m.set_output_busy(true);
            }
        });
    }
}
pub(crate) fn prepare_interaction(id: u64) -> bool {
    if !interaction_idle(id) {
        return false;
    }
    if current().is_some_and(|p| p.0 == id) {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                finish_text_edit_registry(r, true);
                r.finish_style_edit();
                r.close_panel();
                r.hints.clear();
            }
        });
    }
    true
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
    if let Some(point) = screenshot::long_editor::view_point(point, reverse) {
        return point;
    }
    current()
        .and_then(|(id, _)| {
            let (source, display) = crate::pinned_screenshot::geometry(id)?;
            let (full, orientation) = DOCUMENTS.with(|s| {
                s.borrow()
                    .get(&id)
                    .map(|p| (p.base.bounds, p.presented_orientation))
            })?;
            if crate::pinned_screenshot::zoom::active(id) {
                let p = if reverse {
                    [f64::from(point.0), f64::from(point.1)]
                } else {
                    orientation.point([f64::from(point.0), f64::from(point.1)], full)
                };
                let p = crate::pinned_screenshot::zoom::map(id, p, reverse)?;
                let p = if reverse {
                    orientation
                        .inverse()
                        .point(p, orientation.rect(full, full).ok()?)
                } else {
                    p
                };
                return Some((p[0] as f32, p[1] as f32));
            }
            mapped_point(point, reverse, full, source, display, orientation)
        })
        .unwrap_or(point)
}
pub(super) fn view_zoom() -> f32 {
    screenshot::long_editor::view_zoom().unwrap_or_else(|| {
        if let Some(z) = current().and_then(|(id, _)| crate::pinned_screenshot::zoom::scale(id)) {
            return z as f32;
        }
        current()
            .and_then(|(id, _)| crate::pinned_screenshot::geometry(id))
            .map_or(1., |(s, d)| {
                (d.right - d.left) as f32 / (s.right - s.left) as f32
            })
    })
}
pub(crate) fn candidate_present(id: u64, revision: u64, image: Arc<CapturedRegion>) -> bool {
    pin_crop::present(id, revision, image)
}
pub(super) fn source(id: u64) -> Option<Rect> {
    DOCUMENTS.with(|s| s.borrow().get(&id).map(|p| p.base.bounds))
}
pub(super) fn propose(r: &Registry, session: Session) -> Option<ExportRequest> {
    let (id, edit) = current()?;
    let bounds = session.image_crop.or(session.image_source)?;
    let mut snapshot = export::snapshot_at(r, bounds, r.canvas_state.canvases.first()?.scale())?;
    source_metadata(id, &mut snapshot);
    snapshot.session = Arc::new(session);
    DOCUMENTS.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.get_mut(&id)?;
        p.request.request += 1;
        Some(ExportRequest {
            session: edit,
            request: p.request.request,
            snapshot: Arc::new(snapshot),
        })
    })
}
pub(crate) fn end(id: u64, edit: u64) {
    if current() == Some((id, edit)) {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                pin_crop::cancel(r);
            }
        });
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
        r.set_mode(InteractionMode::Mouse);
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
        // A pin proposal is an editable document baseline, not an output-only
        // snapshot. Direction changes must retain the preceding shared history.
        snapshot.session = Arc::new(r.document.session.freeze_visible(&snapshot.session));
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
/// Finish input transactions without ending the editable pin or changing its tool.
pub(crate) fn prepare_recognition(id: u64) -> bool {
    if !current().is_some_and(|p| p.0 == id) {
        return true;
    }
    let ready = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else {
            return false;
        };
        if pin_crop::active() || r.gesture.is_some() || !r.pending_drawing.is_none() {
            return false;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.close_panel();
        true
    });
    if ready {
        publish(id);
    }
    ready
}
pub(crate) fn output(kind: Output) {
    if recognition_busy() || pin_crop::active() {
        return;
    }
    let Some((id, _)) = current() else {
        return;
    };
    publish(id);
    crate::pinned_screenshot::request_output(id, if kind == Output::Copy { 0 } else { 1 });
}
pub(crate) fn pointer(id: u64, event: i32, x: f32, y: f32) {
    if crate::pinned_screenshot::recognition_input_paused(id) {
        return;
    }
    if current().is_some_and(|p| p.0 == id) {
        if pin_crop::pointer(id, event, x, y) {
            return;
        }
        if event == 0
            && let Some(w) = crate::pinned_screenshot::window_for_view(id)
            && !crate::pinned_screenshot::zoom::contains(
                id,
                [
                    f64::from(x * w.window().scale_factor()),
                    f64::from(y * w.window().scale_factor()),
                ],
            )
            && !browse(id)
        {
            return;
        }
        if browse(id) && crate::pinned_screenshot::zoom::active(id) {
            if let Some(w) = crate::pinned_screenshot::window_for_view(id) {
                let dpi = f64::from(w.window().scale_factor());
                crate::pinned_screenshot::zoom::pointer(
                    id,
                    event,
                    [f64::from(x) * dpi, f64::from(y) * dpi],
                );
            }
            return;
        }
        // The OS rounds width and height independently at fractional DPI. Match
        // the actual filled image on each axis, rather than deriving Y from width.
        let y = if crate::pinned_screenshot::zoom::active(id) {
            y
        } else {
            crate::pinned_screenshot::geometry(id).map_or(y, |(s, d)| {
                y * (s.bottom - s.top) as f32 / (d.bottom - d.top) as f32
                    * (d.right - d.left) as f32
                    / (s.right - s.left) as f32
            })
        };
        canvas_pointer(0, event, x, y);
        let enabled = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|r| r.gesture.is_none() && r.pending_drawing.is_none())
        });
        crate::pinned_screenshot::set_recognition_entry(id, enabled);
    }
}
/// Retire only the gesture belonging to the focus-losing edit instance.
pub(crate) fn lost_focus(id: u64) {
    crate::pinned_screenshot::zoom::lost_focus(id);
    if let Some(owner) = current().filter(|p| p.0 == id) {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if current() == Some(owner) {
                pointer(id, 3, 0., 0.);
            }
        });
    }
}
pub(crate) fn key(id: u64, key: i32) {
    if crate::pinned_screenshot::recognizing(id) || !current().is_some_and(|p| p.0 == id) {
        return;
    }
    if crate::pinned_screenshot::zoom::panning(id) {
        return;
    }
    match key {
        0 => history_key(1),
        4 => history_key(2),
        1 => delete_selected(),
        2 => finish_polyline(),
        3 => backspace_polyline(),
        _ => {}
    }
}
pub(crate) fn scroll(id: u64, delta: f32) {
    if pin_crop::active() || crate::pinned_screenshot::recognition_input_paused(id) {
        return;
    }
    if current().is_some_and(|p| p.0 == id) {
        brush_size_scrolled(0, delta);
    }
}
pub(crate) fn escape(id: u64) {
    if crate::pinned_screenshot::zoom::cancel_pan(id) {
        return;
    }
    if current().is_some_and(|p| p.0 == id) {
        escape_from(current_focus_origin());
    }
}

pub(crate) fn browse(id: u64) -> bool {
    current().is_some_and(|p| p.0 == id)
        && REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|r| r.document.mode.is_mouse())
        })
}
pub(crate) fn navigation_key(id: u64, down: bool) -> bool {
    if current().is_none_or(|p| p.0 != id) {
        return false;
    }
    if !down {
        crate::pinned_screenshot::zoom::space(id, false);
        return true;
    }
    let allowed = REGISTRY.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            r.editor.window().is_none()
                && r.toolbar_state.menus.current.is_none()
                && r.toolbar_state.watermark.get().is_none()
                && r.gesture.is_none()
                && r.pending_drawing.is_none()
        })
    });
    if !allowed
        || pin_crop::active()
        || crate::pinned_screenshot::recognizing(id)
        || crate::pinned_screenshot::zoom::panning(id)
    {
        return true;
    }
    if let Some(tool) = crate::pinned_screenshot::zoom::space(id, down) {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                if let Some(tool) = tool {
                    if tool == usize::MAX {
                        r.select_tool_variant(1, 3);
                    } else {
                        r.set_mode(InteractionMode::Tool(tool));
                    }
                } else {
                    r.set_mode(InteractionMode::Mouse);
                }
            }
        });
    }
    true
}
pub(super) fn sync_view_mode(r: &Registry) {
    if let Some((id, _)) = r.screenshot.as_ref().and_then(|e| e.pin) {
        crate::pinned_screenshot::zoom::mode(
            id,
            r.document.mode.is_mouse(),
            if let InteractionMode::Tool(tool) = r.document.mode {
                Some(tool)
            } else {
                None
            },
        );
    }
}
pub(super) fn input_point(id: u64, x: f32, y: f32) -> Option<(f32, f32)> {
    let w = crate::pinned_screenshot::window_for_view(id)?;
    let dpi = f64::from(w.window().scale_factor());
    let p = crate::pinned_screenshot::zoom::local(id, [f64::from(x) * dpi, f64::from(y) * dpi])?;
    DOCUMENTS.with(|s| {
        let s = s.borrow();
        let pinfo = s.get(&id)?;
        let o = pinfo.presented_orientation;
        let p = o
            .inverse()
            .point(p, o.rect(pinfo.base.bounds, pinfo.base.bounds).ok()?);
        Some((p[0] as f32, p[1] as f32))
    })
}
pub(crate) fn shutdown() {
    pin_crop::clear();
    ACTIVE.with(|s| s.borrow_mut().take());
    DOCUMENTS.with(|s| s.borrow_mut().clear());
}

/// Publish input geometry only with the pixels belonging to this document.
pub(super) fn presented(id: u64, session: &Session) {
    if let Some(full) = source(id) {
        crate::pinned_screenshot::zoom::precision(id, full, session.image_orientation);
    }
    DOCUMENTS.with(|s| {
        if let Some(p) = s.borrow_mut().get_mut(&id) {
            p.presented_orientation = session.image_orientation;
        }
    });
}

/// Install the same committed snapshot for inactive storage and the borrowed edit surface.
pub(crate) fn install_direction(id: u64, document: &ExportRequest) {
    let session = DOCUMENTS
        .with(|s| {
            if let Some(p) = s.borrow_mut().get_mut(&id) {
                p.presented_orientation = document.snapshot.session.image_orientation;
                p.document.session = (*document.snapshot.session).clone();
                p.document.session.image_source = Some(p.base.bounds);
                p.request = document.clone();
                return Some(p.document.session.clone());
            }
            None
        })
        .unwrap_or_else(|| (*document.snapshot.session).clone());
    if let Some(full) = source(id) {
        crate::pinned_screenshot::zoom::precision(id, full, session.image_orientation);
    }
    if current().is_some_and(|p| p.0 == id) {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                r.document.session = session;
                r.sync_edit_scope();
                restore_parameter_projection(r);
                cursor::refresh(r);
                if let Some(e) = r.screenshot.as_mut() {
                    e.revision = e.revision.wrapping_add(1);
                }
                sync_history_controls(r);
                r.queue_render();
            }
        });
    }
}

/// Pin input uses the same immutable orientation and visible source as the presented pixels.
fn mapped_point(
    point: (f32, f32),
    reverse: bool,
    full: Rect,
    source: Rect,
    display: Rect,
    orientation: lexift_core::domain::image_transform::Orientation,
) -> Option<(f32, f32)> {
    let oriented = if reverse {
        point
    } else {
        let p = orientation.point([point.0 as f64, point.1 as f64], full);
        (p[0] as f32, p[1] as f32)
    };
    let (from, to) = if reverse {
        (display, source)
    } else {
        (source, display)
    };
    let p = (
        to.left as f32
            + (oriented.0 - from.left as f32) * (to.right - to.left) as f32
                / (from.right - from.left) as f32,
        to.top as f32
            + (oriented.1 - from.top as f32) * (to.bottom - to.top) as f32
                / (from.bottom - from.top) as f32,
    );
    if reverse {
        let p = orientation
            .inverse()
            .point([p.0 as f64, p.1 as f64], orientation.rect(full, full).ok()?);
        Some((p[0] as f32, p[1] as f32))
    } else {
        Some(p)
    }
}
/// Native text input uses the same bounded client area as image feedback.
pub(super) fn clip_text(mut bounds: Bounds) -> Bounds {
    if let Some((id, _)) = current().filter(|(id, _)| crate::pinned_screenshot::zoom::active(*id))
        && let Some((_, display)) = crate::pinned_screenshot::geometry(id)
    {
        bounds.left = bounds
            .left
            .clamp(display.left as f32, display.right as f32 - 1.);
        bounds.top = bounds
            .top
            .clamp(display.top as f32, display.bottom as f32 - 1.);
        bounds.right = bounds.right.clamp(bounds.left + 1., display.right as f32);
        bounds.bottom = bounds.bottom.clamp(bounds.top + 1., display.bottom as f32);
    }
    bounds
}

#[cfg(test)]
mod orientation_tests {
    use super::*;
    #[test]
    fn initial_core_retention_uses_editable_history_but_history_restore_starts_empty() {
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let full = Rect {
            left: 0,
            top: 0,
            right: 10,
            bottom: 20,
        };
        let crop = Rect {
            left: 1,
            top: 2,
            right: 9,
            bottom: 18,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        session.set_image_crop(crop, full);
        session.orient_image(
            session
                .image_orientation
                .action(lexift_core::domain::image_transform::Action::Right),
        );
        REGISTRY.with(|s| s.borrow_mut().as_mut().unwrap().document.session = session.clone());
        let base = Arc::new(CapturedRegion {
            bounds: full,
            pixels: [10, 20, 30, 255].repeat(200),
        });
        let samples = Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![],
        });
        let request = ExportRequest {
            session: 42,
            request: 3,
            snapshot: Arc::new(lexift_core::domain::annotation_export::ExportSnapshot {
                session: Arc::new(session.restored_image()),
                monitor: crop,
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        install(91, base.clone(), samples.clone(), request.clone());
        let document = retained_document(91).unwrap();
        assert_eq!(document.session, 0);
        let mut restored = (*document.snapshot.session).clone();
        assert!(restored.undo());
        assert_eq!(restored.image_crop, Some(crop));
        assert!(restored.undo());
        assert_eq!(restored.image_crop, None);
        install_history(92, base, samples, request);
        let restored = retained_document(92).unwrap();
        assert!(!restored.snapshot.session.can_undo());
        assert_eq!(restored.snapshot.session.image_crop, Some(crop));
        forget(91);
        forget(92);
        REGISTRY.with(|s| s.borrow_mut().take());
    }
    #[test]
    fn presented_pin_mapping_roundtrips_crop_negative_origin_and_dpi() {
        let full = Rect {
            left: -300,
            top: -50,
            right: 300,
            bottom: 1150,
        };
        let crop = Rect {
            left: -230,
            top: 30,
            right: 140,
            bottom: 950,
        };
        for direction in lexift_core::domain::image_transform::Orientation::ALL {
            let source = direction.rect(crop, full).unwrap();
            for scale in [0.4, 1., 1.25] {
                let display = Rect {
                    left: 2700,
                    top: 30,
                    right: 2700 + ((source.right - source.left) as f32 * scale) as i32,
                    bottom: 30 + ((source.bottom - source.top) as f32 * scale) as i32,
                };
                for p in [(-230., 30.), (140., 950.), (-70., 420.)] {
                    let screen = mapped_point(p, false, full, source, display, direction).unwrap();
                    let back =
                        mapped_point(screen, true, full, source, display, direction).unwrap();
                    assert!((p.0 - back.0).abs() < 0.01 && (p.1 - back.1).abs() < 0.01);
                }
            }
        }
    }
}
