use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use lexift_core::{
    Error, Result,
    ports::tray::{TrayAction, TrayHandler, TrayMenuLabels, TrayPort},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETFOCUS,
                NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION,
                LoadIconW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage,
                RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, UnregisterClassW,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
                WM_NULL, WNDCLASSW,
            },
        },
    },
    core::{PCWSTR, w},
};

const TRAY_ICON_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const OPEN_COMMAND_ID: usize = 1;
const SETTINGS_COMMAND_ID: usize = 2;
const QUIT_COMMAND_ID: usize = 3;
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

thread_local! {
    static WINDOW_STATE: RefCell<Option<TrayWindowState>> = const { RefCell::new(None) };
}

pub(crate) struct WindowsTrayPort {
    listener: Mutex<Option<TrayListener>>,
    theme: Arc<AtomicU8>,
    labels: Arc<Mutex<TrayMenuLabels>>,
}

impl WindowsTrayPort {
    pub(crate) fn new() -> Self {
        Self {
            listener: Mutex::new(None),
            theme: Arc::new(AtomicU8::new(0)),
            labels: Arc::new(Mutex::new(TrayMenuLabels::default())),
        }
    }
}

impl TrayPort for WindowsTrayPort {
    fn set_menu_labels(&self, labels: TrayMenuLabels) {
        *self.labels.lock().unwrap_or_else(|p| p.into_inner()) = labels;
    }
    fn set_theme(&self, theme: lexift_core::domain::settings::ThemePreference) {
        use lexift_core::domain::settings::ThemePreference;
        self.theme.store(
            match theme {
                ThemePreference::Light => 0,
                ThemePreference::Dark => 1,
                ThemePreference::System => 2,
            },
            Ordering::Relaxed,
        );
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
        let theme = Arc::clone(&self.theme);
        let labels = Arc::clone(&self.labels);
        let thread = thread::Builder::new()
            .name("lexift-tray".into())
            .spawn(move || run_tray(handler, ready_sender, theme, labels))
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
    menu_active: Cell<bool>,
    theme: Arc<AtomicU8>,
    labels: Arc<Mutex<TrayMenuLabels>>,
}

fn run_tray(
    handler: TrayHandler,
    ready_sender: mpsc::SyncSender<Result<u32>>,
    theme: Arc<AtomicU8>,
    labels: Arc<Mutex<TrayMenuLabels>>,
) {
    let result = run_tray_inner(handler, &ready_sender, theme, labels);
    if let Err(error) = result {
        let _ = ready_sender.send(Err(error));
    }
}

fn run_tray_inner(
    handler: TrayHandler,
    ready_sender: &mpsc::SyncSender<Result<u32>>,
    theme: Arc<AtomicU8>,
    labels: Arc<Mutex<TrayMenuLabels>>,
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
            menu_active: Cell::new(false),
            theme,
            labels,
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
    if message == windows::Win32::UI::WindowsAndMessaging::WM_MENUCHAR
        && let Some(result) = super::tray_menu::menu_character(wparam.0 as u16, lparam)
    {
        return result;
    }
    if super::tray_menu::handle_draw_message(message, lparam) {
        return LRESULT(1);
    }
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
            handle_tray_notification(state, lparam.0 as u32);
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

fn handle_tray_notification(state: &TrayWindowState, raw_event: u32) {
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

fn show_context_menu(state: &TrayWindowState) {
    // TrackPopupMenu pumps messages recursively. Never open another menu inside it.
    let Some(_active) = MenuSession::begin(&state.menu_active) else {
        return;
    };
    let labels = state
        .labels
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let appearance =
        super::tray_menu::MenuAppearance::begin(state.theme.load(Ordering::Relaxed), &labels);
    let text = [&labels.open, &labels.settings, &labels.quit]
        .map(|s| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        tracing::warn!("Windows tray context menu could not be created");
        return;
    };
    let _menu = TrayMenu(menu);
    let built = unsafe {
        AppendMenuW(menu, MF_STRING, OPEN_COMMAND_ID, PCWSTR(text[0].as_ptr()))
            .and_then(|_| {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    SETTINGS_COMMAND_ID,
                    PCWSTR(text[1].as_ptr()),
                )
            })
            .and_then(|_| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
            .and_then(|_| AppendMenuW(menu, MF_STRING, QUIT_COMMAND_ID, PCWSTR(text[2].as_ptr())))
            .and_then(|_| SetMenuDefaultItem(menu, OPEN_COMMAND_ID as u32, 0))
    };
    if built.is_err() {
        tracing::warn!("Windows tray context menu could not be populated");
        return;
    }

    appearance.configure(menu);
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        tracing::warn!("Windows tray context menu cursor position is unavailable");
        return;
    }
    unsafe {
        let _ = SetForegroundWindow(state.hwnd);
    }
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            None,
            state.hwnd,
            None,
        )
    }
    .0 as usize;
    unsafe {
        let _ = PostMessageW(Some(state.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: state.hwnd,
            uID: TRAY_ICON_ID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_SETFOCUS, &data);
    }
    // Return focus to the tray before Open/Quit transfers control to the app.
    if let Some(action) = action_for_command(command) {
        invoke_handler(&state.handler, action);
    }
}

struct MenuSession<'a>(&'a Cell<bool>);

impl<'a> MenuSession<'a> {
    fn begin(active: &'a Cell<bool>) -> Option<Self> {
        if active.replace(true) {
            None
        } else {
            Some(Self(active))
        }
    }
}

impl Drop for MenuSession<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn action_for_command(command: usize) -> Option<TrayAction> {
    match command {
        OPEN_COMMAND_ID => Some(TrayAction::OpenMainWindow),
        SETTINGS_COMMAND_ID => Some(TrayAction::OpenSettings),
        QUIT_COMMAND_ID => Some(TrayAction::Quit),
        _ => None,
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

struct TrayMenu(windows::Win32::UI::WindowsAndMessaging::HMENU);

impl Drop for TrayMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
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
    fn maps_only_supported_menu_commands() {
        assert_eq!(
            action_for_command(OPEN_COMMAND_ID),
            Some(TrayAction::OpenMainWindow)
        );
        assert_eq!(action_for_command(QUIT_COMMAND_ID), Some(TrayAction::Quit));
        assert_eq!(
            action_for_command(SETTINGS_COMMAND_ID),
            Some(TrayAction::OpenSettings)
        );
        assert_eq!(action_for_command(999), None);
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
        assert_eq!(notification_for_event(WM_NULL), None);
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
    fn menu_tracking_rejects_reentry_and_releases_on_close() {
        let active = Cell::new(false);
        let session = MenuSession::begin(&active).unwrap();
        assert!(MenuSession::begin(&active).is_none());
        assert!(active.get());
        drop(session);
        assert!(!active.get());
        assert!(MenuSession::begin(&active).is_some());
    }

    #[test]
    fn version_four_icon_requests_standard_tooltip() {
        let data = notify_data(HWND::default());
        assert!(data.uFlags.contains(NIF_TIP | NIF_SHOWTIP));
    }
}
