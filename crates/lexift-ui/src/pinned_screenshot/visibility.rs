//! Apply one Core-owned visibility batch without changing pin ownership or focus.
use super::*;
use lexift_core::domain::pinned_image::visibility::{self as domain, Event, Request};

#[derive(Default)]
struct Projection {
    serial: u64,
    settled: bool,
    available: [bool; 2],
    notice: u64,
    batch_active: bool,
}
thread_local! { static VIEW: RefCell<Projection> = RefCell::new(Projection::default()); }
pub(super) fn reset() {
    VIEW.with(|v| *v.borrow_mut() = Projection::default());
}

pub(crate) fn available(hidden: bool) -> bool {
    VIEW.with(|v| v.borrow().available[usize::from(hidden)]) && runtime_idle()
}
fn runtime_idle() -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            !r.frozen
                && r.windows.values().all(|p| {
                    p.phase == Phase::Visible
                        && !p.native_drag
                        && !p.gesture.pressed.get()
                        && p.window.get().is_some_and(|w| {
                            !w.get_editing()
                                && !w.get_output_busy()
                                && !w.get_recognizing()
                                && !w.get_cropping()
                                && !w.get_policy_busy()
                        })
                })
        })
    })
}
pub(super) fn operable(id: u64) -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            !r.frozen
                && r.windows.get(&id).is_some_and(|p| {
                    p.phase == Phase::Visible
                        && !p.user_hidden
                        && p.visibility_pending.is_none()
                        && p.window
                            .get()
                            .is_some_and(|w| !w.get_window_suppressed() && !w.get_transform_busy())
                })
        })
    })
}
pub(crate) fn sync(state: &lexift_core::AppState) {
    let accepted = VIEW.with(|v| {
        let mut v = v.borrow_mut();
        let serial = state.pin_visibility.serial;
        if serial < v.serial
            || (serial == v.serial && v.settled && state.pin_visibility.pending.is_some())
        {
            return false;
        }
        v.serial = serial;
        v.settled = state.pin_visibility.pending.is_none();
        v.batch_active = state.pin_visibility.pending.is_some();
        v.available = [
            domain::available(state, false),
            domain::available(state, true),
        ];
        true
    });
    if !accepted {
        return;
    }
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            for (&id, p) in &mut r.windows {
                let pending = state
                    .pin_visibility
                    .pending
                    .as_ref()
                    .filter(|b| b.pending.contains(&id))
                    .map(|b| Request {
                        batch: b.id,
                        id,
                        hidden: b.hidden,
                    });
                if let Some(pin) = state.pinned_images.get(&id) {
                    // An already installed request may still await its Core acknowledgement.
                    if pending.is_none() {
                        p.user_hidden = pin.user_hidden;
                    }
                    p.visibility_pending = pending.filter(|q| q.batch > p.visibility_epoch);
                } else {
                    p.visibility_pending = None;
                }
            }
        }
    });
    update_input();
    let notice = state.pin_visibility.notice.filter(|n| {
        VIEW.with(|v| {
            let mut v = v.borrow_mut();
            if n.batch <= v.notice {
                false
            } else {
                v.notice = n.batch;
                true
            }
        })
    });
    if let Some(notice) = notice {
        crate::bridge::image_history::show_passive_feedback(
            notice.anchor,
            "Some pins could not be hidden or restored. Try again.",
            true,
        );
    }
    crate::tray_menu::refresh_pin_recovery();
    super::transform::project(state);
}
fn update_input() {
    let busy = VIEW.with(|v| v.borrow().batch_active);
    let ids = STATE.with(|s| {
        let s = s.borrow();
        let Some(r) = s.as_ref() else {
            return vec![];
        };
        r.windows
            .iter()
            .filter_map(|(&id, p)| {
                let w = p.window.get()?;
                let suppressed = p.user_hidden || r.frozen || busy;
                if w.get_window_suppressed() == suppressed {
                    return None;
                }
                w.set_window_suppressed(suppressed);
                Some(id)
            })
            .collect::<Vec<_>>()
    });
    for id in ids {
        apply_policy(id);
    }
}
fn complete(request: Request, success: bool) {
    emit_pin(lexift_core::domain::pinned_image::Event::WindowVisibility(
        Event::Applied {
            request,
            result: if success {
                Ok(())
            } else {
                Err(lexift_core::Error::new("Window visibility failed"))
            },
        },
    ));
}
pub(crate) fn apply(request: Request, attempt: u8) {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&request.id).filter(|p| {
            p.phase == Phase::Visible
                && p.visibility_pending == Some(request)
                && p.visibility_epoch < request.batch
        })?;
        Some((
            p.window.get()?.clone_strong(),
            p.window.revision(),
            r.prepare,
            r.lifecycle.clone(),
            p.gesture.clone(),
            p.user_hidden,
            p.window.input(),
        ))
    });
    let Some((w, instance, prepare, lifecycle, gesture, before, input)) = context else {
        let missing = STATE.with(|s| {
            s.borrow().as_ref().is_none_or(|r| {
                r.windows
                    .get(&request.id)
                    .is_none_or(|p| p.phase != Phase::Visible || p.window.get().is_none())
            })
        });
        if missing {
            complete(request, false);
        }
        return;
    };
    if !request.hidden {
        match prepare(w.window()) {
            PassiveWindowPreparation::Pending if attempt < 20 => {
                STATE.with(|s| {
                    if let Some(p) = s.borrow().as_ref().and_then(|r| r.windows.get(&request.id)) {
                        p.window.prepare_after(Duration::from_millis(16), move || {
                            apply(request, attempt + 1)
                        });
                    }
                });
                return;
            }
            PassiveWindowPreparation::Ready => {}
            _ => {
                finish(request, instance, before, false);
                return;
            }
        }
    }
    gesture.cancel();
    input.cancel_pending();
    w.invoke_close_menu();
    clear_feedback(request.id, None);
    (lifecycle.cancel_window_interaction)(w.window());
    let position = w.window().position();
    let previous_size = w.window().size();
    let area = (lifecycle.popup_work_area)(lexift_core::domain::geometry::Point {
        x: position.x + 1,
        y: position.y + 1,
    });
    let success = if request.hidden {
        w.hide().is_ok() && !w.window().is_visible()
    } else {
        show(request.id)
    };
    if !success {
        if before {
            let _ = w.hide();
        } else {
            let _ = w.show();
        }
        w.window().set_size(previous_size);
        w.window().set_position(position);
    }
    if success && request.hidden {
        STATE.with(|s| {
            if let Some(p) = s
                .borrow_mut()
                .as_mut()
                .and_then(|r| r.windows.get_mut(&request.id))
            {
                p.hidden_work_area = area;
            }
        });
    }
    finish(
        request,
        instance,
        if success { request.hidden } else { before },
        success,
    );
}
fn finish(request: Request, instance: u64, hidden: bool, success: bool) {
    let current = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(p) = s
            .as_mut()
            .and_then(|r| r.windows.get_mut(&request.id))
            .filter(|p| {
                p.window.revision() == instance
                    && p.visibility_pending == Some(request)
                    && p.visibility_epoch < request.batch
            })
        else {
            return false;
        };
        p.user_hidden = hidden;
        p.visibility_epoch = request.batch;
        if success && !hidden {
            p.hidden_work_area = None;
        }
        true
    });
    if current {
        if success && !hidden {
            crate::bridge::cancel_idle_memory_trim();
        }
        complete(request, success);
        if success && hidden {
            crate::bridge::schedule_idle_memory_trim();
        }
    }
}
/// Restore only when the effective visibility permits it. Native configuration
/// and the layered frame precede show; this never requests activation.
fn show(id: u64) -> bool {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id).filter(|p| p.phase == Phase::Visible)?;
        Some((
            p.window.get()?.clone_strong(),
            r.lifecycle.clone(),
            p.hidden_work_area,
            p.pointer_sink.clone()?,
        ))
    });
    let Some((w, l, saved_area, sink)) = context else {
        return false;
    };
    let position = w.window().position();
    let size = w.window().size();
    let needs_clamp = (l.popup_work_area)(lexift_core::domain::geometry::Point {
        x: position.x + 1,
        y: position.y + 1,
    })
    .is_some_and(|area| {
        saved_area.is_some_and(|old| old != area)
            && (position.x < area.left
                || position.y < area.top
                || i64::from(position.x) + i64::from(size.width) > i64::from(area.right)
                || i64::from(position.y) + i64::from(size.height) > i64::from(area.bottom))
    });
    // Preparation temporarily restores the passive role. Reinstall the owned
    // receiver before transactional geometry checks call the pin policy bridge.
    (l.prepare_pinned_image)(w.window())
        && (l.complete_passive_window_show)(w.window(), sink)
        && apply_policy(id)
        && (!needs_clamp || super::zoom::settled(id))
        && (!w.get_mouse_passthrough() || policy::frame(&w, &l, true))
        && w.show().is_ok()
        && w.window().is_visible()
}
pub(super) fn frozen(frozen: bool) {
    let ids = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else {
            return vec![];
        };
        r.frozen = frozen;
        r.windows.keys().copied().collect::<Vec<_>>()
    });
    update_input();
    for id in ids {
        let context = STATE.with(|s| {
            let s = s.borrow();
            let r = s.as_ref()?;
            let p = r.windows.get(&id).filter(|p| p.phase == Phase::Visible)?;
            Some((
                p.window.get()?.clone_strong(),
                p.gesture.clone(),
                r.lifecycle.clone(),
                p.user_hidden,
                p.window.input(),
            ))
        });
        if let Some((w, g, l, hidden, input)) = context {
            if frozen || hidden {
                g.cancel();
                input.cancel_pending();
                clear_feedback(id, None);
                w.invoke_close_menu();
                (l.cancel_window_interaction)(w.window());
                let _ = w.hide();
            } else if show(id) {
                policy::schedule_frame(id);
            }
        }
    }
    crate::tray_menu::refresh_pin_recovery();
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        AppCommand, AppEvent, AppState,
        domain::{
            geometry::Rect,
            pinned_image::{
                self as pin,
                interaction::{Opacity, Policy},
            },
            screenshot::{ScreenshotPhase, ScreenshotState},
        },
    };
    use slint::platform::{
        Platform, Renderer, WindowAdapter, WindowEvent,
        software_renderer::{RepaintBufferType, SoftwareRenderer},
    };
    struct Backend;
    thread_local! { static ACTIVATIONS: Cell<u32> = const {Cell::new(0)}; }
    thread_local! { static PASSIVE: RefCell<std::collections::BTreeSet<usize>> = RefCell::new(Default::default()); }
    fn prepare_window(w: &slint::Window) -> PassiveWindowPreparation {
        PASSIVE.with(|p| p.borrow_mut().insert(w as *const _ as usize));
        PassiveWindowPreparation::Ready
    }
    fn activate(_: &slint::Window) -> bool {
        ACTIVATIONS.with(|n| n.set(n.get() + 1));
        true
    }
    impl Platform for Backend {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(Rc::new_cyclic(|weak: &std::rc::Weak<TestWindow>| {
                TestWindow {
                    window: slint::Window::new(weak.clone()),
                    renderer: SoftwareRenderer::new_with_repaint_buffer_type(
                        RepaintBufferType::NewBuffer,
                    ),
                    position: Default::default(),
                    size: Default::default(),
                }
            }))
        }
    }
    // MinimalSoftwareWindow intentionally ignores positions. This adapter keeps
    // real geometry so work-area rollback is verified rather than assumed.
    struct TestWindow {
        window: slint::Window,
        renderer: SoftwareRenderer,
        position: Cell<slint::PhysicalPosition>,
        size: Cell<slint::PhysicalSize>,
    }
    impl WindowAdapter for TestWindow {
        fn window(&self) -> &slint::Window {
            &self.window
        }
        fn renderer(&self) -> &dyn Renderer {
            &self.renderer
        }
        fn position(&self) -> Option<slint::PhysicalPosition> {
            Some(self.position.get())
        }
        fn set_position(&self, p: slint::WindowPosition) {
            self.position.set(p.to_physical(self.window.scale_factor()));
        }
        fn size(&self) -> slint::PhysicalSize {
            self.size.get()
        }
        fn set_size(&self, size: slint::WindowSize) {
            let scale = self.window.scale_factor();
            self.size.set(size.to_physical(scale));
            self.window.dispatch_event(WindowEvent::Resized {
                size: size.to_logical(scale),
            });
        }
    }
    fn create(session: u64) -> (u64, PinnedScreenshotWindow) {
        crate::annotation::screenshot::sync(&ScreenshotState {
            session,
            operation: 1,
            phase: ScreenshotPhase::Output,
            ..Default::default()
        });
        super::super::show(
            session,
            1,
            Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 50,
                    top: 70,
                    right: 250,
                    bottom: 170,
                },
                pixels: [30, 60, 90, 255].repeat(200 * 100),
            }),
        );
        STATE.with(|s| {
            let s = s.borrow();
            let (&id, p) = s.as_ref().unwrap().windows.last_key_value().unwrap();
            (id, p.window.get().unwrap().clone_strong())
        })
    }
    fn pump(core: &RefCell<AppState>) {
        for _ in 0..10 {
            slint::platform::update_timers_and_animations();
            if core.borrow().pin_visibility.pending.is_none() {
                return;
            }
        }
        panic!("visibility batch did not settle");
    }
    fn intent(hidden: bool) {
        emit_pin(pin::Event::WindowVisibility(Event::Requested {
            hidden,
            anchor: Default::default(),
        }));
    }
    #[test]
    fn direction_pixels_and_document_commit_together_and_reject_stale_or_invalid_frames() {
        use lexift_core::domain::{
            annotation::Session,
            annotation_export::{ExportRequest, ExportSnapshot},
            image_transform::Action,
        };
        slint::platform::set_platform(Box::new(Backend)).unwrap();
        let native_ok = Rc::new(Cell::new(true));
        let native = native_ok.clone();
        let mut l = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        )
        .with_pinned_image_preparation(|_| true);
        l.popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            })
        });
        l.complete_passive_window_show = Rc::new(|_, _| true);
        l.configure_pinned_image = Rc::new(move |_, _, _, _| native.get());
        l.activate_user_requested_window = activate;
        init(|_| PassiveWindowPreparation::Ready, l);
        let (id, w) = create(35);
        let full = Rect {
            left: 50,
            top: 70,
            right: 250,
            bottom: 170,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        let request = ExportRequest {
            session: 0,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(session),
                monitor: full,
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        let mut core = AppState::default();
        core.pinned_images.insert(
            id,
            pin::State {
                source: Some(full),
                document: Some(request.clone()),
                ..Default::default()
            },
        );
        let events = Rc::new(RefCell::new(Vec::new()));
        let sent = events.clone();
        crate::annotation::set_handler(Rc::new(move |e| sent.borrow_mut().push(e)));
        core.reduce(AppEvent::PinnedImage(pin::Event::TransformRequested {
            id,
            action: Action::Right,
        }));
        super::super::sync_outputs(&core.pinned_images);
        let old = core.pinned_images[&id].revision;
        assert!(w.get_transform_busy());
        assert!(!operable(id));
        let before = (w.window().position(), w.window().size());
        super::super::present(
            id,
            old,
            Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                pixels: vec![0, 0, 0, 255],
            }),
        );
        for e in events.borrow_mut().drain(..) {
            core.reduce(e);
        }
        super::super::sync_outputs(&core.pinned_images);
        assert_eq!(core.pinned_images[&id].document.as_ref(), Some(&request));
        assert_eq!((w.window().position(), w.window().size()), before);
        assert!(!w.get_transform_busy());
        let late = lexift_core::domain::image_transform::Orientation::identity()
            .action(Action::Right)
            .rect(full, full)
            .unwrap();
        super::super::present(
            id,
            old,
            Arc::new(CapturedRegion {
                bounds: late,
                pixels: [30, 60, 90, 255].repeat(200 * 100),
            }),
        );
        assert_eq!(
            (w.window().position(), w.window().size()),
            before,
            "failed candidate must reject a late success of the same version"
        );
        let stale = core.pinned_images.clone();
        core.reduce(AppEvent::PinnedImage(pin::Event::TransformRequested {
            id,
            action: Action::Left,
        }));
        let revision = core.pinned_images[&id].revision;
        super::super::sync_outputs(&core.pinned_images);
        let proposed = core.pinned_images[&id].proposed.clone().unwrap();
        super::super::sync_outputs(&stale);
        assert!(
            w.get_transform_busy(),
            "stale projection must not clear a newer candidate"
        );
        let bounds = proposed
            .snapshot
            .session
            .image_orientation
            .rect(full, full)
            .unwrap();
        let pixels = Arc::new(CapturedRegion {
            bounds,
            pixels: [30, 60, 90, 255].repeat(200 * 100),
        });
        super::super::present(id, old, pixels.clone());
        assert_eq!((w.window().position(), w.window().size()), before);
        core.reduce(AppEvent::PinnedImage(pin::Event::Rendered { id, revision }));
        super::super::sync_outputs(&core.pinned_images);
        super::super::present(id, revision, pixels);
        slint::platform::update_timers_and_animations();
        for e in events.borrow_mut().drain(..) {
            core.reduce(e);
        }
        super::super::sync_outputs(&core.pinned_images);
        assert_eq!(core.pinned_images[&id].document.as_ref(), Some(&proposed));
        assert!(!w.get_transform_busy());
        assert!(!w.get_editing());
        assert!(operable(id));
        assert_eq!(super::super::geometry(id).unwrap().0, bounds);
        assert_eq!(ACTIVATIONS.with(Cell::get), 0);
        native_ok.set(false);
        core.reduce(AppEvent::PinnedImage(pin::Event::TransformRequested {
            id,
            action: Action::Right,
        }));
        let revision = core.pinned_images[&id].revision;
        super::super::sync_outputs(&core.pinned_images);
        super::super::present(
            id,
            revision,
            Arc::new(CapturedRegion {
                bounds: full,
                pixels: [30, 60, 90, 255].repeat(200 * 100),
            }),
        );
        for e in events.borrow_mut().drain(..) {
            core.reduce(e);
        }
        super::super::sync_outputs(&core.pinned_images);
        assert_eq!(core.pinned_images[&id].document.as_ref(), Some(&proposed));
        assert!(!w.get_transform_busy());
        native_ok.set(true);
        let before_ack = (w.window().position(), w.window().size());
        core.reduce(AppEvent::PinnedImage(pin::Event::TransformRequested {
            id,
            action: Action::Right,
        }));
        let revision = core.pinned_images[&id].revision;
        super::super::sync_outputs(&core.pinned_images);
        core.reduce(AppEvent::PinnedImage(pin::Event::Rendered { id, revision }));
        super::super::sync_outputs(&core.pinned_images);
        super::super::present(
            id,
            revision,
            Arc::new(CapturedRegion {
                bounds: full,
                pixels: [30, 60, 90, 255].repeat(200 * 100),
            }),
        );
        core.reduce(AppEvent::PinnedImage(pin::Event::PreviewFailed {
            id,
            revision,
            error: "cancel before UI acknowledgement".into(),
        }));
        super::super::sync_outputs(&core.pinned_images);
        assert_eq!((w.window().position(), w.window().size()), before_ack);
        assert_eq!(super::super::geometry(id).unwrap().0, bounds);
        slint::platform::update_timers_and_animations();
        for e in events.borrow_mut().drain(..) {
            core.reduce(e);
        }
        assert_eq!(core.pinned_images[&id].document.as_ref(), Some(&proposed));
        close_all();
    }
    #[test]
    fn visibility_preserves_state_handles_failure_and_never_reveals_user_hidden_pins() {
        slint::platform::set_platform(Box::new(Backend)).unwrap();
        let fail = Rc::new(Cell::new(false));
        let failure = fail.clone();
        let frames = Rc::new(Cell::new(0));
        let presented = frames.clone();
        let area = Rc::new(Cell::new(Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }));
        let work = area.clone();
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        )
        .with_pinned_image_preparation(move |_| !failure.replace(false))
        .with_pinned_passthrough_frame(move |_, frame| {
            if frame.is_some() {
                presented.set(presented.get() + 1);
            }
            true
        });
        lifecycle.activate_user_requested_window = activate;
        lifecycle.popup_work_area = Rc::new(move |_| Some(work.get()));
        let receivers = Rc::new(RefCell::new(std::collections::BTreeMap::<
            usize,
            crate::PopupPointerSink,
        >::new()));
        let original = receivers.clone();
        lifecycle.complete_passive_window_show = Rc::new(move |w, sink| {
            let key = w as *const _ as usize;
            PASSIVE.with(|p| p.borrow_mut().remove(&key));
            let mut receivers = original.borrow_mut();
            if let Some(before) = receivers.get(&key) {
                assert!(
                    Rc::ptr_eq(before, &sink),
                    "restore reuses the owned native receiver"
                );
            } else {
                receivers.insert(key, sink);
            }
            true
        });
        lifecycle.configure_pinned_image =
            Rc::new(|w, _, _, _| !PASSIVE.with(|p| p.borrow().contains(&(w as *const _ as usize))));
        init(prepare_window, lifecycle);
        let (a, first) = create(31);
        let (b, second) = create(32);
        let core = Rc::new(RefCell::new(AppState::default()));
        for id in [a, b] {
            core.borrow_mut()
                .pinned_images
                .insert(id, pin::State::default());
        }
        core.borrow_mut()
            .pinned_images
            .get_mut(&a)
            .unwrap()
            .interaction
            .applied = Policy {
            locked: true,
            passthrough: true,
            opacity: Opacity::Twenty,
        };
        core.borrow_mut().pinned_images.get_mut(&b).unwrap().hidden = true;
        let state = core.clone();
        let queued = Rc::new(RefCell::new(Vec::new()));
        let commands = queued.clone();
        crate::annotation::set_handler(Rc::new(move |event| {
            let output = if let AppEvent::PinnedImage(pin::Event::Opened { id, .. }) = event {
                state
                    .borrow_mut()
                    .pinned_images
                    .insert(id, pin::State::default());
                vec![]
            } else {
                state.borrow_mut().reduce(event)
            };
            let snapshot = state.borrow().clone();
            sync_outputs(&snapshot.pinned_images);
            sync(&snapshot);
            for command in output {
                match command {
                    AppCommand::PinnedImage(pin::Command::ApplyVisibility(q)) => {
                        commands.borrow_mut().push(q);
                        slint::Timer::single_shot(Duration::ZERO, move || apply(q, 0));
                    }
                    AppCommand::PinnedImage(pin::Command::ApplyInteraction {
                        id,
                        request,
                        policy,
                    }) => {
                        slint::Timer::single_shot(Duration::ZERO, move || {
                            policy::apply(id, request, policy)
                        });
                    }
                    _ => {}
                }
            }
        }));
        sync_outputs(&core.borrow().pinned_images);
        sync(&core.borrow());
        assert!(available(true) && !available(false));
        second.invoke_press_changed(true);
        assert!(!available(true));
        second.invoke_press_changed(false);
        assert!(available(true));
        STATE.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .windows
                .get_mut(&b)
                .unwrap()
                .native_drag = true
        });
        assert!(!available(true));
        finish_native_drag(b);
        assert!(available(true));
        first
            .window()
            .set_position(slint::PhysicalPosition::new(-8, 5));
        let position = first.window().position();
        let size = first.window().size();
        intent(true);
        pump(&core);
        assert!(!first.window().is_visible() && !second.window().is_visible());
        assert!(core.borrow().pinned_images.values().all(|p| p.user_hidden));
        assert!(second.get_annotations_hidden());
        first.invoke_escape_requested();
        first.invoke_output_requested(0);
        assert_eq!(
            STATE.with(|s| s.borrow().as_ref().unwrap().windows.len()),
            2
        );
        let stale = queued.borrow()[0];
        let hidden = core.borrow().clone();
        set_frozen(true);
        set_frozen(false);
        assert!(!first.window().is_visible() && !second.window().is_visible());
        let (c, third) = create(33);
        assert!(third.window().is_visible() && !core.borrow().pinned_images[&c].user_hidden);
        fail.set(true);
        intent(false);
        pump(&core);
        assert_eq!(
            core.borrow()
                .pinned_images
                .values()
                .filter(|p| p.user_hidden)
                .count(),
            1
        );
        assert_eq!(core.borrow().pin_visibility.notice.unwrap().failed, 1);
        assert!(available(false));
        intent(false);
        pump(&core);
        assert!(
            first.window().is_visible()
                && second.window().is_visible()
                && third.window().is_visible()
        );
        assert_eq!(first.window().position(), position);
        assert_eq!(first.window().size(), size);
        assert!(first.get_mouse_passthrough() && first.get_locked());
        assert_eq!(first.get_content_opacity_percent(), 20);
        assert!(frames.get() > 0);
        assert_eq!(ACTIVATIONS.with(Cell::get), 0);
        sync(&hidden);
        apply(stale, 0);
        assert!(
            first.window().is_visible(),
            "old state and callback cannot rehide a restored pin"
        );
        intent(true);
        pump(&core);
        // Mouse recovery may update policy while hidden, but cannot reveal it.
        emit_pin(pin::Event::RestoreMouseInput);
        pump(&core);
        assert!(!first.window().is_visible() && !first.get_mouse_passthrough());
        assert_eq!(first.get_content_opacity_percent(), 20);
        area.set(Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 600,
        });
        intent(false);
        pump(&core);
        let pos = first.window().position();
        let size = first.window().size();
        let inset = (first.get_frame_inset() * first.window().scale_factor()).round() as i32;
        let grip = (64. * first.window().scale_factor()).round() as i32;
        let width = size.width as i32 - 2 * inset;
        let height = size.height as i32 - 2 * inset;
        assert!((pos.x + inset + width).min(900) - (pos.x + inset).max(100) >= width.min(grip));
        assert!((pos.y + inset + height).min(600) - (pos.y + inset).max(100) >= height.min(grip));
        assert!(first.get_locked() && first.get_content_opacity_percent() == 20);
        intent(true);
        let closing = queued.borrow().last().copied().unwrap();
        close(closing.id);
        pump(&core);
        apply(closing, 0);
        assert!(!core.borrow().pinned_images.contains_key(&closing.id));
        intent(false);
        let exiting = queued.borrow().last().copied().unwrap();
        core.borrow_mut().reduce(AppEvent::ExitRequested);
        sync(&core.borrow());
        apply(exiting, 0);
        slint::platform::update_timers_and_animations();
        assert!(!first.window().is_visible() && !second.window().is_visible());
        assert_eq!(ACTIVATIONS.with(Cell::get), 0);
        close_all();
    }
}
