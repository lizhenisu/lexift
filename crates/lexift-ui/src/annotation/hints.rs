//! Delayed, pointer-transparent hints owned by the annotation window instance.
//! Winit's embedded Tooltip is clipped to its parent, so the 54px toolbar uses a passive host.
use super::*;
use crate::{AnnotationHintWindow, AnnotationHints};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Toolbar(u64),
    Panel(u64),
}
#[derive(Clone)]
struct Request {
    owner: Owner,
    anchor: [f32; 4],
    text: slint::SharedString,
    origin: slint::PhysicalPosition,
}
#[derive(Default)]
pub(super) struct State {
    timer: slint::Timer,
    revision: u64,
    pending: Option<Request>,
    pub(super) window: window::Slot<AnnotationHintWindow>,
}
impl State {
    pub(super) fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.timer.stop();
        self.pending = None;
        self.window.clear();
    }
}
fn owner_window(r: &Registry, owner: Owner) -> Option<&slint::Window> {
    match owner {
        Owner::Toolbar(id) if id == r.toolbar_state.main.revision() => {
            r.toolbar_state.main.get().map(ComponentHandle::window)
        }
        Owner::Panel(id) if id == r.toolbar_state.panel.revision() => {
            r.toolbar_state.panel.get().map(ComponentHandle::window)
        }
        _ => None,
    }
}
macro_rules! bind {
    ($name:ident, $ty:ty) => {
        pub(super) fn $name(view: &$ty, generation: u64, owner: Owner) {
            view.global::<AnnotationHints>()
                .on_requested(move |text, x, y, w, h| {
                    // Slint bindings can change while the registry is borrowed; defer without retaining a component.
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        request(generation, owner, text, [x, y, w, h])
                    });
                });
            view.global::<AnnotationHints>().on_cancelled(move |x, y| {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    cancel(generation, owner, [x, y])
                });
            });
        }
    };
}
bind!(bind_toolbar, AnnotationToolbar);
bind!(bind_panel, AnnotationPanel);
fn request(generation: u64, owner: Owner, text: slint::SharedString, anchor: [f32; 4]) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(window) = owner_window(r, owner) else {
            return;
        };
        if !window.is_visible() || text.is_empty() {
            return;
        }
        let origin = window.position();
        r.hints.clear();
        r.hints.pending = Some(Request {
            owner,
            anchor,
            text,
            origin,
        });
        let revision = r.hints.revision;
        r.hints.timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(1000),
            move || show(generation, revision, 0),
        );
    });
}
fn cancel(generation: u64, owner: Owner, anchor: [f32; 2]) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(r) = slot.as_mut().filter(|r| r.generation == generation)
            && r.hints
                .pending
                .as_ref()
                .is_some_and(|p| p.owner == owner && p.anchor[..2] == anchor)
        {
            r.hints.clear();
        }
    });
}
/// Physical placement keeps hints outside the button while respecting negative-coordinate work areas.
fn place(anchor: Rect, size: (i32, i32), area: Rect) -> Point {
    let x = anchor
        .left
        .clamp(area.left, (area.right - size.0).max(area.left));
    let below = anchor.bottom + 4;
    let y = if below + size.1 <= area.bottom {
        below
    } else {
        anchor.top - size.1 - 4
    };
    Point {
        x,
        y: y.clamp(area.top, (area.bottom - size.1).max(area.top)),
    }
}
fn show(generation: u64, revision: u64, attempt: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.hints.revision == revision)
        else {
            return;
        };
        let Some(request) = r.hints.pending.clone() else {
            return;
        };
        let Some(owner) = owner_window(r, request.owner) else {
            r.hints.clear();
            return;
        };
        if !owner.is_visible()
            || owner.position() != request.origin
            || r.toolbar_state.toolbar_drag.is_some()
        {
            r.hints.clear();
            return;
        }
        let scale = owner.scale_factor();
        let anchor = Rect {
            left: request.origin.x + (request.anchor[0] * scale).round() as i32,
            top: request.origin.y + (request.anchor[1] * scale).round() as i32,
            right: request.origin.x
                + ((request.anchor[0] + request.anchor[2]) * scale).round() as i32,
            bottom: request.origin.y
                + ((request.anchor[1] + request.anchor[3]) * scale).round() as i32,
        };
        let Some(area) = (r.lifecycle.popup_work_area)(Point {
            x: anchor.left,
            y: anchor.top,
        }) else {
            r.hints.clear();
            return;
        };
        if r.hints.window.get().is_none() {
            let Ok(hint) =
                crate::screen_interaction::create(&r.lifecycle, AnnotationHintWindow::new)
            else {
                r.hints.clear();
                return;
            };
            crate::theme::apply(&hint);
            hint.set_text(request.text);
            hint.set_limit(((area.right - area.left) as f32 / scale - 16.).clamp(28., 320.));
            hint.window()
                .set_position(slint::PhysicalPosition::new(anchor.left, anchor.bottom));
            r.hints.window.open(hint);
        }
        let hint = r.hints.window.get().unwrap();
        match (r.prepare)(hint.window()) {
            PassiveWindowPreparation::Ready => {
                let owner = owner_window(r, request.owner).unwrap();
                let scale = hint.window().scale_factor();
                // Native font metrics and wrapped height settle after evaluating the final width.
                // Keep both measurements hidden so the first visible frame is already compact.
                for _ in 0..3 {
                    let size = slint::LogicalSize::new(
                        hint.get_content_width(),
                        hint.get_content_height(),
                    );
                    hint.window().set_size(size);
                    let _ = hint.window().take_snapshot();
                    if (hint.get_content_width() - size.width).abs() < 0.1
                        && (hint.get_content_height() - size.height).abs() < 0.1
                    {
                        break;
                    }
                }
                hint.window().set_size(slint::LogicalSize::new(
                    hint.get_content_width(),
                    hint.get_content_height(),
                ));
                let point = place(
                    anchor,
                    (
                        (hint.get_content_width() * scale).ceil() as i32,
                        (hint.get_content_height() * scale).ceil() as i32,
                    ),
                    area,
                );
                hint.window()
                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
                if !(r.lifecycle.attach_tool_window)(hint.window(), owner)
                    || !(r.lifecycle.complete_toolbar_show)(hint.window(), Rc::new(|_| {}))
                    || !(r.lifecycle.configure_hint_window)(hint.window())
                    || hint.show().is_err()
                {
                    r.hints.clear();
                }
                // No annotation pixels changed. Future captures consult the live exclusion list;
                // do not wake the renderer or allocate a GPU scene just to show a hint.
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                r.hints.timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(16),
                    move || show(generation, revision, attempt + 1),
                );
            }
            _ => r.hints.clear(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::cell::Cell;
    slint::slint! {
        import {ToolButton} from "ui/annotation.slint";
        export {AnnotationHints} from "ui/annotation-hint.slint";
        export component ButtonProbe inherits Window {
            in property <bool> available: false;
            callback invoked();
            ToolButton { x:10px;y:10px;kind:14;enabled:root.available;clicked=>{root.invoked();} }
        }
    }
    struct Clock(Rc<Cell<Duration>>);
    impl Platform for Clock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    #[test]
    fn delay_replacement_cancellation_and_retired_owner() {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
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
        let (generation, owner) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.lifecycle.popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                })
            });
            r.lifecycle.attach_tool_window = Rc::new(|_, _| true);
            r.lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
            r.lifecycle.configure_hint_window = Rc::new(|_| true);
            let main = AnnotationToolbar::new().unwrap();
            main.show().unwrap();
            let revision = r.toolbar_state.main.open(main);
            (r.generation, Owner::Toolbar(revision))
        });
        let advance = |ms| {
            clock.set(Duration::from_millis(ms));
            slint::platform::update_timers_and_animations();
        };
        let visible = || {
            REGISTRY.with(|s| {
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .hints
                    .window
                    .get()
                    .is_some_and(|w| w.window().is_visible())
            })
        };
        request(generation, owner, "first".into(), [10., 10., 38., 38.]);
        advance(999);
        assert!(!visible());
        advance(1000);
        assert!(visible());
        cancel(generation, owner, [10., 10.]);
        assert!(!visible());
        request(generation, owner, "first".into(), [10., 10., 38., 38.]);
        advance(1500);
        request(generation, owner, "second".into(), [60., 10., 38., 38.]);
        cancel(generation, owner, [10., 10.]); // previous target leaving must not cancel the next.
        advance(2499);
        assert!(!visible());
        advance(2500);
        assert!(visible());
        cancel(generation, owner, [60., 10.]);
        request(generation, owner, "cancelled".into(), [10., 10., 38., 38.]);
        cancel(generation, owner, [10., 10.]);
        advance(4000);
        assert!(!visible());
        request(generation, owner, "retired".into(), [10., 10., 38., 38.]);
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            s.as_mut().unwrap().toolbar_state.main.clear();
        });
        advance(5000);
        assert!(!visible());
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            let main = AnnotationToolbar::new().unwrap();
            main.show().unwrap();
            r.toolbar_state.main.open(main);
        });
        request(
            generation,
            owner,
            "old callback".into(),
            [10., 10., 38., 38.],
        );
        advance(6000);
        assert!(!visible());
        lifecycle::shutdown();
    }
    #[test]
    fn placement_flips_clamps_and_supports_negative_coordinates() {
        let area = Rect {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        };
        assert_eq!(
            place(
                Rect {
                    left: -10,
                    top: 940,
                    right: 0,
                    bottom: 978
                },
                (140, 28),
                area
            ),
            Point { x: -140, y: 908 }
        );
        assert_eq!(
            place(
                Rect {
                    left: -1900,
                    top: 0,
                    right: -1880,
                    bottom: 38
                },
                (140, 28),
                area
            ),
            Point { x: -1900, y: 42 }
        );
    }
    #[test]
    fn disabled_button_hover_and_press_dismissal_do_not_invoke_actions() {
        use slint::platform::{PointerEventButton as B, WindowEvent as E};
        slint::platform::set_platform(Box::new(Clock(Rc::new(Cell::new(Duration::ZERO))))).unwrap();
        let probe = ButtonProbe::new().unwrap();
        probe.window().set_size(slint::LogicalSize::new(80., 80.));
        probe.show().unwrap();
        let requests = Rc::new(Cell::new(0));
        let cancellations = Rc::new(Cell::new(0));
        let clicks = Rc::new(Cell::new(0));
        let n = requests.clone();
        probe
            .global::<AnnotationHints>()
            .on_requested(move |text, _, _, _, _| {
                assert!(text.contains("Ctrl+Z"));
                n.set(n.get() + 1);
            });
        let n = cancellations.clone();
        probe
            .global::<AnnotationHints>()
            .on_cancelled(move |_, _| n.set(n.get() + 1));
        let n = clicks.clone();
        probe.on_invoked(move || n.set(n.get() + 1));
        let position = slint::LogicalPosition::new(25., 25.);
        probe.window().dispatch_event(E::PointerMoved { position });
        assert_eq!(requests.get(), 1, "disabled buttons still expose names");
        probe.window().dispatch_event(E::PointerPressed {
            position,
            button: B::Left,
        });
        probe.window().dispatch_event(E::PointerReleased {
            position,
            button: B::Left,
        });
        assert_eq!(clicks.get(), 0);
        assert!(cancellations.get() > 0);
        probe.window().dispatch_event(E::PointerMoved { position });
        assert_eq!(requests.get(), 1, "press latches dismissal until leaving");
        probe.window().dispatch_event(E::PointerMoved {
            position: slint::LogicalPosition::new(70., 70.),
        });
        probe.set_available(true);
        probe.window().dispatch_event(E::PointerMoved { position });
        assert_eq!(requests.get(), 2);
        probe.window().dispatch_event(E::PointerPressed {
            position,
            button: B::Left,
        });
        probe.window().dispatch_event(E::PointerReleased {
            position,
            button: B::Left,
        });
        assert_eq!(clicks.get(), 1);
    }
    #[test]
    fn hint_renders_all_languages_themes_and_scales_without_clipping() {
        use lexift_core::domain::{settings::ThemePreference, ui_language::UiLanguage};
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock))).unwrap();
        let hint = AnnotationHintWindow::new().unwrap();
        for language in UiLanguage::ALL {
            crate::i18n::select(language);
            for theme in [ThemePreference::Light, ThemePreference::Dark] {
                crate::theme::set_current(theme);
                crate::theme::apply(&hint);
                for scale in [1., 1.25] {
                    hint.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        },
                    );
                    hint.set_text(crate::i18n::tr("Choose mosaic tool").into());
                    hint.set_limit(150.);
                    hint.window().set_size(slint::LogicalSize::new(
                        hint.get_content_width(),
                        hint.get_content_height(),
                    ));
                    let _ = hint.window().take_snapshot();
                    hint.window().set_size(slint::LogicalSize::new(
                        hint.get_content_width(),
                        hint.get_content_height(),
                    ));
                    assert!(hint.get_content_width() <= 150.);
                    assert!(hint.get_content_height() >= 28.);
                    let image = hint.window().take_snapshot().unwrap();
                    if let Ok(dir) = std::env::var("LEXIFT_HINT_SNAPSHOT_DIR") {
                        crate::i18n::write_test_bitmap(
                            &dir,
                            &format!("hint-{}-{theme:?}-{scale}.bmp", language.code()),
                            image.width(),
                            image.height(),
                            image.as_slice().iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
    }
}
