use std::{cell::RefCell, sync::Arc};

use lexift_core::domain::geometry::Point;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentProcessId,
    UI::{
        Input::KeyboardAndMouse::GetDoubleClickTime,
        WindowsAndMessaging::{
            CallNextHookEx, GetSystemMetrics, GetWindowThreadProcessId, HC_ACTION, HHOOK,
            MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CYDOUBLECLK, SetWindowsHookExW, UnhookWindowsHookEx,
            WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WindowFromPoint,
        },
    },
};

const DRAG_DISTANCE_SQUARED: i64 = 36;

fn is_drag(start: Point, end: Point) -> bool {
    let dx = i64::from(end.x) - i64::from(start.x);
    let dy = i64::from(end.y) - i64::from(start.y);
    dx * dx + dy * dy >= DRAG_DISTANCE_SQUARED
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Started,
    Completed(Point),
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
            return Some(Gesture::Completed(point));
        }
        if press.second_click {
            self.last_click = None;
            return Some(Gesture::Completed(point));
        }
        self.last_click = Some(press.click);
        None
    }
}

struct Monitor {
    hook: HHOOK,
    tracker: GestureTracker,
    handler: Arc<dyn Fn(Gesture) + Send + Sync>,
}

thread_local! {
    static MONITOR: RefCell<Option<Monitor>> = const { RefCell::new(None) };
}

pub fn start(handler: Arc<dyn Fn(Gesture) + Send + Sync>) -> lexift_core::Result<()> {
    MONITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Ok(());
        }
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) }.map_err(
            |error| lexift_core::Error::new(format!("Could not monitor text selection: {error}")),
        )?;
        *slot = Some(Monitor {
            hook,
            tracker: GestureTracker::default(),
            handler,
        });
        Ok(())
    })
}

pub fn stop() {
    MONITOR.with(|slot| {
        if let Some(monitor) = slot.borrow_mut().take() {
            let _ = unsafe { UnhookWindowsHookEx(monitor.hook) };
        }
    });
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        let event = wparam.0 as u32;
        if event == WM_LBUTTONDOWN || event == WM_LBUTTONUP {
            let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let point = Point {
                x: data.pt.x,
                y: data.pt.y,
            };
            let (window, own_window) = if event == WM_LBUTTONDOWN {
                let window = unsafe { WindowFromPoint(data.pt) };
                let mut process_id = 0;
                if !window.0.is_null() {
                    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
                }
                (
                    window.0 as usize,
                    process_id == unsafe { GetCurrentProcessId() },
                )
            } else {
                (0, false)
            };
            let action = MONITOR.with(|slot| {
                let mut slot = slot.borrow_mut();
                let monitor = slot.as_mut()?;
                let gesture = if event == WM_LBUTTONDOWN {
                    monitor.tracker.down(
                        point,
                        data.time,
                        window,
                        own_window,
                        DoubleClickSettings::current(),
                    )
                } else {
                    monitor.tracker.up(point)
                }?;
                Some((Arc::clone(&monitor.handler), gesture))
            });
            if let Some((handler, gesture)) = action {
                handler(gesture);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{DoubleClickSettings, Gesture, GestureTracker, is_drag};
    use lexift_core::domain::geometry::Point;

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
            Some(Gesture::Completed(point(-96, 24)))
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
            Some(Gesture::Completed(point(16, 10)))
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
}
