use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use lexift_core::domain::geometry::Point;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentProcessId,
    UI::{
        Input::KeyboardAndMouse::GetDoubleClickTime,
        WindowsAndMessaging::{
            CallNextHookEx, GetSystemMetrics, GetWindowThreadProcessId, HC_ACTION, MSLLHOOKSTRUCT,
            SM_CXDOUBLECLK, SM_CYDOUBLECLK, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WindowFromPoint,
        },
    },
};

use super::hook_thread::HookMessageThread;

const DRAG_DISTANCE_SQUARED: i64 = 36;

fn is_drag(start: Point, end: Point) -> bool {
    let dx = i64::from(end.x) - i64::from(start.x);
    let dy = i64::from(end.y) - i64::from(start.y);
    dx * dx + dy * dy >= DRAG_DISTANCE_SQUARED
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Started,
    Completed { start: Point, end: Point },
}

#[derive(Clone, Copy)]
struct Click {
    point: Point,
    time: u32,
    window: usize,
}

#[derive(Clone, Copy)]
struct Press {
    click: Click,
    second_click: bool,
}

#[derive(Clone, Copy)]
struct DoubleClickSettings {
    interval_ms: u32,
    width: i32,
    height: i32,
}

impl DoubleClickSettings {
    fn current() -> Self {
        Self {
            interval_ms: unsafe { GetDoubleClickTime() },
            width: unsafe { GetSystemMetrics(SM_CXDOUBLECLK) },
            height: unsafe { GetSystemMetrics(SM_CYDOUBLECLK) },
        }
    }

    fn matches(self, first: Click, second: Click) -> bool {
        let dx = (i64::from(first.point.x) - i64::from(second.point.x)).abs();
        let dy = (i64::from(first.point.y) - i64::from(second.point.y)).abs();
        first.window != 0
            && first.window == second.window
            && second.time.wrapping_sub(first.time) <= self.interval_ms
            && dx * 2 <= i64::from(self.width)
            && dy * 2 <= i64::from(self.height)
    }
}

#[derive(Default)]
struct GestureTracker {
    press: Option<Press>,
    last_click: Option<Click>,
}

impl GestureTracker {
    fn down(
        &mut self,
        point: Point,
        time: u32,
        window: usize,
        own_window: bool,
        double_click: DoubleClickSettings,
    ) -> Option<Gesture> {
        if own_window {
            self.press = None;
            self.last_click = None;
            return None;
        }
        let click = Click {
            point,
            time,
            window,
        };
        let second_click = self
            .last_click
            .take()
            .is_some_and(|previous| double_click.matches(previous, click));
        self.press = Some(Press {
            click,
            second_click,
        });
        Some(Gesture::Started)
    }

    fn up(&mut self, point: Point) -> Option<Gesture> {
        let press = self.press.take()?;
        if is_drag(press.click.point, point) {
            self.last_click = None;
            return Some(Gesture::Completed {
                start: press.click.point,
                end: point,
            });
        }
        if press.second_click {
            self.last_click = None;
            return Some(Gesture::Completed {
                start: press.click.point,
                end: point,
            });
        }
        self.last_click = Some(press.click);
        None
    }
}

struct Monitor {
    sender: mpsc::Sender<PointerSample>,
    max_callback_us: Arc<AtomicU64>,
}

/// Copy-only hook payload; window lookup and gesture state belong to dispatch.
#[derive(Clone, Copy)]
struct PointerSample {
    down: bool,
    point: Point,
    time: u32,
    sent_at: Instant,
}

fn resolve_window(point: Point) -> (usize, bool, DoubleClickSettings) {
    let window = unsafe {
        WindowFromPoint(windows::Win32::Foundation::POINT {
            x: point.x,
            y: point.y,
        })
    };
    let mut process_id = 0;
    if !window.0.is_null() {
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    }
    (
        window.0 as usize,
        process_id == unsafe { GetCurrentProcessId() },
        DoubleClickSettings::current(),
    )
}

fn process_sample(
    tracker: &mut GestureTracker,
    sample: PointerSample,
    resolve: impl FnOnce(Point) -> (usize, bool, DoubleClickSettings),
) -> Option<Gesture> {
    // After a long downstream stall, the window under an old point is no longer
    // a reliable click target. Drop both halves rather than seed a phantom drag.
    if sample.sent_at.elapsed() > Duration::from_millis(500) {
        *tracker = GestureTracker::default();
        return None;
    }
    if sample.down {
        let (window, own_window, settings) = resolve(sample.point);
        tracker.down(sample.point, sample.time, window, own_window, settings)
    } else {
        tracker.up(sample.point)
    }
}

thread_local! {
    static MONITOR: RefCell<Option<Monitor>> = const { RefCell::new(None) };
}

struct MonitorHandle {
    hook_thread: HookMessageThread,
    dispatcher: JoinHandle<()>,
    sender: mpsc::Sender<PointerSample>,
    stopped: Arc<AtomicBool>,
    max_callback_us: Arc<AtomicU64>,
    max_dispatch_delay_us: Arc<AtomicU64>,
}

static ACTIVE_MONITOR: OnceLock<Mutex<Option<MonitorHandle>>> = OnceLock::new();

fn active_monitor() -> &'static Mutex<Option<MonitorHandle>> {
    ACTIVE_MONITOR.get_or_init(|| Mutex::new(None))
}

pub fn start(handler: Arc<dyn Fn(Gesture) + Send + Sync>) -> lexift_core::Result<()> {
    let mut active = active_monitor()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if active.is_some() {
        return Ok(());
    }
    let (sender, receiver) = mpsc::channel::<PointerSample>();
    let stopped = Arc::new(AtomicBool::new(false));
    let dispatch_stopped = stopped.clone();
    let max_dispatch_delay_us = Arc::new(AtomicU64::new(0));
    let dispatch_delay = Arc::clone(&max_dispatch_delay_us);
    let dispatcher = thread::Builder::new()
        .name("lexift-selection-dispatch".into())
        .spawn(move || {
            let mut tracker = GestureTracker::default();
            while let Ok(sample) = receiver.recv() {
                if dispatch_stopped.load(Ordering::Acquire) {
                    break;
                }
                dispatch_delay.fetch_max(
                    sample.sent_at.elapsed().as_micros() as u64,
                    Ordering::Relaxed,
                );
                if let Some(gesture) = process_sample(&mut tracker, sample, resolve_window)
                    && !dispatch_stopped.load(Ordering::Acquire)
                {
                    handler(gesture);
                }
            }
        })
        .map_err(|error| {
            lexift_core::Error::new(format!("Could not dispatch selection gestures: {error}"))
        })?;
    let max_callback_us = Arc::new(AtomicU64::new(0));
    let hook_sender = sender.clone();
    let hook_max = Arc::clone(&max_callback_us);
    let hook_thread = match HookMessageThread::start("lexift-selection-hook", move || {
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) }.map_err(
            |error| lexift_core::Error::new(format!("Could not monitor text selection: {error}")),
        )?;
        MONITOR.with(|slot| {
            *slot.borrow_mut() = Some(Monitor {
                sender: hook_sender,
                max_callback_us: hook_max,
            });
        });
        Ok(Box::new(move || {
            MONITOR.with(|slot| *slot.borrow_mut() = None);
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }))
    }) {
        Ok(thread) => thread,
        Err(error) => {
            drop(sender);
            let _ = dispatcher.join();
            return Err(error);
        }
    };
    *active = Some(MonitorHandle {
        hook_thread,
        dispatcher,
        sender,
        stopped,
        max_callback_us,
        max_dispatch_delay_us,
    });
    Ok(())
}

pub fn stop() {
    let monitor = active_monitor()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(monitor) = monitor {
        monitor.stopped.store(true, Ordering::Release);
        monitor.hook_thread.stop();
        drop(monitor.sender);
        let _ = monitor.dispatcher.join();
        tracing::info!(
            max_callback_us = monitor.max_callback_us.load(Ordering::Relaxed),
            max_dispatch_delay_us = monitor.max_dispatch_delay_us.load(Ordering::Relaxed),
            "selection mouse hook stopped"
        );
    }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let started = Instant::now();
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        let event = wparam.0 as u32;
        if event == WM_LBUTTONDOWN || event == WM_LBUTTONUP {
            let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let point = Point {
                x: data.pt.x,
                y: data.pt.y,
            };
            MONITOR.with(|slot| {
                let slot = slot.borrow();
                let Some(monitor) = slot.as_ref() else { return };
                let _ = monitor.sender.send(PointerSample {
                    down: event == WM_LBUTTONDOWN,
                    point,
                    time: data.time,
                    sent_at: started,
                });
            });
        }
    }
    MONITOR.with(|slot| {
        if let Some(monitor) = slot.borrow().as_ref() {
            monitor
                .max_callback_us
                .fetch_max(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
    });
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{DoubleClickSettings, Gesture, GestureTracker, is_drag, start, stop};
    use lexift_core::domain::geometry::Point;
    use std::sync::Arc;

    #[test]
    #[ignore = "installs a global mouse hook on an interactive Windows desktop"]
    fn dedicated_hook_can_stop_and_restart() {
        for _ in 0..2 {
            start(Arc::new(|_| {})).unwrap();
            stop();
        }
    }

    const SETTINGS: DoubleClickSettings = DoubleClickSettings {
        interval_ms: 500,
        width: 8,
        height: 8,
    };

    fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    #[test]
    fn short_pointer_movements_are_not_drags() {
        let start = Point { x: -10, y: 20 };
        assert!(!is_drag(start, Point { x: -5, y: 23 }));
        assert!(is_drag(start, Point { x: -4, y: 20 }));
    }

    #[test]
    fn double_click_completes_only_on_second_release_at_system_boundaries() {
        let mut tracker = GestureTracker::default();
        assert_eq!(
            tracker.down(point(-100, 20), 100, 1, false, SETTINGS),
            Some(Gesture::Started)
        );
        assert_eq!(tracker.up(point(-100, 20)), None);
        assert_eq!(
            tracker.down(point(-96, 24), 600, 1, false, SETTINGS),
            Some(Gesture::Started)
        );
        assert_eq!(
            tracker.up(point(-96, 24)),
            Some(Gesture::Completed {
                start: point(-96, 24),
                end: point(-96, 24)
            })
        );
        assert_eq!(
            tracker.down(point(-96, 24), 700, 1, false, SETTINGS),
            Some(Gesture::Started)
        );
        assert_eq!(tracker.up(point(-96, 24)), None);
    }

    #[test]
    fn double_click_rejects_time_distance_and_window_mismatches() {
        for (second, time, window) in [
            (point(10, 10), 601, 1),
            (point(15, 10), 200, 1),
            (point(10, 15), 200, 1),
            (point(10, 10), 200, 2),
        ] {
            let mut tracker = GestureTracker::default();
            tracker.down(point(10, 10), 100, 1, false, SETTINGS);
            assert_eq!(tracker.up(point(10, 10)), None);
            tracker.down(second, time, window, false, SETTINGS);
            assert_eq!(tracker.up(second), None);
        }
    }

    #[test]
    fn drag_does_not_seed_a_double_click_and_still_completes() {
        let mut tracker = GestureTracker::default();
        tracker.down(point(10, 10), 100, 1, false, SETTINGS);
        assert_eq!(
            tracker.up(point(16, 10)),
            Some(Gesture::Completed {
                start: point(10, 10),
                end: point(16, 10)
            })
        );
        tracker.down(point(10, 10), 200, 1, false, SETTINGS);
        assert_eq!(tracker.up(point(10, 10)), None);
    }

    #[test]
    fn lexift_click_cancels_pending_double_click() {
        let mut tracker = GestureTracker::default();
        tracker.down(point(10, 10), 100, 1, false, SETTINGS);
        tracker.up(point(10, 10));
        assert_eq!(tracker.down(point(10, 10), 200, 2, true, SETTINGS), None);
        assert_eq!(tracker.up(point(10, 10)), None);
        tracker.down(point(10, 10), 300, 1, false, SETTINGS);
        assert_eq!(tracker.up(point(10, 10)), None);
    }

    #[test]
    fn dispatch_uses_original_time_filters_own_windows_and_drops_stale_samples() {
        use super::{PointerSample, process_sample};
        use std::time::{Duration, Instant};
        let mut tracker = GestureTracker::default();
        let sample = |down, time, x| PointerSample {
            down,
            time,
            point: point(x, 10),
            sent_at: Instant::now(),
        };
        let outside = |_| (1, false, SETTINGS);
        assert_eq!(
            process_sample(&mut tracker, sample(true, 100, 10), outside),
            Some(Gesture::Started)
        );
        assert_eq!(
            process_sample(&mut tracker, sample(false, 150, 10), |_| panic!(
                "release must not query windows"
            )),
            None
        );
        assert_eq!(
            process_sample(&mut tracker, sample(true, 600, 10), outside),
            Some(Gesture::Started)
        );
        assert_eq!(
            process_sample(&mut tracker, sample(false, 610, 10), outside),
            Some(Gesture::Completed {
                start: point(10, 10),
                end: point(10, 10)
            })
        );
        assert_eq!(
            process_sample(&mut tracker, sample(true, 700, 10), |_| (2, true, SETTINGS)),
            None
        );
        assert_eq!(
            process_sample(&mut tracker, sample(false, 710, 40), outside),
            None
        );
        process_sample(&mut tracker, sample(true, 800, 10), outside);
        let mut stale = sample(false, 810, 40);
        stale.sent_at = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            process_sample(&mut tracker, stale, |_| panic!(
                "stale events must not query windows"
            )),
            None
        );
        assert!(tracker.press.is_none() && tracker.last_click.is_none());
    }

    #[test]
    fn slow_resolution_does_not_block_hook_payload_delivery() {
        use super::{PointerSample, process_sample};
        use std::{
            sync::mpsc,
            time::{Duration, Instant},
        };
        let (sender, receiver) = mpsc::channel();
        let (entered, busy) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut tracker = GestureTracker::default();
            let sample = receiver.recv().unwrap();
            process_sample(&mut tracker, sample, |_| {
                entered.send(()).unwrap();
                gate.recv().unwrap();
                (1, false, SETTINGS)
            });
            process_sample(&mut tracker, receiver.recv().unwrap(), |_| unreachable!())
        });
        let sample = |down, x| PointerSample {
            down,
            point: point(x, 10),
            time: 100,
            sent_at: Instant::now(),
        };
        sender.send(sample(true, 10)).unwrap();
        busy.recv_timeout(Duration::from_secs(2)).unwrap();
        sender.send(sample(false, 40)).unwrap(); // Resolving a window cannot hold this sender.
        resume.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap(),
            Some(Gesture::Completed {
                start: point(10, 10),
                end: point(40, 10)
            })
        );
    }
}
