use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use lexift_core::{
    Error, Result,
    ports::instance::{InstanceActivationHandler, InstancePort, InstanceStatus},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateMutexW, GetCurrentThreadId},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
            GetMessageW, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
            RegisterWindowMessageW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE,
            WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WM_QUIT, WNDCLASSW,
        },
    },
    core::w,
};

const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\Lexift.SingleInstance.0.1");
const WINDOW_CLASS: windows::core::PCWSTR = w!("LexiftInstanceWindow");
const WINDOW_TITLE: windows::core::PCWSTR = w!("Lexift Instance Coordinator");
const ACTIVATION_MESSAGE_NAME: windows::core::PCWSTR = w!("LexiftOpenMainWindow");

thread_local! {
    static WINDOW_STATE: RefCell<Option<Arc<ActivationState>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct ActivationState {
    handler: Mutex<Option<InstanceActivationHandler>>,
    pending: AtomicBool,
}

pub(crate) struct WindowsInstancePort {
    state: Arc<ActivationState>,
    primary: Mutex<Option<PrimaryInstance>>,
}

impl WindowsInstancePort {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(ActivationState::default()),
            primary: Mutex::new(None),
        }
    }
}

impl InstancePort for WindowsInstancePort {
    fn acquire(&self) -> Result<InstanceStatus> {
        let mut primary = self
            .primary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if primary.is_some() {
            return Ok(InstanceStatus::Primary);
        }
        let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME) }
            .map_err(|_| Error::new("Could not create the Lexift instance mutex"))?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            notify_primary_instance();
            return Ok(InstanceStatus::AlreadyRunning);
        }
        let listener = InstanceListener::start(Arc::clone(&self.state))?;
        *primary = Some(PrimaryInstance {
            mutex: mutex.0 as isize,
            listener,
        });
        Ok(InstanceStatus::Primary)
    }

    fn set_activation_handler(&self, handler: InstanceActivationHandler) -> Result<()> {
        *self
            .state
            .handler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&handler));
        if self.state.pending.swap(false, Ordering::SeqCst) {
            invoke_handler(&handler);
        }
        Ok(())
    }
}

struct PrimaryInstance {
    mutex: isize,
    listener: InstanceListener,
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        self.listener.shutdown();
        unsafe {
            let _ = CloseHandle(HANDLE(self.mutex as *mut std::ffi::c_void));
        }
    }
}

struct InstanceListener {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl InstanceListener {
    fn start(state: Arc<ActivationState>) -> Result<Self> {
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("lexift-instance".into())
            .spawn(move || run_instance_window(state, ready_sender))
            .map_err(|_| Error::new("Could not start the instance coordination thread"))?;
        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new("Instance coordination stopped during startup"))
            }
        }
    }

    fn shutdown(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_instance_window(state: Arc<ActivationState>, ready_sender: mpsc::SyncSender<Result<u32>>) {
    let result = run_instance_window_inner(state, &ready_sender);
    if let Err(error) = result {
        let _ = ready_sender.send(Err(error));
    }
}

fn run_instance_window_inner(
    state: Arc<ActivationState>,
    ready_sender: &mpsc::SyncSender<Result<u32>>,
) -> Result<()> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|_| Error::new("Could not get the Windows application module"))?;
    let instance = windows::Win32::Foundation::HINSTANCE(module.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(instance_window_proc),
        hInstance: instance,
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::new(
            "Could not register the instance coordination window",
        ));
    }
    let _class = InstanceWindowClass(instance);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            WINDOW_TITLE,
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|_| Error::new("Could not create the instance coordination window"))?;
    let _window = InstanceWindow(hwnd);
    WINDOW_STATE.with(|slot| *slot.borrow_mut() = Some(state));
    let _state = InstanceWindowStateGuard;
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready_sender.send(Ok(thread_id)).is_err() {
        return Ok(());
    }
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe extern "system" fn instance_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == activation_message() {
        WINDOW_STATE.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                if let Some(handler) = state
                    .handler
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                {
                    invoke_handler(&handler);
                } else {
                    state.pending.store(true, Ordering::SeqCst);
                }
            }
        });
        return LRESULT(0);
    }
    match message {
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn notify_primary_instance() {
    for _ in 0..20 {
        if let Ok(hwnd) = unsafe { FindWindowW(WINDOW_CLASS, WINDOW_TITLE) } {
            let _ = unsafe { PostMessageW(Some(hwnd), activation_message(), WPARAM(0), LPARAM(0)) };
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn activation_message() -> u32 {
    unsafe { RegisterWindowMessageW(ACTIVATION_MESSAGE_NAME) }
}

fn invoke_handler(handler: &InstanceActivationHandler) {
    if catch_unwind(AssertUnwindSafe(|| handler())).is_err() {
        tracing::error!("instance activation handler panicked");
    }
}

struct InstanceWindow(HWND);

impl Drop for InstanceWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

struct InstanceWindowClass(windows::Win32::Foundation::HINSTANCE);

impl Drop for InstanceWindowClass {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterClassW(WINDOW_CLASS, Some(self.0));
        }
    }
}

struct InstanceWindowStateGuard;

impl Drop for InstanceWindowStateGuard {
    fn drop(&mut self) {
        WINDOW_STATE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_activation_runs_when_the_handler_is_installed() {
        let port = WindowsInstancePort::new();
        port.state.pending.store(true, Ordering::SeqCst);
        let called = Arc::new(AtomicBool::new(false));
        let called_for_handler = Arc::clone(&called);
        port.set_activation_handler(Arc::new(move || {
            called_for_handler.store(true, Ordering::SeqCst);
        }))
        .unwrap();
        assert!(called.load(Ordering::SeqCst));
        assert!(!port.state.pending.load(Ordering::SeqCst));
    }

    #[test]
    #[ignore = "uses the process-global Lexift instance mutex and native message window"]
    fn second_instance_notifies_the_primary_instance() {
        let primary = WindowsInstancePort::new();
        assert_eq!(primary.acquire().unwrap(), InstanceStatus::Primary);
        let called = Arc::new(AtomicBool::new(false));
        let called_for_handler = Arc::clone(&called);
        primary
            .set_activation_handler(Arc::new(move || {
                called_for_handler.store(true, Ordering::SeqCst);
            }))
            .unwrap();

        let secondary = WindowsInstancePort::new();
        assert_eq!(secondary.acquire().unwrap(), InstanceStatus::AlreadyRunning);
        for _ in 0..20 {
            if called.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("primary instance did not receive the activation message");
    }
}
