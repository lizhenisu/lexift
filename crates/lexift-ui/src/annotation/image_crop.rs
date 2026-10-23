//! Instance-owned crop drafts. Only applying a draft changes the Core document.
use super::*;
use lexift_core::domain::image_crop::Crop;

struct Draft {
    window: slint::Weak<AnnotationCanvas>,
    crop: Crop,
    image: Rect,
}
thread_local! { static DRAFT: RefCell<Option<Draft>> = const { RefCell::new(None) }; }
pub(super) fn active() -> bool {
    pin_crop::active() || DRAFT.with(|s| s.borrow().is_some())
}
pub(super) fn clear() {
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
        if editor_ocr::busy()
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
        sync(r, true);
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
    sync(r, true);
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
        if apply {
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
    w.set_image_cropping(draft.is_some());
    w.set_content_left(visible.left as f32);
    w.set_content_top(visible.top as f32);
    w.set_content_right(visible.right as f32);
    w.set_content_bottom(visible.bottom as f32);
    let b = draft.unwrap_or(visible);
    w.set_crop_left(b.left as f32);
    w.set_crop_top(b.top as f32);
    w.set_crop_right(b.right as f32);
    w.set_crop_bottom(b.bottom as f32);
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_image_cropping(draft.is_some());
        main.set_can_undo(!active() && r.document.session.can_undo());
        main.set_can_redo(!active() && r.document.session.can_redo());
    }
    image_navigation::content(&w, visible, fit);
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
