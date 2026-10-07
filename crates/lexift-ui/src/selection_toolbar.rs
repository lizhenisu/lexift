//! Passive selection toolbar lifecycle. No visible native-window priming and
//! no direct Core dispatch while Slint/native pointer callbacks are borrowed.
use super::{
    CompletePassiveWindowShow, PassiveWindowPreparation, PopupPointerInput, PopupPointerSink,
    PopupWorkArea, SetPopupDismissal, ToolbarCursorPosition,
};
use crate::{SelectionToolbarWindow, placement};
use lexift_core::{
    AppEvent,
    domain::{
        geometry::{Point, Rect},
        selection::{Selection, SelectionTarget},
    },
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
const TOOLBAR_FADE_SAMPLE_INTERVAL: Duration = Duration::from_millis(33);
const TOOLBAR_FADE_START_LOGICAL_PX: f64 = 24.0;
const TOOLBAR_FADE_END_LOGICAL_PX: f64 = 220.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Visible,
    Closed,
}

/// One instance owns both retry and deferred action delivery. Retained components
/// become inert on retirement, including already queued native callbacks.
pub(super) struct Work {
    phase: Cell<Phase>,
    prepare_timer: slint::Timer,
    input_timer: slint::Timer,
    pending: RefCell<Vec<AppEvent>>,
    generation: u64,
    placement: Cell<Option<(Point, u32, u32, u32)>>,
}
impl Work {
    fn new(generation: u64) -> Rc<Self> {
        Rc::new(Self {
            phase: Cell::new(Phase::Preparing),
            prepare_timer: Default::default(),
            input_timer: Default::default(),
            pending: Default::default(),
            generation,
            placement: Cell::new(None),
        })
    }
    fn retire(&self) {
        self.phase.set(Phase::Closed);
        self.prepare_timer.stop();
        self.input_timer.stop();
        self.pending.borrow_mut().clear();
    }
    fn post(self: &Rc<Self>, event: AppEvent) {
        if self.phase.get() != Phase::Visible {
            return;
        }
        self.pending.borrow_mut().push(event);
        if self.input_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.input_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                let Some(work) = weak.upgrade() else { return };
                let events = std::mem::take(&mut *work.pending.borrow_mut());
                for event in events {
                    let handler = SELECTION_TOOLBAR_REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref()?;
                        (work.phase.get() == Phase::Visible
                            && r.generation == work.generation
                            && r.work.as_ref().is_some_and(|w| Rc::ptr_eq(w, &work))
                            && r.target_valid())
                        .then(|| r.handler.clone())
                        .flatten()
                    });
                    if let Some(handler) = handler {
                        handler(event);
                    }
                }
            });
    }
}
thread_local! {
    pub(super) static SELECTION_TOOLBAR_REGISTRY: RefCell<Option<SelectionToolbarRegistry>> = const { RefCell::new(None) };
}

pub(super) struct SelectionToolbarRegistry {
    pub(super) window: Option<SelectionToolbarWindow>,
    pub(super) selection: Option<Selection>,
    pub(super) handler: Option<Rc<dyn Fn(AppEvent)>>,
    pub(super) prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    pub(super) complete: CompletePassiveWindowShow,
    pub(super) dismiss: SetPopupDismissal,
    pub(super) work_area: PopupWorkArea,
    pub(super) cursor_position: ToolbarCursorPosition,
    pub(super) fade_timer: slint::Timer,
    pub(super) generation: u64,
    pub(super) target: Option<SelectionTarget>,
    pub(super) target_current: Rc<dyn Fn(SelectionTarget) -> bool>,
    pub(super) work: Option<Rc<Work>>,
}

impl SelectionToolbarRegistry {
    pub(super) fn close(&mut self) {
        if let Some(work) = self.work.take() {
            work.retire();
        }
        self.generation = self.generation.wrapping_add(1);
        self.fade_timer.stop();
        if let Some(window) = self.window.take() {
            let _ = (self.dismiss)(window.window(), false);
            let _ = window.hide();
        }
        self.selection = None;
        self.target = None;
    }

    fn target_valid(&self) -> bool {
        self.target
            .is_none_or(|target| (self.target_current)(target))
    }

    pub(super) fn sync(&mut self, selection: Option<Selection>, target: Option<SelectionTarget>) {
        if self.selection == selection && self.target == target {
            return;
        }
        self.close();
        let Some(selection) = selection else { return };
        self.target = target;
        if !self.target_valid() {
            self.target = None;
            return;
        }
        let Ok(window) = SelectionToolbarWindow::new() else {
            return;
        };
        crate::theme::apply(&window);
        if self.handler.is_none() {
            return;
        }
        let generation = self.generation.wrapping_add(1);
        let work = Work::new(generation);
        let action = work.clone();
        window.on_translate_requested(move || {
            action.post(AppEvent::SelectionToolbarTranslateRequested)
        });
        let action = work.clone();
        window.on_copy_requested(move || action.post(AppEvent::SelectionToolbarCopyRequested));
        let action = work.clone();
        window.window().on_close_requested(move || {
            action.post(AppEvent::SelectionToolbarDismissRequested);
            slint::CloseRequestResponse::KeepWindowShown
        });
        window
            .window()
            .set_size(slint::LogicalSize::new(96.0, 48.0));
        self.selection = Some(selection);
        self.window = Some(window);
        self.generation = self.generation.wrapping_add(1);
        self.work = Some(work.clone());
        schedule_toolbar_show(&work, 0);
    }
}

impl Drop for SelectionToolbarRegistry {
    fn drop(&mut self) {
        self.close();
    }
}

fn schedule_toolbar_show(work: &Rc<Work>, attempt: u8) {
    let weak = Rc::downgrade(work);
    work.prepare_timer.start(
        slint::TimerMode::SingleShot,
        if attempt == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(16)
        },
        move || {
            let Some(work) = weak.upgrade().filter(|w| w.phase.get() == Phase::Preparing) else {
                return;
            };
            SELECTION_TOOLBAR_REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(r) = slot.as_mut().filter(|r| {
                    r.generation == work.generation
                        && r.work.as_ref().is_some_and(|w| Rc::ptr_eq(w, &work))
                }) else {
                    return;
                };
                if !r.target_valid() {
                    r.close();
                    return;
                }
                let Some(window) = r.window.as_ref() else {
                    return;
                };
                match (r.prepare)(window.window()) {
                    PassiveWindowPreparation::Ready => {
                        if let Some(selection) = r.selection.as_ref()
                            && let Some(anchor) = selection.anchor
                        {
                            let Some(area) = (r.work_area)(anchor) else {
                                tracing::debug!("selection toolbar work area unavailable");
                                r.close();
                                return;
                            };
                            let size = window.window().size();
                            let scale = window.window().scale_factor();
                            let Some(point) = placement::place_selection_toolbar(
                                &selection.bounds,
                                selection.gesture_start.unwrap_or(anchor),
                                anchor,
                                (size.width, size.height),
                                scale,
                                area,
                            ) else {
                                tracing::debug!("selection toolbar has no unobstructed placement");
                                r.close();
                                return;
                            };
                            let geometry = (point, size.width, size.height, scale.to_bits());
                            if work.placement.get() != Some(geometry) {
                                if attempt >= 19 {
                                    tracing::warn!(
                                        "selection toolbar DPI/placement did not stabilize"
                                    );
                                    r.close();
                                    return;
                                }
                                work.placement.set(Some(geometry));
                                window
                                    .window()
                                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
                                schedule_toolbar_show(&work, attempt + 1);
                                return;
                            }
                        }
                        // Style and pointer subclass precede the first visible frame.
                        if !(r.complete)(
                            window.window(),
                            toolbar_pointer_sink(window.as_weak(), work.clone()),
                        ) || !r.target_valid()
                            || window.show().is_err()
                        {
                            tracing::warn!(
                                "selection toolbar could not complete passive preparation"
                            );
                            r.close();
                            return;
                        }
                        work.phase.set(Phase::Visible);
                        if !(r.dismiss)(window.window(), true) {
                            tracing::warn!("selection toolbar could not monitor dismissal");
                            r.close();
                            return;
                        }
                        r.fade_timer.start(
                            slint::TimerMode::Repeated,
                            TOOLBAR_FADE_SAMPLE_INTERVAL,
                            update_toolbar_fade,
                        );
                    }
                    PassiveWindowPreparation::Pending if attempt < 19 => {
                        // Winit creates inactive adapters with a hidden HWND on the
                        // next event-loop turn. Visible priming risks activating an
                        // unconfigured native window before its passive policy exists.
                        schedule_toolbar_show(&work, attempt + 1);
                    }
                    _ => {
                        tracing::warn!(attempt, "selection toolbar native preparation failed");
                        r.close();
                    }
                }
            });
        },
    );
}

/// Measures from the cursor to the nearest point on the toolbar in logical pixels.
pub(super) fn toolbar_opacity(cursor: Point, bounds: Rect, scale_factor: f32) -> f32 {
    let dx = (i64::from(bounds.left) - i64::from(cursor.x))
        .max(0)
        .max(i64::from(cursor.x) - i64::from(bounds.right));
    let dy = (i64::from(bounds.top) - i64::from(cursor.y))
        .max(0)
        .max(i64::from(cursor.y) - i64::from(bounds.bottom));
    let physical_distance = ((dx as f64).hypot(dy as f64)) / f64::from(scale_factor.max(0.1));
    ((TOOLBAR_FADE_END_LOGICAL_PX - physical_distance)
        / (TOOLBAR_FADE_END_LOGICAL_PX - TOOLBAR_FADE_START_LOGICAL_PX))
        .clamp(0.0, 1.0) as f32
}

fn update_toolbar_fade() {
    let dismiss = SELECTION_TOOLBAR_REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let registry = slot.as_mut()?;
        if !registry.target_valid() {
            let handler = registry.handler.clone();
            registry.close();
            return handler;
        }
        let cursor = (registry.cursor_position)()?;
        let window = registry.window.as_ref()?;
        let position = window.window().position();
        let size = window.window().size();
        let bounds = Rect {
            left: position.x,
            top: position.y,
            right: position.x.saturating_add(size.width as i32),
            bottom: position.y.saturating_add(size.height as i32),
        };
        let opacity = toolbar_opacity(cursor, bounds, window.window().scale_factor());
        if (window.get_fade_opacity() - opacity).abs() > 0.001 {
            window.set_fade_opacity(opacity);
        }
        if opacity > 0.0 {
            return None;
        }
        let handler = registry.handler.as_ref().map(Rc::clone);
        registry.close();
        handler
    });
    if let Some(handler) = dismiss {
        handler(AppEvent::SelectionToolbarDismissRequested);
    }
}

fn toolbar_pointer_sink(
    weak: slint::Weak<SelectionToolbarWindow>,
    work: Rc<Work>,
) -> PopupPointerSink {
    Rc::new(move |input| {
        if work.phase.get() != Phase::Visible {
            return;
        }
        let Some(window) = weak.upgrade() else { return };
        if matches!(input, PopupPointerInput::DismissRequested { .. }) {
            work.post(AppEvent::SelectionToolbarDismissRequested);
            return;
        }
        use slint::platform::{PointerEventButton, WindowEvent};
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        let event = match input {
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
                position: position(x, y),
            },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y }
            | PopupPointerInput::LeftCancelled { x, y } => WindowEvent::PointerReleased {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            _ => return,
        };
        let _ = window.window().dispatch_event_with_result(event);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{Platform, WindowAdapter, software_renderer::SoftwareRenderer};
    use std::time::Instant;
    struct Adapter {
        window: slint::Window,
        renderer: SoftwareRenderer,
        visible: Cell<bool>,
        shows: Cell<u32>,
    }
    impl WindowAdapter for Adapter {
        fn window(&self) -> &slint::Window {
            &self.window
        }
        fn size(&self) -> slint::PhysicalSize {
            slint::PhysicalSize::new(96, 48)
        }
        fn renderer(&self) -> &dyn slint::platform::Renderer {
            &self.renderer
        }
        fn set_visible(&self, visible: bool) -> Result<(), slint::PlatformError> {
            self.visible.set(visible);
            if visible {
                self.shows.set(self.shows.get() + 1);
            }
            Ok(())
        }
    }
    struct TestPlatform(Rc<RefCell<Vec<Rc<Adapter>>>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let adapter = Rc::new_cyclic(|weak: &std::rc::Weak<Adapter>| Adapter {
                window: slint::Window::new(weak.clone()),
                renderer: SoftwareRenderer::new(),
                visible: Cell::new(false),
                shows: Cell::new(0),
            });
            self.0.borrow_mut().push(adapter.clone());
            Ok(adapter)
        }
    }
    thread_local! { static PREPARES: Cell<u32> = const { Cell::new(0) }; }
    fn prepare(_: &slint::Window) -> PassiveWindowPreparation {
        PREPARES.with(|n| {
            let count = n.get();
            n.set(count + 1);
            if count == 0 {
                PassiveWindowPreparation::Pending
            } else {
                PassiveWindowPreparation::Ready
            }
        })
    }
    fn drain(ms: u64) {
        let start = Instant::now();
        loop {
            slint::platform::update_timers_and_animations();
            if start.elapsed().as_millis() >= u128::from(ms) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn install(
        prepare: fn(&slint::Window) -> PassiveWindowPreparation,
        adapters: Rc<RefCell<Vec<Rc<Adapter>>>>,
        calls: Rc<Cell<u32>>,
    ) {
        slint::platform::set_platform(Box::new(TestPlatform(adapters.clone()))).unwrap();
        SELECTION_TOOLBAR_REGISTRY.with(|slot| {
            *slot.borrow_mut() = Some(SelectionToolbarRegistry {
                window: None,
                selection: None,
                handler: Some(Rc::new(move |_| calls.set(calls.get() + 1))),
                prepare,
                complete: Rc::new(move |_, _| {
                    assert!(
                        !adapters.borrow().last().unwrap().visible.get(),
                        "native interaction must precede first show"
                    );
                    true
                }),
                dismiss: Rc::new(|_, _| true),
                work_area: Rc::new(|_| None),
                cursor_position: Rc::new(|| Some(Point { x: 0, y: 0 })),
                fade_timer: Default::default(),
                generation: 0,
                target: None,
                target_current: Rc::new(|_| true),
                work: None,
            })
        });
    }
    fn sync() {
        SELECTION_TOOLBAR_REGISTRY.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().sync(
                Some(Selection {
                    bounds: Vec::new(),
                    gesture_start: None,
                    text: "fixture".into(),
                    anchor: None,
                }),
                None,
            )
        });
    }
    #[test]
    fn pending_preparation_never_shows_and_retired_callbacks_cannot_operate_reopened_toolbar() {
        let adapters = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(Cell::new(0));
        install(prepare, adapters.clone(), calls.clone());
        sync();
        drain(0);
        assert_eq!(
            adapters.borrow()[0].shows.get(),
            0,
            "Pending must not prime native HWND with show/hide"
        );
        drain(22);
        assert_eq!(adapters.borrow()[0].shows.get(), 1);
        let old = SELECTION_TOOLBAR_REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .clone_strong()
        });
        old.invoke_copy_requested();
        assert_eq!(
            calls.get(),
            0,
            "Core delivery must wait until native callback returns"
        );
        SELECTION_TOOLBAR_REGISTRY.with(|s| s.borrow_mut().as_mut().unwrap().close());
        sync();
        drain(0);
        old.invoke_copy_requested();
        old.invoke_translate_requested();
        drain(1);
        assert_eq!(calls.get(), 0);
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .invoke_copy_requested()
        });
        drain(0);
        assert_eq!(calls.get(), 1);
        SELECTION_TOOLBAR_REGISTRY.with(|s| s.borrow_mut().as_mut().unwrap().close());
    }
    #[test]
    fn first_visible_frame_waits_for_final_dpi_placement_and_no_space_never_shows() {
        let adapters = Rc::new(RefCell::new(Vec::new()));
        install(
            |_| PassiveWindowPreparation::Ready,
            adapters.clone(),
            Rc::new(Cell::new(0)),
        );
        let area = Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.work_area = Rc::new(move |_| Some(area));
            r.sync(
                Some(Selection {
                    text: "fixture".into(),
                    anchor: Some(Point { x: 100, y: 100 }),
                    gesture_start: Some(Point { x: 400, y: 150 }),
                    bounds: vec![Rect {
                        left: 100,
                        top: 100,
                        right: 400,
                        bottom: 150,
                    }],
                }),
                None,
            );
        });
        drain(0);
        assert_eq!(
            adapters.borrow()[0].shows.get(),
            0,
            "positioning must stay hidden"
        );
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: 1.25,
                })
        });
        drain(18);
        assert_eq!(
            adapters.borrow()[0].shows.get(),
            0,
            "DPI change must invalidate the prepared placement"
        );
        drain(18);
        assert_eq!(adapters.borrow()[0].shows.get(), 1);
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.sync(
                Some(Selection {
                    text: "full screen fixture".into(),
                    anchor: Some(Point { x: 100, y: 100 }),
                    gesture_start: None,
                    bounds: vec![area],
                }),
                None,
            );
        });
        drain(0);
        assert_eq!(adapters.borrow()[1].shows.get(), 0);
        assert!(SELECTION_TOOLBAR_REGISTRY.with(|s| s.borrow().as_ref().unwrap().window.is_none()));
    }
    #[test]
    fn changed_target_and_exhausted_preparation_destroy_without_showing() {
        let adapters = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(Cell::new(0));
        install(
            |_| PassiveWindowPreparation::Pending,
            adapters.clone(),
            calls.clone(),
        );
        sync();
        drain(355);
        assert!(SELECTION_TOOLBAR_REGISTRY.with(|s| s.borrow().as_ref().unwrap().window.is_none()));
        assert_eq!(adapters.borrow()[0].shows.get(), 0);
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.prepare = |_| PassiveWindowPreparation::Ready;
            r.target_current = Rc::new(|_| false);
        });
        use lexift_core::ports::magnifier::WindowToken;
        SELECTION_TOOLBAR_REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().sync(
                Some(Selection {
                    bounds: Vec::new(),
                    gesture_start: None,
                    text: "fixture".into(),
                    anchor: None,
                }),
                Some(SelectionTarget {
                    window: WindowToken(1),
                    focus: None,
                    process: 2,
                    thread: 3,
                }),
            )
        });
        drain(1);
        assert_eq!(
            adapters.borrow().len(),
            1,
            "stale target must not even create a new toolbar"
        );
        assert_eq!(calls.get(), 0);
    }
}
