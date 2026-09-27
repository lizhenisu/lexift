use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use lexift_core::{
    Error, Result,
    ports::tray::{TrayAction, TrayHandler, TrayMenuHandler, TrayMenuRequest, TrayPort},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETFOCUS,
                NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
                NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
                GetMessageW, IDI_APPLICATION, LoadIconW, MSG, PostQuitMessage, PostThreadMessageW,
                RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TranslateMessage,
                UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
                WM_DESTROY, WNDCLASSW,
            },
        },
    },
    core::{PCWSTR, w},
};

const TRAY_ICON_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const MENU_CANCELLED_MESSAGE: u32 = WM_APP + 2;
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

thread_local! {
    static WINDOW_STATE: RefCell<Option<TrayWindowState>> = const { RefCell::new(None) };
}

pub(crate) struct WindowsTrayPort {
    listener: Mutex<Option<TrayListener>>,
    menu_handler: Arc<Mutex<Option<TrayMenuHandler>>>,
}

impl WindowsTrayPort {
    pub(crate) fn new() -> Self {
        Self {
            listener: Mutex::new(None),
            menu_handler: Arc::new(Mutex::new(None)),
        }
    }
}

impl TrayPort for WindowsTrayPort {
    fn set_menu_handler(&self, handler: TrayMenuHandler) {
        *self.menu_handler.lock().unwrap_or_else(|p| p.into_inner()) = Some(handler);
    }

    fn menu_cancelled(&self) {
        if let Some(listener) = self
            .listener
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            unsafe {
                let _ = PostThreadMessageW(
                    listener.thread_id,
                    MENU_CANCELLED_MESSAGE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    fn register(&self, handler: TrayHandler) -> Result<()> {
        let mut listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if listener.is_some() {
            return Err(Error::new("Windows tray is already registered"));
        }

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let menu_handler = Arc::clone(&self.menu_handler);
        let thread = thread::Builder::new()
            .name("lexift-tray".into())
            .spawn(move || run_tray(handler, ready_sender, menu_handler))
            .map_err(|_| Error::new("Could not start the Windows tray thread"))?;

        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => {
                *listener = Some(TrayListener {
                    thread_id,
                    thread: Some(thread),
                });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new("Windows tray stopped during registration"))
            }
        }
    }
}

impl Drop for WindowsTrayPort {
    fn drop(&mut self) {
        let listener = self
            .listener
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(listener) = listener.take() {
            listener.shutdown();
        }
    }
}

struct TrayListener {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl TrayListener {
    fn shutdown(mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct TrayWindowState {
    hwnd: HWND,
    taskbar_created: u32,
    handler: TrayHandler,
    last_right_click: Cell<Option<(Instant, lexift_core::domain::geometry::Point)>>,
    menu_handler: Arc<Mutex<Option<TrayMenuHandler>>>,
}

fn run_tray(
    handler: TrayHandler,
    ready_sender: mpsc::SyncSender<Result<u32>>,
    menu_handler: Arc<Mutex<Option<TrayMenuHandler>>>,
) {
    let result = run_tray_inner(handler, &ready_sender, menu_handler);
    if let Err(error) = result {
        let _ = ready_sender.send(Err(error));
    }
}

fn run_tray_inner(
    handler: TrayHandler,
    ready_sender: &mpsc::SyncSender<Result<u32>>,
    menu_handler: Arc<Mutex<Option<TrayMenuHandler>>>,
) -> Result<()> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|_| Error::new("Could not get the Windows application module"))?;
    let instance = windows::Win32::Foundation::HINSTANCE(module.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(tray_window_proc),
        hInstance: instance,
        lpszClassName: w!("LexiftTrayWindow"),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::new("Could not register the Windows tray window"));
    }
    let _class = TrayWindowClass { instance };

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("LexiftTrayWindow"),
            w!("Lexift Tray"),
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
    .map_err(|_| Error::new("Could not create the Windows tray window"))?;
    let _window = TrayWindow(hwnd);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_created == 0 {
        return Err(Error::new("Could not register the TaskbarCreated message"));
    }
    WINDOW_STATE.with(|state| {
        *state.borrow_mut() = Some(TrayWindowState {
            hwnd,
            taskbar_created,
            handler,
            last_right_click: Cell::new(None),
            menu_handler,
        });
    });
    let _state = WindowStateGuard;
    let icon = TrayIcon::add(hwnd)?;

    let thread_id = unsafe { GetCurrentThreadId() };
    if ready_sender.send(Ok(thread_id)).is_err() {
        return Ok(());
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            break;
        }
        if message.message == MENU_CANCELLED_MESSAGE {
            // Explicit Escape cancellation only; focus-loss dismissal never posts this.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_SETFOCUS, &notify_data(hwnd));
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    drop(icon);
    Ok(())
}

unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = WINDOW_STATE.with(|state| {
        let state = state.borrow();
        let Some(state) = state.as_ref() else {
            return false;
        };
        if message == state.taskbar_created {
            if let Err(error) = TrayIcon::restore(state.hwnd) {
                tracing::warn!(%error, "Windows tray icon could not be restored after Explorer restart");
            }
            return true;
        }
        if message == TRAY_CALLBACK_MESSAGE {
            handle_tray_notification(state, lparam.0 as u32, wparam);
            return true;
        }
        false
    });
    if handled {
        return LRESULT(0);
    }
    match message {
        WM_CLOSE | WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn handle_tray_notification(state: &TrayWindowState, raw_event: u32, position: WPARAM) {
    use windows::Win32::UI::WindowsAndMessaging::{WM_RBUTTONDOWN, WM_RBUTTONUP};
    if raw_event & 0xffff == WM_RBUTTONDOWN {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_APPS, VK_F10, VK_RBUTTON,
        };
        // Explorer synthesizes right-button notifications for keyboard menus too.
        // Capture the source on the first notification: Apps may already be up
        // by WM_CONTEXTMENU, while a physical right click has its own input state.
        let right = unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) };
        let apps = unsafe { GetAsyncKeyState(VK_APPS.0 as i32) };
        let f10 = unsafe { GetAsyncKeyState(VK_F10.0 as i32) };
        let keyboard = f10 < 0 || (apps != 0 && right == 0);
        state
            .last_right_click
            .set((!keyboard).then(|| (Instant::now(), notification_anchor(position))));
    } else if raw_event & 0xffff == WM_RBUTTONUP && state.last_right_click.get().is_some() {
        state
            .last_right_click
            .set(Some((Instant::now(), notification_anchor(position))));
    }
    match notification_for_event(raw_event) {
        Some(TrayNotification::Open) => {
            invoke_handler(&state.handler, TrayAction::OpenMainWindow);
        }
        Some(TrayNotification::ContextMenu) => show_context_menu(state),
        None => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayNotification {
    Open,
    ContextMenu,
}

fn notification_for_event(raw_event: u32) -> Option<TrayNotification> {
    match raw_event & 0xffff {
        NIN_SELECT | NIN_KEYSELECT => Some(TrayNotification::Open),
        WM_CONTEXTMENU => Some(TrayNotification::ContextMenu),
        _ => None,
    }
}

fn notification_anchor(position: WPARAM) -> lexift_core::domain::geometry::Point {
    lexift_core::domain::geometry::Point {
        x: position.0 as u16 as i16 as i32,
        y: (position.0 >> 16) as u16 as i16 as i32,
    }
}

fn show_context_menu(state: &TrayWindowState) {
    // Version 4 defines wParam coordinates for mouse notifications, but not
    // WM_CONTEXTMENU. Preserve the preceding right-button anchor instead.
    let mouse = state
        .last_right_click
        .take()
        .filter(|(time, _)| time.elapsed() < Duration::from_secs(5));
    let keyboard = mouse.is_none();
    let mut anchor = mouse.map(|(_, point)| point).unwrap_or_default();
    if keyboard {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: state.hwnd,
            uID: TRAY_ICON_ID,
            ..Default::default()
        };
        if let Ok(rect) = unsafe { Shell_NotifyIconGetRect(&id) } {
            anchor.x = rect.left;
            anchor.y = rect.top;
        } else {
            let mut cursor = POINT::default();
            if unsafe { GetCursorPos(&mut cursor) }.is_err() {
                return;
            }
            anchor.x = cursor.x;
            anchor.y = cursor.y;
        }
    }
    // The shell grants foreground permission to its notification recipient.
    // Transfer it within our process before dispatching to the UI event loop.
    unsafe {
        let _ = SetForegroundWindow(state.hwnd);
    }
    let handler = state
        .menu_handler
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(handler) = handler {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            handler(TrayMenuRequest { anchor, keyboard })
        }));
    }
}

fn invoke_handler(handler: &TrayHandler, action: TrayAction) {
    let _ = catch_unwind(AssertUnwindSafe(|| handler(action)));
}

struct TrayIcon {
    data: NOTIFYICONDATAW,
}

impl TrayIcon {
    fn add(hwnd: HWND) -> Result<Self> {
        let data = add_icon(hwnd)?;
        Ok(Self { data })
    }

    fn restore(hwnd: HWND) -> Result<()> {
        add_icon(hwnd).map(|_| ())
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

fn add_icon(hwnd: HWND) -> Result<NOTIFYICONDATAW> {
    let mut data = notify_data(hwnd);
    data.hIcon = load_tray_icon()?;
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return Err(Error::new("Could not add the Windows tray icon"));
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
        return Err(Error::new("Could not configure the Windows tray icon"));
    }
    Ok(data)
}

#[allow(clippy::manual_dangling_ptr)]
fn load_tray_icon() -> Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|_| Error::new("Could not get the Windows application module"))?;
    let instance = windows::Win32::Foundation::HINSTANCE(module.0);
    // Win32 MAKEINTRESOURCE encodes the numeric resource ID as a pointer value.
    unsafe { LoadIconW(Some(instance), PCWSTR(1usize as *const u16)) }
        .or_else(|_| unsafe { LoadIconW(None, IDI_APPLICATION) })
        .map_err(|_| Error::new("Could not load the embedded Windows tray icon"))
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: TRAY_CALLBACK_MESSAGE,
        ..Default::default()
    };
    let tooltip: Vec<_> = "Lexift\0".encode_utf16().collect();
    data.szTip[..tooltip.len()].copy_from_slice(&tooltip);
    data
}

struct TrayWindow(HWND);

impl Drop for TrayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

struct TrayWindowClass {
    instance: windows::Win32::Foundation::HINSTANCE,
}

impl Drop for TrayWindowClass {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterClassW(w!("LexiftTrayWindow"), Some(self.instance));
        }
    }
}

struct WindowStateGuard;

impl Drop for WindowStateGuard {
    fn drop(&mut self) {
        WINDOW_STATE.with(|state| *state.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_coordinates_preserve_negative_monitor_positions() {
        let packed = ((-120i16 as u16 as usize) << 16) | (-1920i16 as u16 as usize);
        assert_eq!(
            notification_anchor(WPARAM(packed)),
            lexift_core::domain::geometry::Point { x: -1920, y: -120 }
        );
    }

    #[test]
    fn filters_supported_notification_messages() {
        assert_eq!(
            notification_for_event(NIN_SELECT),
            Some(TrayNotification::Open)
        );
        assert_eq!(
            notification_for_event(NIN_KEYSELECT),
            Some(TrayNotification::Open)
        );
        assert_eq!(
            notification_for_event(WM_CONTEXTMENU),
            Some(TrayNotification::ContextMenu)
        );
        assert_eq!(
            notification_for_event(windows::Win32::UI::WindowsAndMessaging::WM_NULL),
            None
        );
        // Version 4 emits semantic events; raw button-up notifications must not
        // start a second menu or activate the main window during menu tracking.
        use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONUP, WM_RBUTTONUP};
        assert_eq!(notification_for_event(WM_RBUTTONUP), None);
        assert_eq!(notification_for_event(WM_LBUTTONUP), None);
        assert_eq!(
            notification_for_event((TRAY_ICON_ID << 16) | WM_CONTEXTMENU),
            Some(TrayNotification::ContextMenu)
        );
    }

    #[test]
    #[ignore = "requires an interactive Windows notification area"]
    fn registers_and_shuts_down_the_native_tray() {
        let tray = WindowsTrayPort::new();
        tray.register(std::sync::Arc::new(|_| {}))
            .expect("the native tray should register");
        drop(tray);
    }
    #[test]
    fn version_four_icon_requests_standard_tooltip() {
        let data = notify_data(HWND::default());
        assert!(data.uFlags.contains(NIF_TIP | NIF_SHOWTIP));
    }
}
