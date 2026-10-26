//! Transactional pin geometry presentation. Drafts and candidate histories never mutate live pixels.
use super::*;
use lexift_core::domain::{
    annotation_export::ExportRequest,
    image_crop::Crop,
    pinned_image::{Event, States},
};
use lexift_core::ports::screen_capture::CapturedRegion;
use std::sync::Arc;

struct Pending {
    session: Session,
    request: ExportRequest,
    revision: Option<u64>,
}
struct Edit {
    id: u64,
    edit: u64,
    window: slint::Weak<crate::PinnedScreenshotWindow>,
    source: Rect,
    before: Rect,
    original_before: Rect,
    orientation: lexift_core::domain::image_transform::Orientation,
    position: slint::PhysicalPosition,
    size: slint::PhysicalSize,
    zoom: f32,
    picture: slint::Image,
    draft: Option<Crop>,
    entering: bool,
    pending: Option<Pending>,
}
thread_local! { static EDIT:RefCell<Option<Edit>>=const{RefCell::new(None)}; }
pub(super) fn active() -> bool {
    EDIT.with(|s| s.borrow().is_some())
}
pub(super) fn clear() {
    EDIT.with(|s| s.borrow_mut().take());
}
fn saved(r: &Registry) -> Option<Edit> {
    let (id, edit) = pin_editor::current()?;
    let canvas = r.canvas_state.canvases.first()?;
    let canvas::Surface::Pin(w) = &canvas.window else {
        return None;
    };
    let source = pin_editor::source(id)?;
    let (before, display) = crate::pinned_screenshot::geometry(id)?;
    Some(Edit {
        id,
        edit,
        window: w.as_weak(),
        source,
        before,
        original_before: r.document.session.image_crop.unwrap_or(source),
        orientation: r.document.session.image_orientation,
        position: w.window().position(),
        size: w.window().size(),
        zoom: (display.right - display.left) as f32 / (before.right - before.left) as f32,
        picture: w.get_picture(),
        draft: None,
        entering: false,
        pending: None,
    })
}
pub(super) fn begin(r: &mut Registry) {
    if pin_editor::current().is_some_and(|p| crate::pinned_screenshot::policy::locked(p.0))
        || active()
        || r.gesture().is_some()
        || !r.pending_drawing.is_none()
        || r.toolbar_state.menus.current.is_some()
        || r.toolbar_state
            .main
            .get()
            .is_some_and(|m| m.get_output_busy())
    {
        return;
    }
    let Some(mut e) = saved(r) else { return };
    if r.editor.window().is_some() {
        finish_text_edit_registry(r, true);
    }
    r.finish_style_edit();
    if !r.set_mode(InteractionMode::Mouse) {
        return;
    }
    r.close_panel();
    r.hints.clear();
    e.draft = Some(Crop::new(e.source, r.document.session.image_crop));
    e.entering = true;
    let full = e.source;
    EDIT.with(|s| *s.borrow_mut() = Some(e));
    source(r, full);
    toolbar(r);
    r.queue_render();
}
fn source(r: &mut Registry, bounds: Rect) {
    if let Some(c) = r.canvas_state.canvases.first_mut() {
        c.host.bounds = bounds;
        c.last_pointer = None;
    }
    if let Some(e) = r.screenshot.as_mut() {
        // Retire queued pixels immediately, before the coalesced replacement render runs.
        e.revision = e.revision.wrapping_add(1);
        e.region = lexift_core::domain::region_selection::RegionEditor::new(bounds);
    }
}
fn toolbar(r: &Registry) {
    EDIT.with(|s| {
        let s = s.borrow();
        if let Some(m) = r.toolbar_state.main.get() {
            m.set_image_cropping(s.as_ref().is_some_and(|e| e.draft.is_some()));
            m.set_crop_pending(
                s.as_ref()
                    .is_some_and(|e| e.entering || e.pending.is_some()),
            );
            if let Some(b) = s.as_ref().and_then(|e| e.draft.as_ref().map(|d| d.bounds)) {
                m.set_crop_dimensions(
                    format!("{} × {} px", b.right - b.left, b.bottom - b.top).into(),
                );
            }
        }
    });
    sync_history_controls(r);
}
fn geometry_with(
    r: &Registry,
    e: &Edit,
    w: &crate::PinnedScreenshotWindow,
    b: Rect,
    fit: bool,
) -> bool {
    if let Some(result) = crate::pinned_screenshot::zoom::reframe(e.id, b, false) {
        return result;
    }
    let inset = (w.get_frame_inset() * w.window().scale_factor()).round() as i32;
    let area = (r.lifecycle.popup_work_area)(Point {
        x: e.position.x + inset,
        y: e.position.y + inset,
    })
    .unwrap_or(Rect {
        left: e.position.x,
        top: e.position.y,
        right: e.position.x + e.size.width as i32,
        bottom: e.position.y + e.size.height as i32,
    });
    let width = (b.right - b.left) as f32;
    let height = (b.bottom - b.top) as f32;
    let available = [
        (area.right - area.left - 2 * inset).max(1) as f32,
        (area.bottom - area.top - 2 * inset).max(1) as f32,
    ];
    let z = if fit {
        e.zoom.min(available[0] / width).min(available[1] / height)
    } else {
        e.zoom
    };
    // Preserve both rounded native dimensions when restoring the same range.
    // Reconstructing height from width can otherwise lose a pixel after a DPI change.
    let size = if b == e.before && z == e.zoom {
        e.size
    } else {
        slint::PhysicalSize::new(
            (width * z).round().max(1.) as u32 + 2 * inset as u32,
            (height * z).round().max(1.) as u32 + 2 * inset as u32,
        )
    };
    let pos = slint::PhysicalPosition::new(
        e.position
            .x
            .clamp(area.left, (area.right - size.width as i32).max(area.left)),
        e.position
            .y
            .clamp(area.top, (area.bottom - size.height as i32).max(area.top)),
    );
    let _guard = r.pause_native_pointer_inputs();
    crate::pinned_screenshot::set_geometry(e.id, b, pos, size)
}
/// The full-image preview arrived before its temporary window geometry is exposed.
pub(super) fn preview_ready(r: &Registry, id: u64) -> bool {
    let failed = EDIT.with(|s| {
        if let Some(e) = s.borrow_mut().as_mut().filter(|e| e.id == id && e.entering) {
            if let Some(w) = e.window.upgrade() {
                if !crate::pinned_screenshot::zoom::crop_begin(
                    id,
                    e.orientation.rect(e.source, e.source).unwrap_or(e.source),
                )
                .unwrap_or_else(|| {
                    geometry_with(
                        r,
                        e,
                        &w,
                        e.orientation.rect(e.source, e.source).unwrap_or(e.source),
                        true,
                    )
                }) {
                    return Some((e.id, e.edit));
                }
                w.set_cropping(true);
            }
            e.entering = false;
        }
        None
    });
    if let Some(owner) = failed {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if pin_editor::current() == Some(owner) {
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut() {
                        cancel(r);
                    }
                });
            }
        });
        return false;
    }
    if EDIT.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|e| e.id == id && e.draft.is_some())
    }) {
        let _guard = r.pause_native_pointer_inputs();
        crate::pinned_screenshot::set_edit_interaction(id, true);
    }
    feedback(r);
    toolbar(r);
    true
}
pub(super) fn pointer(id: u64, event: i32, x: f32, y: f32) -> bool {
    let consumed = EDIT.with(|s| {
        let mut s = s.borrow_mut();
        let Some(e) = s.as_mut().filter(|e| e.id == id) else {
            return false;
        };
        if e.entering || e.pending.is_some() {
            return true;
        }
        let Some(c) = e.draft.as_mut() else {
            return true;
        };
        let Some(w) = e.window.upgrade() else {
            return true;
        };
        let Some((b, d)) = crate::pinned_screenshot::geometry(id) else {
            return true;
        };
        let z = (d.right - d.left) as f64 / (b.right - b.left) as f64;
        let p = [
            f64::from(b.left) + f64::from(x * w.window().scale_factor()) / z,
            f64::from(b.top)
                + f64::from(y * w.window().scale_factor()) * f64::from(b.bottom - b.top)
                    / f64::from(d.bottom - d.top),
        ];
        let p = crate::pinned_screenshot::zoom::local(
            id,
            [
                f64::from(x * w.window().scale_factor()),
                f64::from(y * w.window().scale_factor()),
            ],
        )
        .unwrap_or(p);
        let z = crate::pinned_screenshot::zoom::scale(id).unwrap_or(z);
        let p = e.orientation.inverse().point(
            p,
            e.orientation.rect(e.source, e.source).unwrap_or(e.source),
        );
        match event {
            0 => c.begin(p, 6. / z),
            1 => c.update(p),
            2 => {
                c.update(p);
                c.finish();
            }
            3 => c.cancel_drag(),
            _ => {}
        }
        let cursor = match c.hit(p, 6. / z) {
            Some([0, 0]) if c.bounds != e.source => crate::AnnotationCursor::Moving,
            Some([0, 0]) | None => crate::AnnotationCursor::Drawing,
            Some([0, _]) => crate::AnnotationCursor::Vertical,
            Some([_, 0]) => crate::AnnotationCursor::Horizontal,
            Some([-1, -1] | [1, 1]) => crate::AnnotationCursor::DiagonalDown,
            Some(_) => crate::AnnotationCursor::DiagonalUp,
        };
        w.set_cursor(cursor);
        true
    });
    if consumed {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow().as_ref() {
                feedback(r);
                toolbar(r);
            }
        });
    }
    consumed
}
fn feedback(_r: &Registry) {
    EDIT.with(|s| {
        if let Some(e) = s.borrow().as_ref()
            && let (Some(w), Some(c)) = (e.window.upgrade(), e.draft.as_ref())
        {
            let b = e.orientation.rect(c.bounds, e.source).unwrap_or(c.bounds);
            let full = e.orientation.rect(e.source, e.source).unwrap_or(e.source);
            if let Some((a, b, size)) = crate::pinned_screenshot::zoom::crop_feedback(e.id, b) {
                w.set_crop_left(a[0] as f32);
                w.set_crop_top(a[1] as f32);
                w.set_crop_right(b[0] as f32);
                w.set_crop_bottom(b[1] as f32);
                w.set_crop_source_width(size[0] as f32);
                w.set_crop_source_height(size[1] as f32);
                return;
            }
            w.set_crop_left((b.left - full.left) as f32);
            w.set_crop_top((b.top - full.top) as f32);
            w.set_crop_right((b.right - full.left) as f32);
            w.set_crop_bottom((b.bottom - full.top) as f32);
            w.set_crop_source_width((full.right - full.left) as f32);
            w.set_crop_source_height((full.bottom - full.top) as f32);
        }
    });
}
pub(super) fn finish(apply: bool) {
    if !apply {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                cancel(r);
            }
        });
        return;
    }
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return };
        let candidate = EDIT.with(|s| {
            let s = s.borrow();
            let e = s.as_ref()?;
            if e.entering || e.pending.is_some() {
                return None;
            }
            let mut session = r.document.session.clone();
            session.set_image_crop(e.draft.as_ref()?.bounds, e.source);
            Some(session)
        });
        if let Some(candidate) = candidate {
            if candidate.image_crop == r.document.session.image_crop {
                cancel(r);
                return;
            }
            propose(r, candidate);
        }
    });
}
fn propose(r: &Registry, session: Session) {
    if let Some((id, _)) =
        pin_editor::current().filter(|p| crate::pinned_screenshot::policy::locked(p.0))
    {
        let full = pin_editor::source(id);
        let size = |s: &Session| {
            full.and_then(|f| s.image_orientation.rect(s.image_crop.unwrap_or(f), f).ok())
                .map(|b| (b.right - b.left, b.bottom - b.top))
        };
        if size(&session) != size(&r.document.session)
            || session.image_orientation != r.document.session.image_orientation
        {
            return;
        }
    }
    let Some(request) = pin_editor::propose(r, session.clone()) else {
        return;
    };
    let event = EDIT.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            *s = saved(r);
        }
        let e = s.as_mut()?;
        if e.pending.is_some() {
            return None;
        }
        e.pending = Some(Pending {
            session,
            request: request.clone(),
            revision: None,
        });
        Some(Event::DocumentProposed {
            id: e.id,
            edit: e.edit,
            document: request,
        })
    });
    toolbar(r);
    if let Some(event) = event {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Event::DocumentProposed { document, .. } = &event
                && EDIT.with(|s| {
                    s.borrow()
                        .as_ref()
                        .and_then(|e| e.pending.as_ref())
                        .is_some_and(|p| p.request == *document)
                })
            {
                emit_pin(event);
            }
        });
    }
}
pub(super) fn history(r: &Registry, session: Session) {
    propose(r, session);
}
pub(super) fn cancel(r: &mut Registry) -> bool {
    let Some(e) = EDIT.with(|s| s.borrow_mut().take()) else {
        return false;
    };
    emit_pin_later(Event::ProposalCancelled {
        id: e.id,
        edit: e.edit,
        request: e.pending.as_ref().map_or(0, |p| p.request.request),
    });
    if let Some(w) = e.window.upgrade() {
        let _guard = r.pause_native_pointer_inputs();
        (r.lifecycle.cancel_window_interaction)(w.window());
        w.set_cropping(false);
        if crate::pinned_screenshot::zoom::active(e.id) {
            crate::pinned_screenshot::zoom::crop_cancel(e.id);
        } else {
            geometry_with(r, &e, &w, e.before, true);
        }
        w.set_picture(e.picture);
        w.set_decoration(slint::Image::default());
        crate::pinned_screenshot::set_edit_interaction(e.id, !r.document.mode.is_mouse());
    }
    source(r, e.original_before);
    toolbar(r);
    r.queue_render();
    true
}
fn emit_pin_later(event: Event) {
    slint::Timer::single_shot(Duration::ZERO, move || emit_pin(event));
}
pub(super) fn sync_state(states: &States) {
    let failed = EDIT.with(|s| {
        let mut s = s.borrow_mut();
        let Some(e) = s.as_mut() else { return false };
        let Some(p) = states.get(&e.id) else {
            return false;
        };
        if let Some(pending) = e.pending.as_mut()
            && (p.proposed.as_ref() == Some(&pending.request)
                || p.document.as_ref() == Some(&pending.request))
        {
            pending.revision = Some(p.revision);
        }
        if e.pending.as_ref().is_some_and(|v| {
            v.revision == Some(p.revision)
                && p.proposed.is_none()
                && p.error.is_some()
                && p.document.as_ref().is_none_or(|d| d != &v.request)
        }) {
            e.pending = None;
            if e.draft.is_none() {
                *s = None;
            }
            return true;
        }
        false
    });
    if failed {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow().as_ref() {
                toolbar(r);
            }
        });
    }
}
pub(super) fn present(id: u64, revision: u64, image: Arc<CapturedRegion>) -> bool {
    let candidate = EDIT.with(|s| {
        s.borrow().as_ref().is_some_and(|e| {
            e.id == id
                && e.pending
                    .as_ref()
                    .is_some_and(|p| p.revision == Some(revision))
        })
    });
    if !candidate {
        return false;
    }
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return false };
        if pin_editor::current().is_none_or(|p| p.0 != id) {
            return false;
        }
        let Some(mut e) = EDIT.with(|s| s.borrow_mut().take()) else {
            return false;
        };
        let Some(p) = e.pending.take() else {
            return false;
        };
        if pin_editor::current() != Some((id, e.edit))
            || p.session
                .image_orientation
                .rect(p.request.snapshot.monitor, e.source)
                .ok()
                != Some(image.bounds)
        {
            e.pending = Some(p);
            EDIT.with(|s| *s.borrow_mut() = Some(e));
            return false;
        }
        if let Some(w) = e.window.upgrade() {
            let color_only = r.document.session.image_crop == p.session.image_crop
                && r.document.session.image_orientation == p.session.image_orientation;
            let positioned = if color_only {
                true
            } else if e.orientation != p.session.image_orientation {
                crate::pinned_screenshot::transform::geometry(id, image.bounds)
            } else {
                geometry_with(r, &e, &w, image.bounds, true)
            };
            if !positioned {
                e.pending = Some(p);
                EDIT.with(|s| *s.borrow_mut() = Some(e));
                emit_pin_later(Event::PreviewFailed {
                    id,
                    revision,
                    error: crate::i18n::tr("Screenshot unavailable"),
                });
                return false;
            }
            w.set_cropping(false);
            let _guard = r.pause_native_pointer_inputs();
            crate::pinned_screenshot::set_edit_interaction(id, !r.document.mode.is_mouse());
        }
        let original = p.request.snapshot.monitor;
        pin_editor::presented(id, &p.session);
        r.document.session = p.session;
        source(r, original);
        toolbar(r);
        restore_parameter_projection(r);
        cursor::refresh(r);
        emit_pin_later(Event::ProposalAccepted {
            id,
            edit: e.edit,
            request: p.request.request,
            revision,
        });
        r.queue_render();
        true
    })
}

pub(super) fn waiting() -> bool {
    EDIT.with(|s| s.borrow().as_ref().is_some_and(|e| e.pending.is_some()))
}
