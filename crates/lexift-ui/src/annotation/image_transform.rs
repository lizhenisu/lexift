//! Candidate orientation is installed with its prepared base and feedback, never ahead of pixels.
use super::*;
use lexift_core::domain::image_transform::Action;
struct Pending {
    generation: u64,
    revision: u64,
    session: Session,
}
thread_local! {static PENDING:RefCell<Option<Pending>>=const {RefCell::new(None)};}
pub(super) fn clear() {
    PENDING.with(|s| s.borrow_mut().take());
    screenshot::delay::clear();
}
pub(super) fn busy() -> bool {
    PENDING.with(|s| s.borrow().is_some())
}
pub(super) fn available() -> bool {
    screenshot::long::image_editing()
        && !busy()
        && !editor_ocr::busy()
        && !image_crop::active()
        && !screenshot::long_editor::outputting()
}
pub(super) fn preview(r: &Registry) -> Option<Session> {
    PENDING.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|p| p.generation == r.generation)
            .map(|p| p.session.clone())
    })
}
pub(super) fn track(generation: u64, revision: u64) {
    PENDING.with(|s| {
        if let Some(p) = s
            .borrow_mut()
            .as_mut()
            .filter(|p| p.generation == generation)
        {
            p.revision = revision;
        }
    });
}
pub(super) fn current(generation: u64, revision: u64) -> bool {
    PENDING.with(|s| {
        s.borrow()
            .as_ref()
            .is_none_or(|p| p.generation == generation && p.revision == revision)
    })
}
pub(super) fn apply(index: i32) {
    if !available() {
        return;
    }
    let Some(action) = [
        Action::Left,
        Action::Right,
        Action::Horizontal,
        Action::Vertical,
    ]
    .get(index as usize)
    .copied() else {
        return;
    };
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.gesture().is_some()
            || !r.pending_drawing.is_none()
            || screenshot::long::editor().is_none_or(|w| w.get_image_panning())
        {
            return;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.close_panel();
        r.hints.clear();
        let mut candidate = r.document.session.clone();
        if !candidate.orient_image(candidate.image_orientation.action(action)) {
            return;
        }
        submit(r, candidate);
    });
}
pub(super) fn ready(r: &mut Registry, revision: u64) {
    let pending = PENDING.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref()
            .is_some_and(|p| p.generation == r.generation && p.revision == revision)
        {
            s.take()
        } else {
            None
        }
    });
    if let Some(p) = pending {
        r.document.session = p.session;
        r.set_mode(InteractionMode::Mouse);
        image_crop::sync(r, true);
        sync_history_controls(r);
        if let Some(m) = r.toolbar_state.main.get() {
            m.set_output_busy(false);
        }
    }
}
pub(super) fn failed(r: &mut Registry) -> bool {
    let failed = PENDING.with(|s| s.borrow_mut().take().is_some());
    if failed {
        if let Some(m) = r.toolbar_state.main.get() {
            m.set_output_busy(false);
        }
        mosaic::notify(r, "Operation failed. Try again.");
        r.queue_render();
    }
    failed
}

pub(super) fn submit(r: &mut Registry, session: Session) {
    PENDING.with(|s| {
        *s.borrow_mut() = Some(Pending {
            generation: r.generation,
            revision: 0,
            session,
        })
    });
    if let Some(m) = r.toolbar_state.main.get() {
        m.set_output_busy(true);
    }
    r.queue_render();
}
