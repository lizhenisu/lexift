//! Session-local crop keyboard input; geometry and rollback remain in Core.
use super::*;
use lexift_core::domain::region_selection::RegionNudge;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::annotation) enum Owner {
    Canvas(usize, u64),
    Toolbar(u64),
}
#[derive(Default)]
pub(in crate::annotation) struct State {
    pressed: u8,
    suppressed: u8,
    owner: Option<(Owner, u64)>,
}
fn current(r: &Registry, generation: u64, owner: Owner) -> bool {
    r.generation == generation
        && match owner {
            Owner::Canvas(i, revision) => r
                .canvas_state
                .canvases
                .get(i)
                .is_some_and(|c| c.revision() == revision),
            Owner::Toolbar(revision) => {
                r.toolbar_state.main.get().is_some() && r.toolbar_state.main.revision() == revision
            }
        }
}
fn allowed(r: &Registry) -> bool {
    r.screenshot.as_ref().is_some_and(|e| {
        e.pin.is_none()
            && !e.region.dragging()
            && FLOW.with(|s| {
                let s = s.borrow();
                s.state.session == e.id && s.state.phase == ScreenshotPhase::Editing
            })
    }) && !super::size_input::busy(r)
        && r.document.mode.is_mouse()
        && r.editor.window().is_none()
        && r.toolbar_state.watermark.get().is_none()
        && r.toolbar_state.panel.get().is_none()
        && r.toolbar_state.menus.current.is_none()
        && r.toolbar_state.toolbar_drag.is_none()
}
fn selection(r: &Registry) -> Option<(u64, Rect)> {
    let e = r.screenshot.as_ref().filter(|e| e.pin.is_none())?;
    Some((e.id, e.region.bounds()?))
}
fn publish(value: Option<(u64, Rect)>) {
    if let Some((session, bounds)) = value {
        emit(ScreenshotEvent::Selected { session, bounds });
    }
}
fn changed_later(r: &mut Registry, before: Option<Rect>) {
    let value = selection(r);
    if value.map(|v| v.1) == before {
        return;
    }
    r.queue_render();
    let generation = r.generation;
    slint::Timer::single_shot(Duration::ZERO, move || {
        let valid = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|r| r.generation == generation && selection(r) == value)
        });
        if valid {
            publish(value);
        }
    });
}
pub(in crate::annotation) fn invalidate(r: &mut Registry) {
    let Some(e) = r.screenshot.as_mut().filter(|e| e.pin.is_none()) else {
        return;
    };
    let before = e.region.bounds();
    e.region.cancel_keyboard();
    e.keyboard = State::default();
    changed_later(r, before);
}
pub(in crate::annotation) fn finish(r: &mut Registry) {
    if let Some(e) = r.screenshot.as_mut().filter(|e| e.pin.is_none()) {
        e.region.finish_keyboard();
        e.keyboard = State::default();
    }
    position_region_toolbar(r, false);
}
pub(in crate::annotation) fn commit_current() {
    let value = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        finish(r);
        selection(r)
    });
    publish(value);
}
pub(in crate::annotation) fn escape(r: &mut Registry) -> bool {
    let Some(e) = r.screenshot.as_mut().filter(|e| e.pin.is_none()) else {
        return false;
    };
    if !e.region.keyboard_adjusting() {
        return e.keyboard.suppressed != 0;
    }
    let before = e.region.bounds();
    e.region.cancel_keyboard();
    e.keyboard.suppressed |= e.keyboard.pressed;
    changed_later(r, before);
    true
}
fn direction(text: &str) -> Option<usize> {
    use slint::platform::Key;
    [
        Key::LeftArrow,
        Key::RightArrow,
        Key::UpArrow,
        Key::DownArrow,
    ]
    .iter()
    .position(|k| text == char::from(*k).to_string())
}
/// Called only by an unconsumed root key event from its owning canvas/toolbar.
fn handle(
    generation: u64,
    source: (Owner, u64),
    text: &str,
    down: bool,
    control: bool,
    shift: bool,
    repeat: bool,
) -> bool {
    let (owner, epoch) = source;
    if text.eq_ignore_ascii_case("c") && !control && !shift {
        let eligible = REGISTRY.with(|s| {
            let slot = s.borrow();
            slot.as_ref().is_some_and(|r| {
                let foreground = r
                    .lifecycle
                    .annotation_foreground
                    .as_ref()
                    .is_none_or(|query| {
                        query().is_some_and(|token| {
                            r.canvas_state.canvases.iter().any(|c| {
                                (r.lifecycle.annotation_window_token)(c.window.window())
                                    == Some(token)
                            }) || r.toolbar_state.main.get().is_some_and(|w| {
                                (r.lifecycle.annotation_window_token)(w.window()) == Some(token)
                            })
                        })
                    });
                current(r, generation, owner) && foreground && super::picker_feedback(r).is_some()
            })
        });
        if eligible && down && !repeat {
            crate::screen_interaction::pixel_picker::copy();
        }
        return eligible;
    }
    let Some(direction) = direction(text) else {
        return false;
    };
    let bit = 1 << direction;
    let (accepted, value) = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| current(r, generation, owner)) else {
            return (false, None);
        };
        if !down {
            let Some(e) = r.screenshot.as_mut() else {
                return (false, None);
            };
            if e.keyboard.owner != Some((owner, epoch)) {
                return (false, None);
            }
            let held = e.keyboard.pressed & bit != 0 || e.keyboard.suppressed & bit != 0;
            e.keyboard.pressed &= !bit;
            e.keyboard.suppressed &= !bit;
            if e.keyboard.pressed == 0 {
                e.region.finish_keyboard();
                e.keyboard.owner = None;
            }
            position_region_toolbar(r, false);
            return (held, None);
        }
        if !allowed(r) {
            return (false, None);
        }
        if r.screenshot
            .as_ref()
            .unwrap()
            .keyboard
            .owner
            .is_some_and(|token| token != (owner, epoch))
        {
            invalidate(r);
        }
        let e = r.screenshot.as_mut().unwrap();
        if repeat && e.keyboard.pressed & bit == 0 {
            return (true, None);
        }
        e.keyboard.owner = Some((owner, epoch));
        e.keyboard.pressed |= bit;
        if e.keyboard.suppressed & bit != 0 {
            return (true, None);
        };
        if !e.region.begin_keyboard() {
            return (true, None);
        };
        let step = if shift { 10 } else { 1 };
        let (dx, dy) = match direction {
            0 => (-step, 0),
            1 => (step, 0),
            2 => (0, -step),
            _ => (0, step),
        };
        let mut desktop = e.frames[0].bounds;
        for f in e.frames.iter().skip(1) {
            desktop.left = desktop.left.min(f.bounds.left);
            desktop.top = desktop.top.min(f.bounds.top);
            desktop.right = desktop.right.max(f.bounds.right);
            desktop.bottom = desktop.bottom.max(f.bounds.bottom);
        }
        let action = if control {
            RegionNudge::Resize { dw: dx, dh: dy }
        } else {
            RegionNudge::Move { dx, dy }
        };
        if !e.region.nudge(action, desktop) {
            return (true, None);
        }
        let value = selection(r);
        r.queue_render();
        (true, value)
    });
    publish(value);
    accepted
}
/// Observe native repeat and focus only; Slint controls retain first refusal on key input.
pub(in crate::annotation) fn bind(
    window: &slint::Window,
    generation: u64,
    owner: Owner,
) -> impl Fn(slint::SharedString, bool, bool, bool) -> bool + use<> {
    let repeat = Rc::new(Cell::new(false));
    let epoch = Rc::new(Cell::new(0u64));
    let native_repeat = repeat.clone();
    let native_epoch = epoch.clone();
    window.on_winit_window_event(move |_, event| {
        match event {
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                native_repeat.set(event.repeat)
            }
            winit::event::WindowEvent::Focused(false) => {
                let previous = native_epoch.get();
                native_epoch.set(previous.wrapping_add(1));
                native_repeat.set(false);
                slint::Timer::single_shot(Duration::ZERO, move || {
                    lost_focus(generation, owner, previous)
                });
            }
            _ => {}
        }
        EventResult::Propagate
    });
    move |text, down, control, shift| {
        handle(
            generation,
            (owner, epoch.get()),
            text.as_str(),
            down,
            control,
            shift,
            repeat.get(),
        )
    }
}
fn lost_focus(generation: u64, owner: Owner, epoch: u64) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(r) = slot.as_mut().filter(|r| current(r, generation, owner))
            && r.screenshot
                .as_ref()
                .is_some_and(|e| e.keyboard.owner == Some((owner, epoch)))
        {
            invalidate(r);
        }
    })
}
/// First selection is a user gesture; activate once after all editor windows are prepared.
pub(in crate::annotation) fn activate_initial() {
    let context = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.canvas_state.annotation_ready {
            return None;
        };
        let e = r.screenshot.as_mut().filter(|e| e.pin.is_none())?;
        if !FLOW.with(|s| {
            let s = s.borrow();
            s.state.session == e.id && s.state.phase == ScreenshotPhase::Editing
        }) {
            return None;
        }
        let origin = e.initial_focus.take()?;
        let b = e.region.bounds()?;
        tracing::debug!(?origin, current=?r.lifecycle.annotation_foreground.as_ref().and_then(|q|q()), "Screenshot editor input activation considered");
        if !permits_focus_activation(r, origin) {
            return None;
        };
        let c = r.canvas_state.canvases.iter().find(|c| {
            b.left >= c.bounds.left
                && b.left < c.bounds.right
                && b.top >= c.bounds.top
                && b.top < c.bounds.bottom
        })?;
        Some((
            c.window.clone_strong(),
            r.lifecycle.activate_user_requested_window,
        ))
    });
    if let Some((window, activate)) = context {
        activate(window.window());
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestPlatform, lifecycle};
    use super::*;
    use slint::platform::Key;

    const ORIGINAL: Rect = Rect {
        left: 40,
        top: 40,
        right: 200,
        bottom: 160,
    };
    struct Fixture {
        owner: Owner,
        events: Rc<RefCell<Vec<ScreenshotEvent>>>,
    }
    impl Fixture {
        fn new() -> Self {
            slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
            super::super::super::init(|_| PassiveWindowPreparation::Ready, lifecycle());
            let events = Rc::new(RefCell::new(vec![]));
            let recorded = events.clone();
            super::super::super::set_handler(Rc::new(move |event| {
                if let AppEvent::Screenshot(event) = event {
                    recorded.borrow_mut().push(event);
                }
            }));
            FLOW.with(|s| {
                s.borrow_mut().state = ScreenshotState {
                    session: 51,
                    phase: ScreenshotPhase::Editing,
                    ..Default::default()
                }
            });
            let owner = REGISTRY.with(|s| {
                let mut s = s.borrow_mut();
                let r = s.as_mut().unwrap();
                r.generation = 42;
                r.document.mode = InteractionMode::Mouse;
                r.document
                    .session
                    .add(super::super::tests::object(Bounds::from_corners(
                        (60., 60.),
                        (100., 100.),
                    )));
                r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
                let bounds = Rect {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 200,
                };
                let window = AnnotationCanvas::new().unwrap();
                window.window().set_size(slint::PhysicalSize::new(300, 200));
                r.canvas_state
                    .canvases
                    .push(CanvasLayer::new(window, bounds));
                let frames = Arc::new(vec![CapturedRegion {
                    bounds,
                    pixels: [120, 160, 180, 255].repeat(300 * 200),
                }]);
                r.screenshot = Some(Editing {
                    pin: None,
                    id: 51,
                    capture: Arc::new(ScreenCaptureFrame {
                        revision: 1,
                        serial: 1,
                        regions: frames.as_ref().clone(),
                    }),
                    frames,
                    region: RegionEditor::new(ORIGINAL),
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
                Owner::Canvas(0, r.canvas_state.canvases[0].revision())
            });
            Self { owner, events }
        }
        fn key(&self, key: Key, down: bool, ctrl: bool, shift: bool, repeat: bool) -> bool {
            handle(
                42,
                (self.owner, 0),
                &char::from(key).to_string(),
                down,
                ctrl,
                shift,
                repeat,
            )
        }
        fn bounds(&self) -> Rect {
            REGISTRY.with(|s| selection(s.borrow().as_ref().unwrap()).unwrap().1)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            super::super::super::shutdown();
        }
    }
    #[test]
    fn color_key_uses_raw_pixels_during_crop_drag_and_rejects_repeat_tools_and_foreign_focus() {
        let f = Fixture::new();
        use crate::screen_interaction::pixel_picker as picker;
        let (frames, core) = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            let e = r.screenshot.as_mut().unwrap();
            assert!(e.region.begin(Point { x: 100, y: 100 }, 1., true));
            (e.frames.clone(), FLOW.with(|s| s.borrow().state.clone()))
        });
        picker::begin(51, frames, Rc::new(|| {}));
        picker::sync(&core);
        picker::prepare(1., &lifecycle()).unwrap();
        picker::point(51, Some(Point { x: 80, y: 80 }));
        let send = |repeat| handle(42, (f.owner, 0), "c", true, false, false, repeat);
        assert!(send(false));
        assert!(send(true));
        assert!(matches!(
            f.events.borrow().as_slice(),
            [ScreenshotEvent::CopyColor {
                color: lexift_core::domain::screen_snapshot::Rgb(120, 160, 180),
                point: Point { x: 80, y: 80 },
                ..
            }]
        ));
        assert_eq!(f.bounds(), ORIGINAL);
        REGISTRY
            .with(|s| s.borrow_mut().as_mut().unwrap().document.mode = InteractionMode::Tool(0));
        assert!(!send(false));
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.document.mode = InteractionMode::Mouse;
            r.lifecycle.annotation_foreground = Some(Rc::new(|| None));
        });
        assert!(!send(false));
        assert_eq!(f.events.borrow().len(), 1);
        picker::clear();
    }
    #[test]
    fn locked_crop_keyboard_routes_keep_anchor_and_escape_restores_geometry_only() {
        use lexift_core::domain::region_selection::{AspectRatio, SizeAxis};
        let f = Fixture::new();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.screenshot.as_mut().unwrap().region.apply_size(
                160,
                120,
                Some(AspectRatio::new(4, 3)),
                SizeAxis::Width,
                Rect {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 200,
                },
            );
        });
        assert!(f.key(Key::RightArrow, true, true, false, false));
        assert_eq!(
            f.bounds(),
            Rect {
                right: 201,
                bottom: 161,
                ..ORIGINAL
            }
        );
        assert!(f.key(Key::RightArrow, false, true, false, false));
        assert!(f.key(Key::DownArrow, true, true, true, false));
        assert_eq!(
            f.bounds(),
            Rect {
                right: 215,
                bottom: 171,
                ..ORIGINAL
            }
        );
        REGISTRY.with(|s| assert!(escape(s.borrow_mut().as_mut().unwrap())));
        assert_eq!(
            f.bounds(),
            Rect {
                right: 201,
                bottom: 161,
                ..ORIGINAL
            }
        );
        assert!(f.key(Key::DownArrow, true, true, true, true));
        assert_eq!(
            f.bounds(),
            Rect {
                right: 201,
                bottom: 161,
                ..ORIGINAL
            }
        );
        f.key(Key::DownArrow, false, true, false, false);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(
                r.screenshot.as_ref().unwrap().region.aspect(),
                Some(AspectRatio::new(4, 3))
            );
            assert_eq!(r.document.session.objects.len(), 1);
            assert!(r.document.session.undo());
            assert!(r.document.session.objects.is_empty());
        });
    }
    #[test]
    fn keyboard_repeats_modifiers_multiple_arrows_and_escape_share_one_snapshot() {
        let f = Fixture::new();
        assert!(f.key(Key::RightArrow, true, false, false, false));
        assert!(f.key(Key::RightArrow, true, false, true, true));
        assert!(f.key(Key::DownArrow, true, true, false, false));
        assert_eq!(
            f.bounds(),
            Rect {
                left: 51,
                right: 211,
                bottom: 161,
                ..ORIGINAL
            }
        );
        assert!(f.key(Key::RightArrow, false, false, false, false));
        REGISTRY.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .region
                    .keyboard_adjusting()
            )
        });
        REGISTRY.with(|s| assert!(escape(s.borrow_mut().as_mut().unwrap())));
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.key(Key::DownArrow, true, true, true, true));
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.key(Key::DownArrow, false, false, false, false));
        assert!(f.key(Key::DownArrow, true, true, true, false));
        assert_eq!(f.bounds().bottom, 170);
        f.key(Key::DownArrow, false, false, false, false);
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(!r.screenshot.as_ref().unwrap().region.keyboard_adjusting());
            assert_eq!(
                r.document.session.objects[0].bounds,
                Bounds::from_corners((60., 60.), (100., 100.))
            );
            assert!(r.document.session.can_undo());
            assert!(r.toolbar_state.main.get().is_some());
        });
        // Each changed down event emits once; release and suppressed repeats emit nothing.
        assert_eq!(f.events.borrow().len(), 4);
    }
    #[test]
    fn keyboard_rejects_wrong_scope_stale_input_and_noop_updates() {
        let f = Fixture::new();
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .screenshot
                .as_mut()
                .unwrap()
                .region = RegionEditor::new(Rect {
                left: 0,
                top: 0,
                right: 300,
                bottom: 200,
            })
        });
        assert!(f.key(Key::LeftArrow, true, false, true, false));
        assert!(f.key(Key::LeftArrow, false, false, false, false));
        assert!(f.events.borrow().is_empty());
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert_eq!(r.screenshot.as_ref().unwrap().revision, 0);
            r.screenshot.as_mut().unwrap().region = RegionEditor::new(ORIGINAL);
            r.screenshot.as_mut().unwrap().pin = Some((7, 9));
        });
        assert!(!f.key(Key::LeftArrow, true, false, false, false));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .screenshot
                .as_mut()
                .unwrap()
                .pin = None
        });
        FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Output);
        assert!(!f.key(Key::LeftArrow, true, false, false, false));
        FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Editing);
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Tool(0));
        });
        assert!(!f.key(Key::LeftArrow, true, false, false, false));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Mouse);
        });
        assert!(!handle(
            41,
            (f.owner, 0),
            &char::from(Key::LeftArrow).to_string(),
            true,
            false,
            false,
            false
        ));
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.key(Key::RightArrow, true, false, false, false));
        lost_focus(42, f.owner, 0);
        assert_eq!(f.bounds(), ORIGINAL);
        // A native repeat after focus loss cannot start another gesture.
        assert!(f.key(Key::RightArrow, true, false, false, true));
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(handle(
            42,
            (f.owner, 1),
            &char::from(Key::RightArrow).to_string(),
            true,
            false,
            false,
            false
        ));
        lost_focus(42, f.owner, 0); // old focus loss and late keyup cannot cancel a newer gesture
        assert!(!f.key(Key::RightArrow, false, false, false, false));
        assert_eq!(f.bounds().left, 41);
        commit_current();
        REGISTRY.with(|s| {
            assert!(
                !s.borrow()
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .region
                    .keyboard_adjusting()
            )
        });
        assert_eq!(
            f.events.borrow().last(),
            Some(&ScreenshotEvent::Selected {
                session: 51,
                bounds: f.bounds()
            })
        );
    }

    #[test]
    fn keyboard_slint_canvas_toolbar_routes_and_owned_controls() {
        use slint::platform::WindowEvent;
        let f = Fixture::new();
        let (canvas, toolbar, toolbar_owner) = REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            let canvas = match &r.canvas_state.canvases[0].window {
                canvas::Surface::Canvas(c) => c.clone_strong(),
                _ => panic!("screen canvas expected"),
            };
            (
                canvas,
                r.toolbar_state.main.get().unwrap().clone_strong(),
                Owner::Toolbar(r.toolbar_state.main.revision()),
            )
        });
        canvas.on_region_key(bind(canvas.window(), 42, f.owner));
        toolbar.on_region_key(bind(toolbar.window(), 42, toolbar_owner));
        canvas.show().unwrap();
        toolbar.show().unwrap();
        for window in [canvas.window(), toolbar.window()] {
            window.dispatch_event(WindowEvent::WindowActiveChanged(true));
            window.dispatch_event(WindowEvent::KeyPressed {
                text: char::from(Key::RightArrow).to_string().into(),
            });
            window.dispatch_event(WindowEvent::KeyReleased {
                text: char::from(Key::RightArrow).to_string().into(),
            });
        }
        assert_eq!(f.bounds().left, 42);
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.toolbar_state.panel.open(AnnotationPanel::new().unwrap());
        });
        assert!(!f.key(Key::DownArrow, true, false, false, false));
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.toolbar_state.panel.clear();
            r.toolbar_state
                .watermark
                .open(AnnotationWatermarkInput::new().unwrap());
        });
        assert!(!f.key(Key::DownArrow, true, false, false, false));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .toolbar_state
                .watermark
                .clear()
        });
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert!(r.screenshot.as_mut().unwrap().region.begin(
                Point { x: 100, y: 100 },
                1.,
                true
            ));
        });
        assert!(!f.key(Key::DownArrow, true, false, false, false));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .screenshot
                .as_mut()
                .unwrap()
                .region
                .cancel();
        });
        // Changing tools during an uncommitted segment rolls back only that segment.
        assert!(f.key(Key::DownArrow, true, false, false, false));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Tool(0));
        });
        assert_eq!(
            f.bounds(),
            Rect {
                left: 42,
                right: 202,
                ..ORIGINAL
            }
        );
        assert!(!f.key(Key::DownArrow, false, false, false, false));
    }

    #[test]
    fn keyboard_initial_activation_is_prepared_scoped_and_attempted_once() {
        thread_local! {static ACTIVATIONS:Cell<usize>=const { Cell::new(0) };}
        fn activate(_: &slint::Window) -> bool {
            ACTIVATIONS.with(|c| c.set(c.get() + 1));
            false // native failure must not cause repeated activation attempts
        }
        let _f = Fixture::new();
        let revision = REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.lifecycle.activate_user_requested_window = activate;
            r.screenshot.as_mut().unwrap().initial_focus = Some(focus::Origin::Unobserved);
            r.toolbar_state.main.revision()
        });
        super::super::toolbar_ready(42, revision);
        ACTIVATIONS.with(|c| assert_eq!(c.get(), 0));
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .canvas_state
                .annotation_ready = true
        });
        super::super::toolbar_ready(41, revision);
        super::super::toolbar_ready(42, revision.wrapping_add(1));
        ACTIVATIONS.with(|c| assert_eq!(c.get(), 0));
        super::super::toolbar_ready(42, revision);
        super::super::toolbar_ready(42, revision);
        ACTIVATIONS.with(|c| assert_eq!(c.get(), 1));
    }

    #[test]
    fn help_mode_and_parameter_focus_refresh_without_document_changes() {
        let _f = Fixture::new();
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            let monitor = r.canvas_state.canvases[0].bounds;
            crate::screen_interaction::screenshot_help::prepare(1., monitor, &r.lifecycle).unwrap();
            let mouse = super::super::help_feedback(r).unwrap();
            r.render_state.frame_tasks.cancel_render();
            r.set_mode(InteractionMode::Tool(0));
            assert!(
                r.render_state.frame_tasks.render_pending(),
                "tool change must refresh read-only feedback"
            );
            let drawing = super::super::help_feedback(r).unwrap();
            assert_ne!(
                mouse.bounds.bottom - mouse.bounds.top,
                drawing.bounds.bottom - drawing.bounds.top
            );
            let panel = AnnotationPanel::new().unwrap();
            panel.set_menu(false);
            r.toolbar_state.panel.open(panel);
            assert!(
                super::super::help_feedback(r).is_some(),
                "a normal options panel does not own text input"
            );
        });
        for active in [true, false] {
            let revision = REGISTRY.with(|s| {
                let mut s = s.borrow_mut();
                let r = s.as_mut().unwrap();
                r.render_state.frame_tasks.cancel_render();
                r.toolbar_state
                    .panel
                    .get()
                    .unwrap()
                    .set_parameter_input_active(active);
                r.toolbar_state.panel.revision()
            });
            super::super::super::apply_panel_input(
                42,
                revision.wrapping_add(1),
                0,
                panel::Input::FocusChanged,
            );
            REGISTRY.with(|s| {
                assert!(
                    !s.borrow()
                        .as_ref()
                        .unwrap()
                        .render_state
                        .frame_tasks
                        .render_pending()
                )
            });
            super::super::super::apply_panel_input(42, revision, 0, panel::Input::FocusChanged);
            REGISTRY.with(|s| {
                let s = s.borrow();
                let r = s.as_ref().unwrap();
                assert!(r.render_state.frame_tasks.render_pending());
                assert_eq!(super::super::help_feedback(r).is_none(), active);
                assert_eq!(r.document.session.objects.len(), 1);
            });
        }
    }
    #[test]
    fn keyboard_output_commits_latest_crop_before_snapshot_without_undo_steps() {
        let f = Fixture::new();
        f.key(Key::RightArrow, true, false, false, false);
        f.key(Key::DownArrow, true, true, true, false);
        let latest = f.bounds();
        super::super::output(ScreenshotOutput::Copy);
        let events = f.events.borrow();
        let [
            ..,
            ScreenshotEvent::Selected { bounds, .. },
            ScreenshotEvent::Output { request, .. },
        ] = events.as_slice()
        else {
            panic!("crop must be synchronized before output")
        };
        assert_eq!(*bounds, latest);
        assert_eq!(request.snapshot.monitor, latest);
        drop(events);
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert!(!r.screenshot.as_ref().unwrap().region.keyboard_adjusting());
            assert_eq!(r.document.session.objects.len(), 1);
            assert!(r.document.session.undo());
            assert!(r.document.session.objects.is_empty());
            assert!(
                !r.document.session.undo(),
                "crop changes must not create history steps"
            );
        });
        FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Output);
        assert!(!f.key(Key::RightArrow, true, false, false, false));
        assert_eq!(f.bounds(), latest);
    }
    #[test]
    fn ocr_commits_keyboard_crop_blocks_mutation_and_owns_post_capture_feedback() {
        use lexift_core::domain::screenshot::OcrOutcome;
        let f = Fixture::new();
        f.key(Key::RightArrow, true, false, false, false);
        let expected = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .screenshot
                .as_ref()
                .unwrap()
                .region
                .bounds()
                .unwrap()
        });
        super::super::recognize_text();
        assert!(f.events.borrow().iter().any(
            |e| matches!(e, ScreenshotEvent::Recognize {session:51,bounds} if *bounds == expected)
        ));
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(r.toolbar_state.panel.get().is_none());
            assert!(r.editor.window().is_none());
        });
        let recovery = REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            super::super::Recovery {
                document: r.document.clone(),
                position: slint::PhysicalPosition::new(10, 10),
                collapsed: true,
                lifecycle: r.lifecycle.clone(),
            }
        });
        FLOW.with(|s| s.borrow_mut().recovery = Some(recovery));
        let objects = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .document
                .session
                .objects
                .clone()
        });
        let mut busy = FLOW.with(|s| s.borrow().state.clone());
        busy.phase = ScreenshotPhase::Recognizing;
        super::super::sync(&busy);
        assert!(!f.key(Key::RightArrow, true, false, false, true));
        super::super::super::undo();
        super::super::super::delete_selected();
        super::super::recognize_text();
        super::super::request(); // Repeated screenshot request must preserve the feedback context.
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert!(!r.set_mode(InteractionMode::Tool(0)));
            assert!(super::super::pointer(r, 0, 0, 70., 70.));
            assert!(super::super::pointer(r, 0, 1, 100., 100.));
            assert!(super::super::pointer(r, 0, 2, 100., 100.));
            assert_eq!(r.document.session.objects, objects);
            assert_eq!(
                r.screenshot.as_ref().unwrap().region.bounds(),
                Some(expected)
            );
            assert!(r.toolbar_state.main.get().unwrap().get_output_busy());
        });
        super::super::super::escape_from(focus::Origin::Unobserved);
        assert!(matches!(
            f.events.borrow().last(),
            Some(ScreenshotEvent::Cancelled { session: 51 })
        ));
        let mut done = busy;
        done.phase = ScreenshotPhase::Idle;
        done.ocr_result = Some(OcrOutcome::Copied);
        super::super::sync(&done);
        super::super::restore(51);
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(r.screenshot.is_none());
            assert_eq!(r.document.session.objects, objects);
            assert!(r.document.mode.is_mouse());
        });
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
            if FLOW.with(|s| {
                s.borrow()
                    .toast
                    .get()
                    .is_some_and(|t| t.window().is_visible())
            }) {
                break;
            }
        }
        FLOW.with(|s| {
            let s = s.borrow();
            let toast = s.toast.get().unwrap();
            assert_eq!(
                toast.get_message(),
                crate::i18n::tr(OcrOutcome::Copied.message())
            );
            assert!(!toast.get_error());
            assert!(toast.window().is_visible());
            assert!(
                toast.window().size().width >= 320,
                "toast must not use Winit's 10px initial width"
            );
        });
        // New session retires the independent toast and all delayed show work.
        super::super::sync(&ScreenshotState {
            session: 52,
            phase: ScreenshotPhase::Preparing,
            ..Default::default()
        });
        assert!(FLOW.with(|s| s.borrow().toast.get().is_none()));
        super::super::super::shutdown();
    }
}
