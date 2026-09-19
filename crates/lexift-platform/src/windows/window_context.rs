use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, Weak, mpsc},
    thread::{self, JoinHandle},
};

use lexift_core::{Error, Result};
use raw_window_handle::HasWindowHandle;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount,
        Threading::GetCurrentThreadId,
    },
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        WindowsAndMessaging::{
            CallNextHookEx, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GA_ROOTOWNER, GetAncestor,
            GetMessageW, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT,
            WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
            WindowFromPoint,
        },
    },
};

use super::popup::required_hwnd;

type DismissHandler = Arc<dyn Fn() + Send + Sync + 'static>;

static ACTIVE_MONITOR: Mutex<Option<Weak<SharedState>>> = Mutex::new(None);

pub(crate) struct WindowsWindowContextMonitor {
    shared: Arc<SharedState>,
    listener: Option<ContextListener>,
}

impl WindowsWindowContextMonitor {
    pub(crate) fn new(handler: DismissHandler) -> Result<Self> {
        let shared = Arc::new(SharedState {
            armed_context: Mutex::new(None),
            handler,
        });
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let listener_state = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("lexift-window-context".into())
            .spawn(move || run_listener(listener_state, ready_sender))
            .map_err(|_| Error::new("Could not start the window context monitor"))?;

        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                shared,
                listener: Some(ContextListener {
                    thread_id,
                    thread: Some(thread),
                }),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new(
                    "Window context monitor stopped during registration",
                ))
            }
        }
    }

    pub(crate) fn arm_context(&self, windows: &[&dyn HasWindowHandle]) -> Result<()> {
        let mut members = Vec::with_capacity(windows.len() * 3);
        for window in windows {
            let window = required_hwnd(window)?;
            for identity in window_identities(window) {
                if identity != 0 && !members.contains(&identity) {
                    members.push(identity);
                }
            }
        }
        if members.is_empty() {
            return Err(Error::new("Window context members are unavailable"));
        }
        *self
            .shared
            .armed_context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ArmedContext {
            members,
            armed_at: unsafe { GetTickCount() },
        });
        Ok(())
    }

    pub(crate) fn disarm(&self) {
        self.shared.disarm();
    }
}

impl Drop for WindowsWindowContextMonitor {
    fn drop(&mut self) {
        self.shared.disarm();
        if let Some(listener) = self.listener.take() {
            listener.shutdown();
        }
    }
}

struct SharedState {
    armed_context: Mutex<Option<ArmedContext>>,
    handler: DismissHandler,
}

struct ArmedContext {
    members: Vec<isize>,
    armed_at: u32,
}

#[derive(Clone, Copy, Debug)]
enum InteractionSource {
    Foreground,
    Mouse,
}

impl SharedState {
    fn disarm(&self) {
        *self
            .armed_context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn dismiss_if_external(
        &self,
        target: HWND,
        event_time: u32,
        source: InteractionSource,
    ) -> bool {
        self.dismiss_for_identities(&window_identities(target), event_time, source)
    }

    fn dismiss_for_identities(
        &self,
        target_identities: &[isize],
        event_time: u32,
        source: InteractionSource,
    ) -> bool {
        let classification = {
            let mut armed_context = self
                .armed_context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match armed_context.as_ref() {
                Some(armed) if !event_is_after_arm(event_time, armed.armed_at) => None,
                Some(armed)
                    if target_identities
                        .iter()
                        .any(|identity| *identity != 0 && armed.members.contains(identity)) =>
                {
                    Some(false)
                }
                Some(_) => {
                    *armed_context = None;
                    Some(true)
                }
                None => None,
            }
        };
        if let Some(external) = classification {
            tracing::debug!(?source, external, "window context interaction classified");
        }
        if classification == Some(true) {
            let handler = Arc::clone(&self.handler);
            let _ = catch_unwind(AssertUnwindSafe(move || handler()));
        }
        classification == Some(true)
    }
}

struct ContextListener {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl ContextListener {
    fn shutdown(mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_listener(shared: Arc<SharedState>, ready_sender: mpsc::SyncSender<Result<u32>>) {
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    {
        let mut active = ACTIVE_MONITOR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.as_ref().and_then(Weak::upgrade).is_some() {
            let _ = ready_sender.send(Err(Error::new(
                "A window context monitor is already running",
            )));
            return;
        }
        *active = Some(Arc::downgrade(&shared));
    }

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(_) => {
            clear_active_monitor(&shared);
            let _ = ready_sender.send(Err(Error::new(
                "Could not resolve the application module for the window context monitor",
            )));
            return;
        }
    };
    let mouse_hook = match unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook_proc),
            Some(HINSTANCE(module.0)),
            0,
        )
    } {
        Ok(hook) => hook,
        Err(_) => {
            clear_active_monitor(&shared);
            let _ = ready_sender.send(Err(Error::new(
                "Could not install the outside-click monitor",
            )));
            return;
        }
    };
    let foreground_hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if foreground_hook.0.is_null() {
        unsafe {
            let _ = UnhookWindowsHookEx(mouse_hook);
        }
        clear_active_monitor(&shared);
        let _ = ready_sender.send(Err(Error::new(
            "Could not install the foreground-change monitor",
        )));
        return;
    }

    if ready_sender.send(Ok(thread_id)).is_ok() {
        loop {
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
            if result <= 0 {
                break;
            }
        }
    }

    unsafe {
        let _ = UnhookWinEvent(foreground_hook);
        let _ = UnhookWindowsHookEx(mouse_hook);
    }
    clear_active_monitor(&shared);
}

fn clear_active_monitor(shared: &Arc<SharedState>) {
    let mut active = ACTIVE_MONITOR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if active
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|current| Arc::ptr_eq(&current, shared))
    {
        *active = None;
    }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && is_mouse_button_down(wparam.0 as u32) {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let target = unsafe { WindowFromPoint(event.pt) };
        notify_if_external(target, event.time, InteractionSource::Mouse);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn foreground_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    event_time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        notify_if_external(hwnd, event_time, InteractionSource::Foreground);
    }
}

fn notify_if_external(target: HWND, event_time: u32, source: InteractionSource) -> bool {
    let shared = ACTIVE_MONITOR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(Weak::upgrade);
    shared.is_some_and(|shared| shared.dismiss_if_external(target, event_time, source))
}

fn window_identities(window: HWND) -> [isize; 3] {
    if window.0.is_null() {
        return [0; 3];
    }
    [
        window.0 as isize,
        unsafe { GetAncestor(window, GA_ROOT) }.0 as isize,
        unsafe { GetAncestor(window, GA_ROOTOWNER) }.0 as isize,
    ]
}

fn is_mouse_button_down(message: u32) -> bool {
    matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    )
}

fn event_is_after_arm(event_time: u32, armed_at: u32) -> bool {
    (event_time.wrapping_sub(armed_at) as i32) > 0
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn test_shared(handler: DismissHandler) -> SharedState {
        SharedState {
            armed_context: Mutex::new(None),
            handler,
        }
    }

    #[test]
    fn mouse_monitor_only_observes_button_down_messages() {
        assert!(is_mouse_button_down(WM_LBUTTONDOWN));
        assert!(is_mouse_button_down(WM_RBUTTONDOWN));
        assert!(is_mouse_button_down(WM_MBUTTONDOWN));
        assert!(is_mouse_button_down(WM_XBUTTONDOWN));
        assert!(!is_mouse_button_down(0x0200));
    }

    #[test]
    fn disarmed_and_same_context_events_do_not_dismiss() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let shared = test_shared(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(!shared.dismiss_for_identities(&[11], 101, InteractionSource::Mouse));
        *shared.armed_context.lock().unwrap() = Some(ArmedContext {
            members: vec![11, 12, 13],
            armed_at: 100,
        });
        assert!(!shared.dismiss_for_identities(&[21, 12, 22], 101, InteractionSource::Mouse));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn external_event_dismisses_once_and_disarms() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let shared = test_shared(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        *shared.armed_context.lock().unwrap() = Some(ArmedContext {
            members: vec![11, 12, 13],
            armed_at: 100,
        });

        assert!(shared.dismiss_for_identities(&[21, 22, 23], 101, InteractionSource::Mouse));
        assert!(!shared.dismiss_for_identities(&[21, 22, 23], 102, InteractionSource::Foreground));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn independently_rooted_members_belong_to_one_dynamic_context() {
        let shared = test_shared(Arc::new(|| {}));
        *shared.armed_context.lock().unwrap() = Some(ArmedContext {
            members: vec![11, 12, 13, 31, 32, 33],
            armed_at: 100,
        });

        assert!(!shared.dismiss_for_identities(&[41, 32, 42], 101, InteractionSource::Mouse));
        assert!(!shared.dismiss_for_identities(&[31, 51, 52], 102, InteractionSource::Foreground));
    }

    #[test]
    fn queued_events_from_before_arm_are_ignored() {
        assert!(!event_is_after_arm(100, 100));
        assert!(!event_is_after_arm(99, 100));
        assert!(event_is_after_arm(101, 100));
        assert!(event_is_after_arm(1, u32::MAX));
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn installs_and_removes_native_hooks() {
        let monitor = WindowsWindowContextMonitor::new(Arc::new(|| {}))
            .expect("native context hooks should register");
        monitor.disarm();
        drop(monitor);
    }
}
