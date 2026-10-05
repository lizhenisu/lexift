//! Ownership and cancellable preparation for a single transient annotation component.
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::{Rc, Weak},
    time::Duration,
};

/// FIFO input work belongs to one transient window, independently of Registry borrows.
/// Retirement also disables callbacks from externally retained Slint components.
pub(super) struct InputWork {
    active: Cell<bool>,
    pointer_gate: PointerGate,
    timer: slint::Timer,
    pending: RefCell<Vec<Box<dyn FnOnce()>>>,
    pointer_routes: RefCell<Vec<Weak<PointerRoute>>>,
}
impl InputWork {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            active: Cell::new(true),
            pointer_gate: PointerGate::default(),
            timer: slint::Timer::default(),
            pending: RefCell::new(Vec::new()),
            pointer_routes: RefCell::new(Vec::new()),
        })
    }
    /// Attach a typed component receiver to this instance's existing FIFO queue.
    /// Retirement cancels both queued delivery and callbacks from retained components.
    pub fn receiver<T: 'static>(self: &Rc<Self>, receive: Rc<dyn Fn(T)>) -> Rc<dyn Fn(T)> {
        let input = self.clone();
        Rc::new(move |event| {
            let receive = receive.clone();
            input.post(move || receive(event));
        })
    }

    /// Native hooks may outlive a retired component. Reject their events before
    /// callbacks inspect session state or dispatch into a retained Slint window.
    pub fn pointer_sink(
        self: &Rc<Self>,
        receive: impl Fn(crate::bridge::PopupPointerInput) + 'static,
    ) -> crate::bridge::PopupPointerSink {
        let work = self.clone();
        Rc::new(move |input| {
            if work.active.get() && work.pointer_gate.allows() {
                receive(input);
            }
        })
    }

    /// Preserve native event order across temporary receiver unavailability.
    /// Normal delivery is immediate; queued events retire with this window.
    pub fn pointer_sink_when_ready(
        self: &Rc<Self>,
        ready: Rc<dyn Fn() -> bool>,
        receive: crate::bridge::PopupPointerSink,
    ) -> crate::bridge::PopupPointerSink {
        let route = Rc::new(PointerRoute {
            work: self.clone(),
            ready,
            receive,
            queued: RefCell::new(VecDeque::new()),
            scheduled: Cell::new(false),
        });
        let mut routes = self.pointer_routes.borrow_mut();
        routes.retain(|route| route.strong_count() != 0);
        routes.push(Rc::downgrade(&route));
        drop(routes);
        self.pointer_sink(move |event| {
            if route.scheduled.get() || !(route.ready)() {
                route.enqueue(event);
            } else {
                (route.receive)(event);
            }
        })
    }

    /// Native child teardown may synchronously send input to a surviving owner.
    /// Pause only pointer delivery for that scope; retain queued component work.
    pub fn pause_pointer(self: &Rc<Self>) -> PointerPause {
        self.pointer_gate.pause()
    }

    pub fn post(self: &Rc<Self>, action: impl FnOnce() + 'static) {
        if !self.active.get() {
            return;
        }
        self.pending.borrow_mut().push(Box::new(action));
        if self.timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                let Some(work) = weak.upgrade() else { return };
                let batch = std::mem::take(&mut *work.pending.borrow_mut());
                for action in batch {
                    if !work.active.get() {
                        break;
                    }
                    action();
                }
            });
    }
    /// Keep the existing two event-loop turns so posted parameter updates precede settlement.
    pub fn after_inputs(self: &Rc<Self>, action: impl FnOnce() + 'static) {
        let weak = Rc::downgrade(self);
        self.post(move || {
            if let Some(work) = weak.upgrade() {
                work.post(action);
            }
        });
    }
    #[cfg(test)]
    pub fn retired(&self) -> bool {
        !self.active.get()
    }
    #[cfg(test)]
    pub fn pending(&self) -> bool {
        self.timer.running() || !self.pending.borrow().is_empty()
    }

    pub fn retire(&self) {
        self.active.set(false);
        self.timer.stop();
        self.pending.borrow_mut().clear();
        for route in self
            .pointer_routes
            .borrow_mut()
            .drain(..)
            .filter_map(|route| route.upgrade())
        {
            route.cancel();
        }
    }
}

/// A queued native event must not be overtaken by a later press or release.
struct PointerRoute {
    work: Rc<InputWork>,
    ready: Rc<dyn Fn() -> bool>,
    receive: crate::bridge::PopupPointerSink,
    queued: RefCell<VecDeque<crate::bridge::PopupPointerInput>>,
    scheduled: Cell<bool>,
}

impl PointerRoute {
    fn enqueue(self: &Rc<Self>, event: crate::bridge::PopupPointerInput) {
        self.queued.borrow_mut().push_back(event);
        self.schedule();
    }

    fn schedule(self: &Rc<Self>) {
        if self.scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        self.work.post(move || {
            let Some(route) = weak.upgrade() else { return };
            // Bound this turn; reentrant events appended by the receiver stay
            // behind the existing head and run in a later event-loop turn.
            let budget = route.queued.borrow().len();
            for _ in 0..budget {
                if !route.work.active.get() || !route.work.pointer_gate.allows() {
                    route.cancel();
                    return;
                }
                if !(route.ready)() {
                    break;
                }
                let Some(event) = route.queued.borrow_mut().pop_front() else {
                    break;
                };
                (route.receive)(event);
            }
            route.scheduled.set(false);
            if !route.queued.borrow().is_empty() {
                route.schedule();
            }
        });
    }

    fn cancel(&self) {
        self.queued.borrow_mut().clear();
        self.scheduled.set(false);
    }
}

/// A pointer-only gate shared by transient components and text editors.
/// Pausing does not discard queued work or change the instance lifetime.
#[derive(Default)]
pub(super) struct PointerGate(Rc<Cell<usize>>);
impl PointerGate {
    pub fn allows(&self) -> bool {
        self.0.get() == 0
    }
    pub fn pause(&self) -> PointerPause {
        self.0.set(
            self.0
                .get()
                .checked_add(1)
                .expect("pointer pause depth exhausted"),
        );
        PointerPause(self.0.clone())
    }
}
/// Nestable scope that never reactivates an input instance retired while paused.
pub(super) struct PointerPause(Rc<Cell<usize>>);
impl Drop for PointerPause {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Drop for InputWork {
    fn drop(&mut self) {
        self.retire();
    }
}

type Retirement = Box<dyn Fn(&slint::Window)>;

thread_local! { static NEXT_WINDOW: Cell<u64> = const { Cell::new(0) }; }

pub(super) fn next_revision() -> u64 {
    NEXT_WINDOW.with(|next| {
        let revision = next
            .get()
            .checked_add(1)
            .expect("annotation window identity exhausted");
        next.set(revision);
        revision
    })
}

pub(super) struct Slot<C: ComponentHandle> {
    current: Option<C>,
    input: Rc<InputWork>,
    revision: u64,
    preparation: slint::Timer,
    placement: slint::Timer,
    retirement: Option<Retirement>,
}

impl<C: ComponentHandle> Default for Slot<C> {
    fn default() -> Self {
        Self {
            current: None,
            input: InputWork::new(),
            revision: 0,
            preparation: slint::Timer::default(),
            placement: slint::Timer::default(),
            retirement: None,
        }
    }
}

impl<C: ComponentHandle> Slot<C> {
    pub fn get(&self) -> Option<&C> {
        self.current.as_ref()
    }
    pub fn input(&self) -> Rc<InputWork> {
        self.input.clone()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn open(&mut self, component: C) -> u64 {
        self.clear();
        self.input = InputWork::new();
        self.current = Some(component);
        self.revision
    }

    /// Window-specific hooks retire with the component, including direct owner destruction.
    pub fn open_with_retirement(
        &mut self,
        component: C,
        retirement: impl Fn(&slint::Window) + 'static,
    ) -> u64 {
        let revision = self.open(component);
        self.retirement = Some(Box::new(retirement));
        revision
    }

    pub fn prepare_after(&self, delay: Duration, prepare: impl FnMut() + 'static) {
        if self.current.is_some() {
            self.preparation
                .start(slint::TimerMode::SingleShot, delay, prepare);
        }
    }

    /// Disable delivery before any session-wide native resource teardown begins.
    pub fn retire_input(&self) {
        self.input.retire();
        self.preparation.stop();
        self.placement.stop();
    }

    pub fn clear(&mut self) {
        self.retire_input();
        self.revision = next_revision();
        if let Some(component) = self.current.take() {
            if let Some(retirement) = self.retirement.take() {
                retirement(component.window());
            }
            let _ = component.hide();
        }
    }

    pub fn position_after(&self, delay: Duration, position: impl FnMut() + 'static) {
        if self.current.is_some() {
            self.placement
                .start(slint::TimerMode::SingleShot, delay, position);
        }
    }
}

impl<C: ComponentHandle> Drop for Slot<C> {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnnotationPanel, AnnotationWatermarkInput};
    use std::rc::Rc;

    #[test]
    fn pointer_retry_cannot_move_press_behind_release_when_readiness_changes() {
        use crate::bridge::PopupPointerInput;
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        };
        let phase = Rc::new(Cell::new(0));
        let readiness = phase.clone();
        let events = Rc::new(RefCell::new(Vec::new()));
        let received = events.clone();
        let work = InputWork::new();
        let sink = work.pointer_sink_when_ready(
            Rc::new(move || match readiness.get() {
                0 => false,
                1 => {
                    readiness.set(2);
                    false
                }
                _ => true,
            }),
            Rc::new(move |event| {
                received.borrow_mut().push(match event {
                    PopupPointerInput::LeftPressed { .. } => "down",
                    PopupPointerInput::LeftReleased { .. } => "up",
                    _ => unreachable!(),
                })
            }),
        );
        sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
        sink(PopupPointerInput::LeftReleased { x: 0., y: 0. });
        phase.set(1);
        advance();
        assert!(
            events.borrow().is_empty(),
            "release must not overtake a deferred press"
        );
        advance();
        assert_eq!(*events.borrow(), ["down", "up"]);
        phase.set(0);
        sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
        work.retire();
        phase.set(2);
        advance();
        assert_eq!(*events.borrow(), ["down", "up"], "retirement cancels retry");
    }

    #[test]
    fn window_input_is_retired_even_while_the_old_component_is_retained() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        };
        let mut owner = Slot::default();
        let window = AnnotationWatermarkInput::new().unwrap();
        let retained = window.clone_strong();
        let weak_window = window.as_weak();
        owner.open(window);
        let input = owner.input();
        let weak_input = Rc::downgrade(&input);
        let values = Rc::new(RefCell::new(Vec::new()));
        let callback_input = input.clone();
        let received = values.clone();
        retained.on_apply(move |value| {
            let received = received.clone();
            callback_input.post(move || received.borrow_mut().push(value.to_string()));
        });
        retained.invoke_apply("active".into());
        advance();
        assert_eq!(*values.borrow(), vec!["active"]);
        retained.invoke_apply("queued before close".into());
        assert!(input.pending());
        owner.clear();
        assert!(input.retired() && !input.pending());
        owner.open(AnnotationWatermarkInput::new().unwrap());
        retained.invoke_apply("retired".into());
        assert!(!input.pending());
        advance();
        assert_eq!(*values.borrow(), vec!["active"]);
        drop(retained);
        drop(input);
        assert!(weak_window.upgrade().is_none());
        assert!(
            weak_input.upgrade().is_none(),
            "queue and callbacks must not form a cycle"
        );
        let current = owner.input();
        current.post(|| panic!("destroyed owner must cancel pending input"));
        drop(owner);
        assert!(current.retired() && !current.pending());
        current.post(|| panic!("retired input cannot schedule new work"));
        advance();
    }

    #[test]
    fn pointer_pause_preserves_queued_work_and_cannot_revive_retired_input() {
        let input = InputWork::new();
        let delivered = Rc::new(Cell::new(0));
        let observed = delivered.clone();
        let sink = input.pointer_sink(move |_| observed.set(observed.get() + 1));
        let event = crate::bridge::PopupPointerInput::LeftPressed { x: 0., y: 0. };
        input.post(|| {});
        let outer = input.pause_pointer();
        let inner = input.pause_pointer();
        sink(event);
        drop(inner);
        sink(event);
        assert_eq!(delivered.get(), 0);
        assert!(input.pending());
        drop(outer);
        sink(event);
        assert_eq!(delivered.get(), 1);
        assert!(input.pending());
        let paused = input.pause_pointer();
        input.retire();
        drop(paused);
        sink(event);
        assert_eq!(delivered.get(), 1);
        assert!(input.retired() && !input.pending());
    }

    #[test]
    fn native_pointer_delivery_follows_slot_retirement_not_component_liveness() {
        use crate::{
            AnnotationTextInput,
            bridge::{PopupPointerInput, dispatch_pointer_input},
        };
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for dpi in [1., 1.25] {
            let window = AnnotationTextInput::new().unwrap();
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
            window.window().set_size(slint::LogicalSize::new(200., 80.));
            window.show().unwrap();
            let calls = Rc::new(Cell::new(0));
            let observed = calls.clone();
            window.on_pointer(move |_, _, _| observed.set(observed.get() + 1));
            let mut slot = Slot::default();
            slot.open(window.clone_strong());
            let input = slot.input();
            let weak_input = Rc::downgrade(&input);
            let weak_window = window.as_weak();
            let sink = input.pointer_sink(move |event| {
                if let Some(window) = weak_window.upgrade() {
                    dispatch_pointer_input(window.window(), event);
                }
            });
            let click = |sink: &crate::bridge::PopupPointerSink| {
                for event in [
                    PopupPointerInput::Moved {
                        x: 20. * dpi,
                        y: 20. * dpi,
                    },
                    PopupPointerInput::LeftPressed {
                        x: 20. * dpi,
                        y: 20. * dpi,
                    },
                    PopupPointerInput::LeftReleased {
                        x: 20. * dpi,
                        y: 20. * dpi,
                    },
                ] {
                    sink(event);
                }
            };
            click(&sink);
            assert!(calls.get() > 0);
            let before = calls.get();
            slot.retire_input();
            assert!(
                slot.get().is_some(),
                "input retires before native destruction"
            );
            click(&sink);
            sink(PopupPointerInput::LeftCancelled {
                x: 20. * dpi,
                y: 20. * dpi,
            });
            sink(PopupPointerInput::DismissRequested {
                cause: crate::bridge::PopupDismissCause::ForegroundChanged,
            });
            assert_eq!(calls.get(), before);
            slot.open(window.clone_strong());
            window.show().unwrap();
            let weak_window = window.as_weak();
            let next_sink = slot.input().pointer_sink(move |event| {
                if let Some(window) = weak_window.upgrade() {
                    dispatch_pointer_input(window.window(), event);
                }
            });
            click(&next_sink);
            assert!(calls.get() > before);
            let before = calls.get();
            click(&sink);
            assert_eq!(
                calls.get(),
                before,
                "replacement does not reactivate old input"
            );
            drop(slot);
            click(&next_sink);
            assert_eq!(
                calls.get(),
                before,
                "direct owner destruction retires the hook"
            );
            drop(input);
            assert!(
                weak_input.upgrade().is_some(),
                "retained native hook owns its retired gate"
            );
            drop(sink);
            assert!(
                weak_input.upgrade().is_none(),
                "hook release frees the gate without a cycle"
            );
        }
    }

    #[test]
    fn window_preparation_is_cancelled_and_identity_is_not_reused_by_new_owners() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let component = AnnotationWatermarkInput::new().unwrap();
        let old = component.as_weak();
        let mut slot = Slot::default();
        let first = slot.open(component);
        slot.prepare_after(Duration::from_millis(16), || {
            panic!("closed window must not prepare")
        });
        slot.position_after(Duration::from_millis(32), || {
            panic!("closed window must not reposition")
        });
        slot.clear();
        assert!(old.upgrade().is_none());
        assert!(slot.revision() > first);
        let component = AnnotationWatermarkInput::new().unwrap();
        let replaced = component.as_weak();
        let second = slot.open(component);
        slot.prepare_after(Duration::from_millis(16), || {
            panic!("replaced window must not prepare")
        });
        let component = AnnotationWatermarkInput::new().unwrap();
        let last = component.as_weak();
        assert!(slot.open(component) > second);
        assert!(replaced.upgrade().is_none());
        slot.prepare_after(Duration::from_millis(16), || {
            panic!("destroyed owner must not prepare")
        });
        slot.position_after(Duration::from_millis(32), || {
            panic!("destroyed owner must not reposition")
        });
        drop(slot);
        assert!(last.upgrade().is_none());
        let mut new_owner = Slot::default();
        assert!(new_owner.open(AnnotationWatermarkInput::new().unwrap()) > second);
        let retired = Rc::new(Cell::new(0));
        let mut panel_owner = Slot::default();
        let panel = AnnotationPanel::new().unwrap();
        let held_panel = panel.clone_strong();
        let calls = retired.clone();
        panel_owner.open_with_retirement(panel, move |_| calls.set(calls.get() + 1));
        panel_owner.prepare_after(Duration::from_millis(16), || {
            panic!("retired panel must not prepare")
        });
        // Native hook cleanup cannot rely on the last component handle disappearing.
        panel_owner.clear();
        assert_eq!(retired.get(), 1);
        drop(held_panel);
        let calls = retired.clone();
        panel_owner.open_with_retirement(AnnotationPanel::new().unwrap(), move |_| {
            calls.set(calls.get() + 1)
        });
        panel_owner.prepare_after(Duration::from_millis(16), || {
            panic!("destroyed panel owner must not prepare")
        });
        drop(panel_owner);
        assert_eq!(retired.get(), 2);
        clock.set(Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
    }
}
