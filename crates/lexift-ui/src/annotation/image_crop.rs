//! Instance-owned crop drafts. Only applying a draft changes the Core document.
use super::*;
use lexift_core::domain::image_crop::Crop;

struct Draft {
    window: slint::Weak<AnnotationCanvas>,
    crop: Crop,
    image: Rect,
}
thread_local! { static DRAFT: RefCell<Option<Draft>> = const { RefCell::new(None) }; static APPLYING:RefCell<Option<Draft>>=const{RefCell::new(None)}; }
pub(super) fn active() -> bool {
    pin_crop::active() || DRAFT.with(|s| s.borrow().is_some())
}
pub(super) fn clear() {
    APPLYING.with(|s| s.borrow_mut().take());
    DRAFT.with(|s| s.borrow_mut().take());
}
pub(super) fn begin() {
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return };
        if pin_editor::active() {
            pin_crop::begin(r);
            return;
        }
        if image_resize::busy()
            || editor_ocr::busy()
            || active()
            || !screenshot::long::image_editing()
            || screenshot::long_editor::outputting()
            || r.gesture().is_some()
            || !r.pending_drawing.is_none()
            || r.toolbar_state.menus.current.is_some()
        {
            return;
        }
        let Some(w) = screenshot::long::editor() else {
            return;
        };
        if w.get_image_panning() {
            return;
        }
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, true);
        }
        r.finish_style_edit();
        if !r.set_mode(InteractionMode::Mouse) {
            return;
        }
        r.close_panel();
        r.hints.clear();
        let Some(image) = r.screenshot.as_ref().and_then(|e| e.region.bounds()) else {
            return;
        };
        DRAFT.with(|s| {
            *s.borrow_mut() = Some(Draft {
                window: w.as_weak(),
                crop: Crop::new(image, r.document.session.image_crop),
                image,
            })
        });
        if r.document.session.image_scale == Default::default() {
            sync(r, true);
        }
        retire_frames(r);
        r.queue_render();
    });
}
pub(super) fn cancel(r: &mut Registry) -> bool {
    if pin_crop::cancel(r) {
        return true;
    }
    if !active() {
        return false;
    }
    clear();
    if r.document.session.image_scale == Default::default() {
        sync(r, true);
    }
    retire_frames(r);
    r.queue_render();
    true
}
pub(super) fn finish(apply: bool) {
    if pin_crop::active() {
        pin_crop::finish(apply);
        return;
    }
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return };
        if apply
            && DRAFT.with(|s| {
                s.borrow().as_ref().is_some_and(|d| {
                    r.document
                        .session
                        .image_scale
                        .validate(d.image, d.crop.bounds, r.document.session.image_orientation)
                        .is_err()
                })
            })
        {
            mosaic::notify(r, "Enter valid image dimensions (maximum 40 MP)");
            return;
        }
        let Some(d) = DRAFT.with(|s| s.borrow_mut().take()) else {
            return;
        };
        if !d.window.upgrade().is_some_and(|w| {
            r.canvas_state
                .canvases
                .iter()
                .any(|c| std::ptr::eq(c.window.window(), w.window()))
        }) {
            return;
        }
        if apply && r.document.session.image_scale != Default::default() {
            let mut candidate = r.document.session.clone();
            if candidate.set_image_crop(d.crop.bounds, d.image) {
                APPLYING.with(|s| *s.borrow_mut() = Some(d));
                image_transform::submit(r, candidate);
                return;
            }
        } else if apply {
            r.document.session.set_image_crop(d.crop.bounds, d.image);
        }
        sync(r, true);
        r.queue_render();
    });
}
pub(super) fn sync(r: &Registry, fit: bool) {
    let Some(w) = screenshot::long::editor() else {
        return;
    };
    let Some(full) = r.screenshot.as_ref().and_then(|e| e.region.bounds()) else {
        return;
    };
    let draft = DRAFT.with(|s| s.borrow().as_ref().map(|d| d.crop.bounds));
    let visible = if draft.is_some() {
        full
    } else {
        r.document.session.image_crop.unwrap_or(full)
    };
    image_navigation::orient(&w, full, r.document.session.image_orientation);
    let oriented_visible = r
        .document
        .session
        .image_orientation
        .rect(visible, full)
        .unwrap_or(visible);
    w.set_image_cropping(draft.is_some());
    w.set_content_left(oriented_visible.left as f32);
    w.set_content_top(oriented_visible.top as f32);
    w.set_content_right(oriented_visible.right as f32);
    w.set_content_bottom(oriented_visible.bottom as f32);
    let b = draft.unwrap_or(visible);
    let b = r
        .document
        .session
        .image_orientation
        .rect(b, full)
        .unwrap_or(b);
    w.set_crop_left(b.left as f32);
    w.set_crop_top(b.top as f32);
    w.set_crop_right(b.right as f32);
    w.set_crop_bottom(b.bottom as f32);
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_image_cropping(draft.is_some());
        main.set_can_undo(!active() && r.document.session.can_undo());
        main.set_can_redo(!active() && r.document.session.can_redo());
    }
    image_navigation::content(
        &w,
        visible,
        fit,
        if draft.is_some() {
            Default::default()
        } else {
            r.document.session.image_scale
        },
    );
}
/// Input is already instance-checked and converted through the shared viewport.
pub(super) fn pointer(w: &AnnotationCanvas, kind: i32, p: [f32; 2], tolerance: f64) -> bool {
    DRAFT.with(|s| {
        let mut s = s.borrow_mut();
        let Some(d) = s.as_mut() else { return false };
        if !d
            .window
            .upgrade()
            .is_some_and(|owner| std::ptr::eq(owner.window(), w.window()))
        {
            return false;
        }
        let p = p.map(f64::from);
        match kind {
            0 => d.crop.begin(p, tolerance),
            1 => d.crop.update(p),
            2 => {
                d.crop.update(p);
                d.crop.finish();
            }
            3 => d.crop.cancel_drag(),
            _ => {}
        }
        let b = d.crop.bounds;
        w.set_crop_left(b.left as f32);
        w.set_crop_top(b.top as f32);
        w.set_crop_right(b.right as f32);
        w.set_crop_bottom(b.bottom as f32);
        let cursor = match d.crop.hit(p, tolerance) {
            Some([0, 0]) => crate::AnnotationCursor::Moving,
            Some([0, _]) => crate::AnnotationCursor::Vertical,
            Some([_, 0]) => crate::AnnotationCursor::Horizontal,
            Some([-1, -1] | [1, 1]) => crate::AnnotationCursor::DiagonalDown,
            Some(_) => crate::AnnotationCursor::DiagonalUp,
            None => crate::AnnotationCursor::Drawing,
        };
        w.set_crop_cursor(cursor);
        true
    })
}
pub(super) fn dragging() -> bool {
    DRAFT.with(|s| s.borrow().as_ref().is_some_and(|d| d.crop.dragging()))
}
pub(super) fn lost_focus() {
    DRAFT.with(|s| {
        if let Some(d) = s.borrow_mut().as_mut() {
            d.crop.cancel_drag();
            if let Some(w) = d.window.upgrade() {
                let b = d.crop.bounds;
                w.set_crop_left(b.left as f32);
                w.set_crop_top(b.top as f32);
                w.set_crop_right(b.right as f32);
                w.set_crop_bottom(b.bottom as f32);
            }
        }
    });
}

pub(super) fn candidate_done() {
    APPLYING.with(|s| s.borrow_mut().take());
}
pub(super) fn candidate_failed() {
    if let Some(d) = APPLYING.with(|s| s.borrow_mut().take()) {
        DRAFT.with(|s| *s.borrow_mut() = Some(d));
    }
}

fn retire_frames(r: &mut Registry) {
    if let Some(e) = r.screenshot.as_mut() {
        e.revision = e.revision.saturating_add(1);
        e.accepted_after = e.revision;
    }
}
