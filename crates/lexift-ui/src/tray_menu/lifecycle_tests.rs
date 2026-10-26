//! Drive queued callbacks and preparation timers without wall-clock sleeps.
use super::*;
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use std::{
    cell::Cell,
    sync::{Arc, Mutex},
};
type Queue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;
struct Proxy(Queue);
impl slint::platform::EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        Ok(())
    }
    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), slint::EventLoopError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}
struct Backend {
    queue: Queue,
    clock: Rc<Cell<u64>>,
}
impl Platform for Backend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        Some(Box::new(Proxy(self.queue.clone())))
    }
    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(self.clock.get())
    }
}
thread_local! {static PREP:Cell<u8>=const {Cell::new(0)};static CALLS:Cell<u32>=const {Cell::new(0)};}
fn prepare(_: &slint::Window) -> PassiveWindowPreparation {
    CALLS.set(CALLS.get() + 1);
    match PREP.get() {
        1 => PassiveWindowPreparation::Pending,
        2 => PassiveWindowPreparation::Failed,
        _ => PassiveWindowPreparation::Ready,
    }
}
fn lifecycle() -> WindowLifecycleCallbacks {
    WindowLifecycleCallbacks::new(
        |_, _| true,
        |_| true,
        |_| true,
        |_, _, _| true,
        |_, _| true,
        |_, _| true,
        || true,
    )
}
fn setup() -> (Queue, Rc<Cell<u64>>, Rc<Cell<u32>>) {
    let queue = Queue::default();
    let clock = Rc::new(Cell::new(0));
    slint::platform::set_platform(Box::new(Backend {
        queue: queue.clone(),
        clock: clock.clone(),
    }))
    .unwrap();
    init(prepare, lifecycle());
    let actions = Rc::new(Cell::new(0));
    let a = actions.clone();
    set_handler(Rc::new(move |_| a.set(a.get() + 1)));
    (queue, clock, actions)
}
fn drain(queue: &Queue) {
    loop {
        let events = std::mem::take(&mut *queue.lock().unwrap());
        if events.is_empty() {
            break;
        }
        for event in events {
            event();
        }
    }
}
fn request(request_id: u64) -> TrayMenuRequest {
    TrayMenuRequest {
        request_id,
        anchor: Default::default(),
        keyboard: true,
    }
}
fn key() -> Opening {
    REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref().unwrap();
        Opening {
            generation: r.generation,
            request_id: r.request.unwrap().request_id,
        }
    })
}
/// Slint installs one process-wide event-loop proxy. Isolate each scheduler fixture
/// so it cannot replace the proxy of unrelated UI tests running in parallel.
fn isolated(name: &str, run: impl FnOnce()) {
    if std::env::var("LEXIFT_TRAY_LIFECYCLE_CASE").ok().as_deref() == Some(name) {
        run();
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("tray_menu::lifecycle_tests::{name}"),
            "--nocapture",
        ])
        .env("LEXIFT_TRAY_LIFECYCLE_CASE", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn duplicate_and_retired_requests_do_not_recreate_windows() {
    isolated(
        "duplicate_and_retired_requests_do_not_recreate_windows",
        || {
            let (_q, _clock, _actions) = setup();
            show(request(10));
            let first = key();
            show(request(10));
            show(request(9));
            assert_eq!(key(), first);
            close(false);
            show(request(10));
            show(request(0));
            assert!(!is_open());
            show(request(11));
            assert_eq!(key().request_id, 11);
            close(false);
        },
    );
}
#[test]
fn queued_action_and_dismissal_execute_at_most_once() {
    isolated("queued_action_and_dismissal_execute_at_most_once", || {
        let (queue, _clock, actions) = setup();
        show(request(1));
        let owner = key();
        finish_later(owner, Some(AppEvent::MainWindowRequested), false, "action");
        finish_later(
            owner,
            Some(AppEvent::SettingsWindowRequested),
            false,
            "duplicate_action",
        );
        finish_later(owner, None, true, "explicit_cancel");
        drain(&queue);
        assert_eq!(actions.get(), 1);
        assert!(!is_open());
        show(request(1));
        assert!(!is_open());
    });
}
#[test]
fn old_action_and_focus_callbacks_cannot_close_replacement() {
    isolated(
        "old_action_and_focus_callbacks_cannot_close_replacement",
        || {
            let (queue, _clock, actions) = setup();
            show(request(1));
            let old = key();
            let weak = REGISTRY.with(|s| {
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .window
                    .as_ref()
                    .unwrap()
                    .as_weak()
            });
            finish_later(old, Some(AppEvent::MainWindowRequested), false, "action");
            finish_later(old, None, false, "focus_lost");
            show(request(2));
            assert!(weak.upgrade().is_none());
            drain(&queue);
            assert!(is_open());
            assert_eq!(key().request_id, 2);
            assert_eq!(actions.get(), 0);
            close(false);
        },
    );
}
#[test]
fn request_identity_survives_registry_reinitialization() {
    isolated(
        "request_identity_survives_registry_reinitialization",
        || {
            let (queue, _clock, actions) = setup();
            show(request(1));
            let old = key();
            finish_later(old, None, false, "native_dismissal");
            init(prepare, lifecycle());
            show(request(2));
            assert_eq!(old.generation, key().generation);
            drain(&queue);
            assert!(is_open());
            assert_eq!(actions.get(), 0);
            close(false);
        },
    );
}
#[test]
fn preparation_retries_are_bounded_and_destroy_unshown_window() {
    isolated(
        "preparation_retries_are_bounded_and_destroy_unshown_window",
        || {
            let (queue, clock, _actions) = setup();
            PREP.set(1);
            show(request(1));
            for _ in 0..12 {
                clock.set(clock.get() + 20);
                slint::platform::update_timers_and_animations();
                drain(&queue);
            }
            assert_eq!(CALLS.get(), 9);
            assert!(!is_open());
        },
    );
}
#[test]
fn delayed_preparation_failure_does_not_retire_new_opening() {
    isolated(
        "delayed_preparation_failure_does_not_retire_new_opening",
        || {
            let (queue, clock, actions) = setup();
            PREP.set(2);
            show(request(1));
            clock.set(1);
            slint::platform::update_timers_and_animations();
            PREP.set(0);
            show(request(2));
            drain(&queue);
            assert!(is_open());
            assert_eq!(key().request_id, 2);
            assert_eq!(actions.get(), 0);
            close(false);
        },
    );
}
#[test]
fn failed_activation_or_configuration_finishes_without_action() {
    isolated(
        "failed_activation_or_configuration_finishes_without_action",
        || {
            let (queue, clock, actions) = setup();
            for stage in 0..4 {
                let mut callbacks = lifecycle();
                match stage {
                    0 => callbacks.configure_menu_window = Rc::new(|_| false),
                    1 => callbacks.complete_passive_window_show = Rc::new(|_, _| false),
                    2 => callbacks.activate_user_requested_window = |_| false,
                    _ => callbacks.set_popup_dismissal = Rc::new(|_, _| false),
                }
                init(prepare, callbacks);
                show(request(stage + 1));
                clock.set(clock.get() + 1);
                slint::platform::update_timers_and_animations();
                drain(&queue);
                assert!(!is_open());
            }
            assert_eq!(actions.get(), 0);
        },
    );
}
