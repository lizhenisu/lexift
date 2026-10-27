//! Image document candidates are installed with matching pixels and feedback.
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
    image_resize::clear();
}
pub(super) fn busy() -> bool {
    PENDING.with(|s| s.borrow().is_some())
}
pub(super) fn available() -> bool {
    screenshot::long::image_editing()
        && !busy()
        && !image_resize::busy()
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
/// Finish input transactions before opening the canvas menu, not while a native
/// text control owns its own context menu. Stale canvases cannot open a menu.
pub(super) fn prepare_menu() -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return false };
        if !idle(r) {
            return false;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.close_panel();
        r.hints.clear();
        sync(r);
        true
    })
}
fn idle(r: &Registry) -> bool {
    available()
        && r.gesture().is_none()
        && r.pending_drawing.is_none()
        && r.toolbar_state.watermark.get().is_none()
        && r.toolbar_state.menus.current.is_none()
        && screenshot::long::editor().is_some_and(|w| !w.get_image_panning())
}
pub(super) fn color(action: lexift_core::domain::image_color::Action) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if !idle(r) {
            return;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.close_panel();
        r.hints.clear();
        let mut candidate = r.document.session.clone();
        if candidate.color_image(candidate.image_color.action(action)) {
            submit(r, candidate);
        }
    });
}
pub(super) fn sync(r: &Registry) {
    if let Some(w) = screenshot::long::editor() {
        let color = r.document.session.image_color;
        w.set_image_color_enabled(idle(r));
        w.set_image_output_busy(
            busy() || editor_ocr::busy() || screenshot::long_editor::outputting(),
        );
        w.set_grayscale(color.grayscale);
        w.set_inverted(color.inverted);
        w.set_brightness(i32::from(color.brightness.value()));
        w.set_contrast(i32::from(color.contrast.value()));
    }
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
        let geometry_changed = p.session.image_orientation != r.document.session.image_orientation
            || p.session.image_crop != r.document.session.image_crop
            || p.session.image_scale != r.document.session.image_scale;
        let scale_changed = p.session.image_scale != r.document.session.image_scale;
        r.document.session = p.session;
        image_crop::candidate_done();
        if geometry_changed {
            if !scale_changed {
                r.set_mode(InteractionMode::Mouse);
            }
            image_crop::sync(r, true);
        }
        if scale_changed {
            image_resize::completed();
            restore_parameter_projection(r);
        }
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
        image_crop::candidate_failed();
        image_resize::failed();
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
pub(super) fn cancel_scale(r: &mut Registry) {
    let cancelled = PENDING.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|p| {
            belongs(p, r) && p.session.image_scale != r.document.session.image_scale
        }) {
            s.take();
            true
        } else {
            false
        }
    });
    if cancelled {
        if let Some(e) = r.screenshot.as_mut() {
            e.revision = e.revision.saturating_add(1);
            e.accepted_after = e.revision;
        }
        if let Some(w) = r.toolbar_state.main.get() {
            w.set_output_busy(false);
        }
    }
}
pub(super) fn submit(r: &mut Registry, mut session: Session) {
    if session.image_crop == r.document.session.image_crop
        && session.image_orientation == r.document.session.image_orientation
        && session.image_color != r.document.session.image_color
    {
        // Selection is temporary interaction state. A color-only history step
        // must not jump back to the selection captured before that step.
        session.selected = r.document.session.selected.and_then(|index| {
            let id = r.document.session.objects.get(index)?.id;
            session.objects.iter().position(|object| object.id == id)
        });
    }
    if let Some(w) = screenshot::long::editor() {
        w.invoke_close_image_menu();
    }
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
    fn keyboard_menu_waits_for_releases_and_cancellation_retires_pending_open() {
        use slint::platform::{Key, WindowEvent};
        slint::platform::set_platform(Box::new(Backend)).unwrap();
        let w = AnnotationCanvas::new().unwrap();
        w.set_image_view(true);
        w.invoke_focus_input();
        let opens = Rc::new(std::cell::Cell::new(0));
        let count = opens.clone();
        w.on_prepare_image_menu(move || {
            count.set(count.get() + 1);
            false
        });
        let key = |key: Key, down| {
            let text = char::from(key).to_string().into();
            w.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        key(Key::Shift, true);
        key(Key::F10, true);
        key(Key::F10, true); // Auto-repeat still yields one opening.
        assert_eq!(opens.get(), 0);
        key(Key::F10, false);
        assert_eq!(opens.get(), 0);
        key(Key::Shift, false);
        assert_eq!(opens.get(), 1);
        key(Key::Shift, true);
        key(Key::F10, true);
        key(Key::Shift, false); // The opposite release order also works.
        assert_eq!(opens.get(), 1);
        key(Key::F10, false);
        assert_eq!(opens.get(), 2);
        key(Key::Menu, true);
        w.invoke_close_image_menu();
        key(Key::Menu, false);
        assert_eq!(opens.get(), 2);
        key(Key::Menu, true);
        key(Key::Menu, false);
        assert_eq!(opens.get(), 3);
    }
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

            // Color candidates must not inherit orientation's switch-to-mouse
            // behavior, including undo and redo prepared through the same path.
            r.document.mode = InteractionMode::Tool(2);
            r.document.session.add(Object {
                id: Default::default(),
                image_basis: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((1., 2.), (8., 12.)),
                points: vec![],
                erased: vec![],
                style: Style::default(),
                extra: Extra::None,
            });
            r.document.session.selected = None;
            let mut colored = r.document.session.clone();
            colored.color_image(
                colored
                    .image_color
                    .action(lexift_core::domain::image_color::Action::Grayscale),
            );
            submit(r, colored.clone());
            track(r.generation, 11, 5);
            assert!(!r.document.session.image_color.grayscale);
            ready(r, 4);
            assert!(busy());
            ready(r, 5);
            assert_eq!(r.document.mode, InteractionMode::Tool(2));
            assert!(r.document.session.image_color.grayscale);
            r.document.session.selected = Some(0);
            let mut undo = r.document.session.clone();
            assert!(undo.undo());
            submit(r, undo);
            track(r.generation, 11, 6);
            ready(r, 6);
            assert!(!r.document.session.image_color.grayscale);
            assert_eq!(r.document.mode, InteractionMode::Tool(2));
            assert_eq!(r.document.session.selected, Some(0));
            let mut redo = r.document.session.clone();
            assert!(redo.redo());
            submit(r, redo);
            track(r.generation, 11, 7);
            ready(r, 7);
            assert_eq!(r.document.session.image_color, colored.image_color);
            assert_eq!(r.document.mode, InteractionMode::Tool(2));
            assert_eq!(r.document.session.selected, Some(0));
            let before = r.document.session.clone();
            let mut resized = before.clone();
            let scale = lexift_core::domain::image_resize::ImageScale::new(3, 2).unwrap();
            assert!(resized.resize_image(scale).unwrap());
            submit(r, resized.clone());
            track(r.generation, 11, 8);
            assert!(failed(r, 8));
            assert_eq!(r.document.session.image_scale, before.image_scale);
            submit(r, resized.clone());
            track(r.generation, 11, 9);
            cancel_scale(r);
            assert!(!busy());
            ready(r, 9);
            assert_eq!(r.document.session.image_scale, before.image_scale);
            submit(r, resized);
            track(r.generation, 11, 10);
            ready(r, 9);
            assert!(busy());
            ready(r, 10);
            assert_eq!(r.document.session.image_scale, scale);
            assert_eq!(r.document.mode, InteractionMode::Tool(2));
            assert_eq!(r.document.session.selected, Some(0));
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
