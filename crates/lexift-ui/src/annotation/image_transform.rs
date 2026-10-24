//! Candidate orientation is installed with its prepared base and feedback, never ahead of pixels.
use super::*;
use lexift_core::domain::image_transform::Action;
struct Pending {
    generation: u64,
    owner: (u64, u64),
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
            .filter(|p| belongs(p, r))
            .map(|p| p.session.clone())
    })
}
pub(super) fn track(generation: u64, id: u64, revision: u64) {
    PENDING.with(|s| {
        if let Some(p) = s
            .borrow_mut()
            .as_mut()
            .filter(|p| p.generation == generation && p.owner.0 == id)
        {
            p.revision = revision;
        }
    });
}
pub(super) fn current(generation: u64, id: u64, revision: u64) -> bool {
    PENDING.with(|s| {
        s.borrow()
            .as_ref()
            .is_none_or(|p| p.generation == generation && p.owner.0 == id && p.revision == revision)
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
            .is_some_and(|p| belongs(p, r) && p.revision == revision)
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
pub(super) fn failed(r: &mut Registry, revision: u64) -> bool {
    let failed = PENDING.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref()
            .is_some_and(|p| belongs(p, r) && p.revision == revision)
        {
            s.take();
            true
        } else {
            false
        }
    });
    if failed {
        if let Some(m) = r.toolbar_state.main.get() {
            m.set_output_busy(false);
        }
        mosaic::notify(r, "Operation failed. Try again.");
        r.queue_render();
    }
    failed
}

fn belongs(p: &Pending, r: &Registry) -> bool {
    p.generation == r.generation
        && r.screenshot.as_ref().is_some_and(|e| e.id == p.owner.0)
        && r.canvas_state
            .canvases
            .first()
            .is_some_and(|c| c.revision() == p.owner.1)
}
pub(super) fn submit(r: &mut Registry, session: Session) {
    let Some(owner) = r
        .screenshot
        .as_ref()
        .zip(r.canvas_state.canvases.first())
        .map(|(e, c)| (e.id, c.revision()))
    else {
        return;
    };
    PENDING.with(|s| {
        *s.borrow_mut() = Some(Pending {
            generation: r.generation,
            owner,
            revision: 0,
            session,
        })
    });
    if let Some(m) = r.toolbar_state.main.get() {
        m.set_output_busy(true);
    }
    r.queue_render();
}

#[cfg(test)]
mod tests {
    struct Backend;
    impl slint::platform::Platform for Backend {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(
                slint::platform::software_renderer::MinimalSoftwareWindow::new(
                    slint::platform::software_renderer::RepaintBufferType::NewBuffer,
                ),
            )
        }
    }

    use super::*;
    #[test]
    fn late_failure_and_frame_after_canvas_rebuild_cannot_retire_new_candidate() {
        slint::platform::set_platform(Box::new(Backend)).unwrap();
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
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let bounds = Rect {
                left: 0,
                top: 0,
                right: 20,
                bottom: 40,
            };
            r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
            r.document.session.image_source = Some(bounds);
            r.canvas_state
                .canvases
                .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
            r.screenshot = Some(screenshot::Editing::image(
                10,
                bounds,
                std::sync::Arc::new(Vec::new()),
            ));
            let mut candidate = r.document.session.clone();
            let turn = candidate.image_orientation.action(Action::Right);
            candidate.orient_image(turn);
            submit(r, candidate.clone());
            track(r.generation, 10, 2);
            ready(r, 1);
            assert_eq!(r.document.session.image_orientation, Default::default());
            assert!(!failed(r, 1));
            assert!(busy());
            r.canvas_state.canvases.clear();
            r.canvas_state
                .canvases
                .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
            r.screenshot.as_mut().unwrap().id = 11;
            submit(r, candidate);
            track(r.generation, 11, 3);
            assert!(!failed(r, 2));
            ready(r, 2);
            assert!(busy());
            assert_eq!(r.document.session.image_orientation, Default::default());
            ready(r, 3);
            assert!(!busy());
            assert_eq!(r.document.session.image_orientation, turn);
            assert!(r.document.session.can_undo());
            ready(r, 3);
            assert_eq!(r.document.session.image_orientation, turn);
            let old = r.document.session.clone();
            let mut next = old.clone();
            next.orient_image(turn.action(Action::Vertical));
            submit(r, next);
            track(r.generation, 11, 4);
            assert!(failed(r, 4));
            assert!(!busy());
            assert_eq!(r.document.session.image_orientation, old.image_orientation);
            assert_eq!(r.document.session.can_undo(), old.can_undo());
        });
        clear();
        REGISTRY.with(|s| s.borrow_mut().take());
    }
    #[test]
    fn replacement_editor_rejects_old_preview_and_old_tracking_without_committing() {
        let owner = (41, 9);
        PENDING.with(|s| {
            *s.borrow_mut() = Some(Pending {
                generation: 7,
                owner,
                revision: 12,
                session: Session::new(),
            })
        });
        assert!(current(7, 41, 12));
        assert!(!current(7, 40, 12));
        assert!(!current(6, 41, 12));
        assert!(!current(7, 41, 11));
        track(7, 40, 99);
        assert!(current(7, 41, 12));
        PENDING.with(|s| {
            *s.borrow_mut() = Some(Pending {
                generation: 7,
                owner: (42, 10),
                revision: 1,
                session: Session::new(),
            })
        });
        track(7, 41, 100);
        assert!(current(7, 42, 1));
        assert!(!current(7, 41, 12));
        PENDING.with(|s| s.borrow_mut().take());
    }
}
