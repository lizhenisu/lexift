use super::window_shell_policy as shell_policy;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use super::hook_thread::HookMessageThread;

use crate::{
    PassiveToolWindowPreparation, PopupDismissCause, PopupPointerEvent, PopupPointerHandler,
    PopupResizeEdge,
};

pub(crate) fn configure_passive(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<PassiveToolWindowPreparation> {
    use raw_window_handle::{HandleError, RawWindowHandle};
    use windows::Win32::Foundation::HWND;

    let handle = match window.window_handle() {
        Ok(handle) => handle,
        Err(HandleError::NotSupported | HandleError::Unavailable) => {
            return Ok(PassiveToolWindowPreparation::Pending);
        }
        Err(error) => {
            return Err(lexift_core::Error::new(format!(
                "Window handle is unavailable: {error}"
            )));
        }
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(lexift_core::Error::new(
            "Win32 window handle is unavailable",
        ));
    };
    let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);
    shell_policy::install(hwnd, Some(shell_policy::Activation::Passive))?;
    configure_hwnd_extended_style(hwnd, passive_extended_style)?;
    Ok(PassiveToolWindowPreparation::Ready)
}

/// Applies to floating menus; normal application windows retain their taskbar policy.
pub(crate) fn configure_menu_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    shell_policy::install(required_hwnd(window)?, None)?;
    configure_extended_style(window, menu_window_extended_style)
}

pub(crate) fn configure_shell_role(
    window: &impl raw_window_handle::HasWindowHandle,
    settings: bool,
) -> lexift_core::Result<()> {
    shell_policy::set_settings_role(required_hwnd(window)?, settings)
}

fn menu_window_extended_style(style: isize) -> isize {
    shell_policy::tool_style(style)
}

/// Requests Windows 11's native rounded outer corners for the translation popup.
///
/// The caller selects an opaque square shell if the DWM preference is unavailable, avoiding a
/// second Slint-drawn corner that cannot match the native window clip.
pub(crate) fn configure_translation_popup_corners(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::{Foundation::HANDLE, UI::WindowsAndMessaging::SetPropW};
    use windows::core::w;

    let hwnd = required_hwnd(window)?;
    unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.TranslationPopup"),
            Some(HANDLE(std::ptr::dangling_mut())),
        )
    }
    .map_err(|error| lexift_core::Error::new(format!("Could not mark popup HWND: {error}")))?;
    let preference = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as _,
            std::mem::size_of_val(&preference) as u32,
        )
    }
    .map_err(|error| {
        lexift_core::Error::new(format!(
            "Could not request rounded translation popup corners: {error}"
        ))
    })
}

pub(crate) fn enable_interaction_without_activation(
    window: &impl raw_window_handle::HasWindowHandle,
    pointer_handler: PopupPointerHandler,
) -> lexift_core::Result<()> {
    // Update the role before writing styles; the persistent guard otherwise
    // correctly retains the previous passive WS_EX_NOACTIVATE policy.
    shell_policy::install(
        required_hwnd(window)?,
        Some(shell_policy::Activation::Interactive),
    )?;
    install_pointer_bridge(window, pointer_handler, true)?;
    configure_extended_style(window, interactive_extended_style)
}

pub(crate) fn enable_passive_interaction(
    window: &impl raw_window_handle::HasWindowHandle,
    pointer_handler: PopupPointerHandler,
) -> lexift_core::Result<()> {
    shell_policy::install(
        required_hwnd(window)?,
        Some(shell_policy::Activation::Passive),
    )?;
    install_pointer_bridge(window, pointer_handler, false)?;
    configure_extended_style(window, passive_extended_style)
}

/// Passive client dragging retains the previous foreground window, so Escape
/// cannot be delivered as a toolbar key event. Query it only while this HWND
/// owns capture; releasing capture also clears the native pressed state.
pub(crate) fn cancel_toolbar_drag_on_escape(
    window: &impl raw_window_handle::HasWindowHandle,
) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetCapture, ReleaseCapture, VK_ESCAPE,
    };
    let Ok(hwnd) = required_hwnd(window) else {
        return false;
    };
    unsafe {
        if GetCapture() != hwnd || GetAsyncKeyState(VK_ESCAPE.0 as i32) >= 0 {
            return false;
        }
        let _ = ReleaseCapture();
    }
    true
}

pub(crate) fn attach_owner(
    child: &impl raw_window_handle::HasWindowHandle,
    owner: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{
            GWLP_HWNDPARENT, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
            SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos,
        },
    };
    let child = required_hwnd(child)?;
    let owner = required_hwnd(owner)?;
    unsafe {
        SetLastError(WIN32_ERROR(0));
        let previous = SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner.0 as isize);
        if previous == 0 && GetLastError().0 != 0 {
            return Err(lexift_core::Error::new(
                "Could not attach tool window to its owner",
            ));
        }
        SetWindowPos(
            child,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOMOVE | SWP_NOSIZE,
        )
        .map_err(|_| lexift_core::Error::new("Could not raise tool window above its owner"))?;
    }
    super::annotation_window_order::tool(child, owner).map_err(|error| {
        lexift_core::Error::new(format!("Could not register annotation tool order: {error}"))
    })?;
    Ok(())
}

const POPUP_INPUT_SUBCLASS_ID: usize = 0x4C58_4654;
const POPUP_DISMISS_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x4C;
const WHEEL_DELTA: f32 = 120.0;
const LOGICAL_SCROLL_PIXELS_PER_NOTCH: f32 = 60.0;

thread_local! {
    static POPUP_HOOK_STATE: RefCell<Option<PopupHookState>> = const { RefCell::new(None) };
}

struct PopupHookState {
    sender: mpsc::Sender<DismissalEvent>,
    max_callback_us: Arc<AtomicU64>,
}

enum DismissalEvent {
    Watch(usize, usize, u64, Instant),
    Unwatch(usize),
    MouseDown(i32, i32, Instant),
    Foreground(usize, Instant),
    Escape(Instant),
    Focus(usize, Instant),
}

#[derive(Clone, Copy, Debug)]
struct PopupDismissWatch {
    instance: u64,
    started_at: Instant,
    initial_foreground: usize,
    foreground_changed: bool,
    dismissal_posted: bool,
}

struct PopupDismissMonitor {
    watches: HashMap<usize, PopupDismissWatch>,
}

impl PopupDismissMonitor {
    fn watch(
        &mut self,
        hwnd: usize,
        initial_foreground: usize,
        instance: u64,
        started_at: Instant,
    ) {
        self.watches.insert(
            hwnd,
            PopupDismissWatch {
                instance,
                started_at,
                initial_foreground,
                foreground_changed: false,
                dismissal_posted: false,
            },
        );
    }

    fn unwatch(&mut self, hwnd: usize) {
        self.watches.remove(&hwnd);
    }
}

#[path = "popup_input_timing.rs"]
mod input_timing;

struct PopupDismissHandle {
    sender: mpsc::Sender<DismissalEvent>,
    hook_thread: HookMessageThread,
    dispatcher: JoinHandle<()>,
    watched: HashSet<usize>,
    max_callback_us: Arc<AtomicU64>,
    max_dispatch_delay_us: Arc<AtomicU64>,
}

static ACTIVE_DISMISSAL: OnceLock<Mutex<Option<PopupDismissHandle>>> = OnceLock::new();

fn active_dismissal() -> &'static Mutex<Option<PopupDismissHandle>> {
    ACTIVE_DISMISSAL.get_or_init(|| Mutex::new(None))
}

fn start_dismissal() -> lexift_core::Result<PopupDismissHandle> {
    use windows::Win32::UI::{
        Accessibility::SetWinEventHook,
        WindowsAndMessaging::{
            EVENT_SYSTEM_FOREGROUND, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL,
            WINEVENT_OUTOFCONTEXT,
        },
    };
    let (sender, receiver) = mpsc::channel::<DismissalEvent>();
    let max_dispatch_delay_us = Arc::new(AtomicU64::new(0));
    let dispatch_delay = Arc::clone(&max_dispatch_delay_us);
    let dispatcher = thread::Builder::new()
        .name("lexift-popup-dismiss-dispatch".into())
        .spawn(move || {
            let mut monitor = PopupDismissMonitor {
                watches: HashMap::new(),
            };
            while let Ok(event) = receiver.recv() {
                match event {
                    DismissalEvent::Watch(hwnd, foreground, instance, started_at) => {
                        monitor.watch(hwnd, foreground, instance, started_at)
                    }
                    DismissalEvent::Unwatch(hwnd) => monitor.unwatch(hwnd),
                    DismissalEvent::MouseDown(x, y, sent_at) => {
                        dispatch_delay
                            .fetch_max(sent_at.elapsed().as_micros() as u64, Ordering::Relaxed);
                        post_dismissals_for_outside_point(&mut monitor, x, y, sent_at);
                    }
                    DismissalEvent::Focus(focus, sent_at) => {
                        let root = hwnd_key(unsafe {
                            windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                                hwnd_from_key(focus),
                                windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                            )
                        });
                        for (key, watch) in &mut monitor.watches {
                            let hwnd = hwnd_from_key(*key);
                            if !is_ocr_selection(hwnd)
                                && root == watch.initial_foreground
                                && sent_at >= watch.started_at
                                && !watch.dismissal_posted
                                && !unsafe {
                                    windows::Win32::UI::WindowsAndMessaging::GetPropW(
                                        hwnd,
                                        windows::core::w!("Lexift.ClipboardHistoryEscape"),
                                    )
                                }
                                .0
                                .is_null()
                            {
                                post_dismissal(hwnd, watch, PopupDismissCause::ForegroundChanged);
                            }
                        }
                    }
                    DismissalEvent::Escape(sent_at) => {
                        for (key, watch) in &mut monitor.watches {
                            let hwnd = hwnd_from_key(*key);
                            if sent_at >= watch.started_at
                                && !watch.dismissal_posted
                                && !unsafe {
                                    windows::Win32::UI::WindowsAndMessaging::GetPropW(
                                        hwnd,
                                        windows::core::w!("Lexift.ClipboardHistoryEscape"),
                                    )
                                }
                                .0
                                .is_null()
                            {
                                post_dismissal(hwnd, watch, PopupDismissCause::Escape);
                            }
                        }
                    }
                    DismissalEvent::Foreground(hwnd, sent_at) => {
                        dispatch_delay
                            .fetch_max(sent_at.elapsed().as_micros() as u64, Ordering::Relaxed);
                        post_dismissals_for_foreground(&mut monitor, hwnd, sent_at);
                    }
                }
            }
        })
        .map_err(|error| {
            lexift_core::Error::new(format!("Could not dispatch popup dismissal: {error}"))
        })?;
    let max_callback_us = Arc::new(AtomicU64::new(0));
    let hook_sender = sender.clone();
    let hook_max = Arc::clone(&max_callback_us);
    let hook_thread = match HookMessageThread::start("lexift-popup-dismiss-hook", move || {
        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(popup_mouse_hook), None, 0) }
            .map_err(|_| {
                lexift_core::Error::new("Could not monitor pointer input outside popup")
            })?;
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(popup_foreground_hook),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if foreground_hook.0.is_null() {
            let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
            return Err(lexift_core::Error::new(
                "Could not monitor foreground changes for popup",
            ));
        }
        let keyboard_hook = match unsafe {
            SetWindowsHookExW(
                windows::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL,
                Some(history_keyboard_hook),
                None,
                0,
            )
        } {
            Ok(h) => h,
            Err(_) => {
                let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
                let _ =
                    unsafe { windows::Win32::UI::Accessibility::UnhookWinEvent(foreground_hook) };
                return Err(lexift_core::Error::new(
                    "Could not monitor clipboard history cancellation",
                ));
            }
        };
        let focus_hook = unsafe {
            SetWinEventHook(
                windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS,
                windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS,
                None,
                Some(history_focus_hook),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if focus_hook.0.is_null() {
            let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
            let _ = unsafe { UnhookWindowsHookEx(keyboard_hook) };
            let _ = unsafe { windows::Win32::UI::Accessibility::UnhookWinEvent(foreground_hook) };
            return Err(lexift_core::Error::new(
                "Could not monitor original input focus",
            ));
        }
        POPUP_HOOK_STATE.with(|slot| {
            *slot.borrow_mut() = Some(PopupHookState {
                sender: hook_sender,
                max_callback_us: hook_max,
            })
        });
        Ok(Box::new(move || {
            use windows::Win32::UI::Accessibility::UnhookWinEvent;
            POPUP_HOOK_STATE.with(|slot| *slot.borrow_mut() = None);
            let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
            let _ = unsafe { UnhookWindowsHookEx(keyboard_hook) };
            let _ = unsafe { UnhookWinEvent(foreground_hook) };
            let _ = unsafe { UnhookWinEvent(focus_hook) };
        }))
    }) {
        Ok(thread) => thread,
        Err(error) => {
            drop(sender);
            let _ = dispatcher.join();
            return Err(error);
        }
    };
    Ok(PopupDismissHandle {
        sender,
        hook_thread,
        dispatcher,
        watched: HashSet::new(),
        max_callback_us,
        max_dispatch_delay_us,
    })
}

fn stop_dismissal(monitor: PopupDismissHandle) {
    monitor.hook_thread.stop();
    drop(monitor.sender);
    let _ = monitor.dispatcher.join();
    tracing::info!(
        max_callback_us = monitor.max_callback_us.load(Ordering::Relaxed),
        max_dispatch_delay_us = monitor.max_dispatch_delay_us.load(Ordering::Relaxed),
        "popup mouse hook stopped"
    );
}

fn lock_dismissal() -> std::sync::MutexGuard<'static, Option<PopupDismissHandle>> {
    active_dismissal()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn current_foreground_key() -> usize {
    hwnd_key(unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() })
}

pub(crate) fn foreground_token() -> Option<lexift_core::ports::magnifier::WindowToken> {
    let key = current_foreground_key();
    (key != 0).then_some(lexift_core::ports::magnifier::WindowToken(key as isize))
}

fn watch_dismissal(hwnd: windows::Win32::Foundation::HWND) -> lexift_core::Result<()> {
    static NEXT_WATCH: AtomicU64 = AtomicU64::new(1);
    let instance = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
    let started_at = Instant::now();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetPropW(
            hwnd,
            windows::core::w!("Lexift.PopupDismissInstance"),
            Some(windows::Win32::Foundation::HANDLE(instance as usize as _)),
        )
    }
    .map_err(|error| {
        lexift_core::Error::new(format!(
            "Could not identify popup dismissal instance: {error}"
        ))
    })?;
    let mut slot = lock_dismissal();
    if slot.is_none() {
        *slot = Some(start_dismissal()?);
    }
    let monitor = slot.as_mut().expect("dismissal monitor was installed");
    let key = hwnd_key(hwnd);
    if monitor
        .sender
        .send(DismissalEvent::Watch(
            key,
            current_foreground_key(),
            instance,
            started_at,
        ))
        .is_err()
    {
        let stopped = slot.take();
        drop(slot);
        if let Some(stopped) = stopped {
            stop_dismissal(stopped);
        }
        return Err(lexift_core::Error::new("Popup dismissal monitor stopped"));
    }
    monitor.watched.insert(key);
    Ok(())
}

fn unwatch_dismissal(hwnd: windows::Win32::Foundation::HWND) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::RemovePropW(
            hwnd,
            windows::core::w!("Lexift.PopupDismissInstance"),
        );
    }
    let monitor = {
        let mut slot = lock_dismissal();
        let Some(active) = slot.as_mut() else { return };
        let key = hwnd_key(hwnd);
        active.watched.remove(&key);
        let _ = active.sender.send(DismissalEvent::Unwatch(key));
        if active.watched.is_empty() {
            slot.take()
        } else {
            None
        }
    };
    if let Some(monitor) = monitor {
        stop_dismissal(monitor);
    }
}

pub(crate) fn set_dismissal(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    let hwnd = required_hwnd(window)?;
    if enabled {
        watch_dismissal(hwnd)
    } else {
        unregister_dismissal_hwnd(hwnd);
        Ok(())
    }
}

fn unregister_dismissal_hwnd(hwnd: windows::Win32::Foundation::HWND) {
    unwatch_dismissal(hwnd);
}

/// Escape is observed, never swallowed; only explicitly marked history windows close.
pub(crate) fn set_history_dismissal(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    let hwnd = required_hwnd(window)?;
    unsafe {
        if enabled {
            windows::Win32::UI::WindowsAndMessaging::SetPropW(
                hwnd,
                windows::core::w!("Lexift.ClipboardHistoryEscape"),
                Some(windows::Win32::Foundation::HANDLE(1usize as _)),
            )
            .map_err(|_| lexift_core::Error::new("Could not prepare history cancellation"))?;
        } else {
            let _ = windows::Win32::UI::WindowsAndMessaging::RemovePropW(
                hwnd,
                windows::core::w!("Lexift.ClipboardHistoryEscape"),
            );
        }
    }
    set_dismissal(window, enabled)
}

/// Selection spans several unrelated monitor HWNDs. Only Escape dismisses the group.
pub(crate) fn set_ocr_dismissal(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    let hwnd = required_hwnd(window)?;
    unsafe {
        if enabled {
            windows::Win32::UI::WindowsAndMessaging::SetPropW(
                hwnd,
                windows::core::w!("Lexift.OcrSelection"),
                Some(windows::Win32::Foundation::HANDLE(1usize as _)),
            )
            .map_err(|_| lexift_core::Error::new("Could not prepare OCR cancellation"))?;
        } else {
            let _ = windows::Win32::UI::WindowsAndMessaging::RemovePropW(
                hwnd,
                windows::core::w!("Lexift.OcrSelection"),
            );
        }
    }
    set_history_dismissal(window, enabled)
}
fn is_ocr_selection(hwnd: windows::Win32::Foundation::HWND) -> bool {
    !unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetPropW(
            hwnd,
            windows::core::w!("Lexift.OcrSelection"),
        )
    }
    .0
    .is_null()
}
unsafe extern "system" fn history_focus_hook(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    event_time: u32,
) {
    POPUP_HOOK_STATE.with(|s| {
        if let Some(s) = s.borrow().as_ref() {
            let _ = s.sender.send(DismissalEvent::Focus(
                hwnd_key(hwnd),
                event_instant(
                    Instant::now(),
                    unsafe { windows::Win32::System::SystemInformation::GetTickCount() },
                    event_time,
                ),
            ));
        }
    });
}
unsafe extern "system" fn history_keyboard_hook(
    code: i32,
    w: windows::Win32::Foundation::WPARAM,
    l: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code == HC_ACTION as i32 && matches!(w.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) && l.0 != 0 {
        let data = unsafe { &*(l.0 as *const KBDLLHOOKSTRUCT) };
        if data.vkCode == u32::from(windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0) {
            POPUP_HOOK_STATE.with(|s| {
                if let Some(s) = s.borrow().as_ref() {
                    let _ = s.sender.send(DismissalEvent::Escape(Instant::now()));
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}

unsafe extern "system" fn popup_mouse_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
        WM_XBUTTONDOWN,
    };
    let started = Instant::now();
    if code == HC_ACTION as i32
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
        && lparam.0 != 0
    {
        let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        POPUP_HOOK_STATE.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                let _ = state.sender.send(DismissalEvent::MouseDown(
                    data.pt.x,
                    data.pt.y,
                    Instant::now(),
                ));
            }
        });
    }
    POPUP_HOOK_STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            state
                .max_callback_us
                .fetch_max(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
    });
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn popup_foreground_hook(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    _object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    event_time: u32,
) {
    POPUP_HOOK_STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            let _ = state.sender.send(DismissalEvent::Foreground(
                hwnd_key(hwnd),
                event_instant(
                    Instant::now(),
                    unsafe { windows::Win32::System::SystemInformation::GetTickCount() },
                    event_time,
                ),
            ));
        }
    });
}

// WinEvent callbacks can arrive after a new popup is registered. Preserve the OS event time.
fn event_instant(now: Instant, ticks: u32, event_ticks: u32) -> Instant {
    now.checked_sub(std::time::Duration::from_millis(
        ticks.wrapping_sub(event_ticks) as u64,
    ))
    .unwrap_or(now)
}

fn post_dismissals_for_outside_point(
    monitor: &mut PopupDismissMonitor,
    x: i32,
    y: i32,
    sent_at: Instant,
) {
    use windows::Win32::{Foundation::RECT, UI::WindowsAndMessaging::GetWindowRect};
    for (key, watch) in &mut monitor.watches {
        if watch.dismissal_posted || sent_at < watch.started_at {
            continue;
        }
        let hwnd = hwnd_from_key(*key);
        if is_ocr_selection(hwnd) {
            continue;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()
            && !point_inside_rect(x, y, rect.left, rect.top, rect.right, rect.bottom)
        {
            post_dismissal(hwnd, watch, PopupDismissCause::PointerOutside);
        }
    }
}

fn post_dismissals_for_foreground(
    monitor: &mut PopupDismissMonitor,
    foreground: usize,
    sent_at: Instant,
) {
    for (key, watch) in &mut monitor.watches {
        if is_ocr_selection(hwnd_from_key(*key)) {
            continue;
        }
        if sent_at < watch.started_at {
            continue;
        }
        if watch.dismissal_posted || *key == foreground {
            if *key == foreground {
                watch.foreground_changed = true;
            }
            continue;
        }
        if should_dismiss_for_foreground(watch, foreground) {
            post_dismissal(
                hwnd_from_key(*key),
                watch,
                PopupDismissCause::ForegroundChanged,
            );
        }
    }
}

fn should_dismiss_for_foreground(watch: &mut PopupDismissWatch, foreground: usize) -> bool {
    if foreground == watch.initial_foreground && !watch.foreground_changed {
        return false;
    }
    watch.foreground_changed = true;
    true
}

fn post_dismissal(
    hwnd: windows::Win32::Foundation::HWND,
    watch: &mut PopupDismissWatch,
    cause: PopupDismissCause,
) {
    if !dismissal_instance_matches(hwnd, watch.instance) {
        return;
    }
    if unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(hwnd),
            POPUP_DISMISS_MESSAGE,
            windows::Win32::Foundation::WPARAM(dismissal_code(cause)),
            windows::Win32::Foundation::LPARAM(watch.instance as isize),
        )
    }
    .is_ok()
    {
        watch.dismissal_posted = true;
    }
}

fn dismissal_instance_matches(hwnd: windows::Win32::Foundation::HWND, instance: u64) -> bool {
    instance != 0
        && unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetPropW(
                hwnd,
                windows::core::w!("Lexift.PopupDismissInstance"),
            )
        }
        .0 as usize
            == instance as usize
}

fn dismissal_code(cause: PopupDismissCause) -> usize {
    match cause {
        PopupDismissCause::Escape => 2,
        PopupDismissCause::PointerOutside => 0,
        PopupDismissCause::ForegroundChanged => 1,
    }
}
fn dismissal_cause(code: usize) -> Option<PopupDismissCause> {
    match code {
        2 => Some(PopupDismissCause::Escape),
        0 => Some(PopupDismissCause::PointerOutside),
        1 => Some(PopupDismissCause::ForegroundChanged),
        _ => None,
    }
}

fn point_inside_rect(x: i32, y: i32, left: i32, top: i32, right: i32, bottom: i32) -> bool {
    x >= left && x < right && y >= top && y < bottom
}

fn hwnd_key(hwnd: windows::Win32::Foundation::HWND) -> usize {
    hwnd.0 as usize
}

fn hwnd_from_key(key: usize) -> windows::Win32::Foundation::HWND {
    windows::Win32::Foundation::HWND(key as *mut core::ffi::c_void)
}

const BEGIN_MOVE_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x54;

struct PopupInputBridge {
    handler: PopupPointerHandler,
    last_position: (f32, f32),
    pressed: bool,
    tracking_leave: bool,
    resize: Option<PopupResizeSession>,
    activate_on_click: bool,
    pending_move: Option<usize>,
    move_handoff: bool,
}

/// A resize request handed to the Windows sizing loop.
///
/// The platform remembers only which handle started it; geometry stays with Windows and the UI.
#[derive(Clone, Copy)]
struct PopupResizeSession {
    edge: PopupResizeEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PopupWindowRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl PopupWindowRect {
    fn from_rect(rect: windows::Win32::Foundation::RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

impl PopupResizeEdge {
    fn hit_test_code(self) -> isize {
        use windows::Win32::UI::WindowsAndMessaging::{
            HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        };
        match self {
            Self::Left => HTLEFT as isize,
            Self::Right => HTRIGHT as isize,
            Self::Top => HTTOP as isize,
            Self::Bottom => HTBOTTOM as isize,
            Self::TopLeft => HTTOPLEFT as isize,
            Self::TopRight => HTTOPRIGHT as isize,
            Self::BottomLeft => HTBOTTOMLEFT as isize,
            Self::BottomRight => HTBOTTOMRIGHT as isize,
        }
    }
}

fn scale_dip_to_physical(dip: f32, dpi: u32) -> i32 {
    ((dip * dpi.max(96) as f32) / 96.0).round().max(1.0) as i32
}

fn native_top_resize_hit_test(rect: PopupWindowRect, x: i32, y: i32, dpi: u32) -> bool {
    let strip = scale_dip_to_physical(8.0, dpi);
    let corner = scale_dip_to_physical(36.0, dpi);
    x >= rect.left.saturating_add(corner)
        && x < rect.right.saturating_sub(corner)
        && y >= rect.top
        && y < rect.top.saturating_add(strip)
}

impl PopupInputBridge {
    fn new(handler: PopupPointerHandler, activate_on_click: bool) -> Self {
        Self {
            handler,
            last_position: (0.0, 0.0),
            pressed: false,
            tracking_leave: false,
            resize: None,
            activate_on_click,
            pending_move: None,
            move_handoff: false,
        }
    }

    fn replace_handler(&mut self, handler: PopupPointerHandler, activate_on_click: bool) {
        self.handler = handler;
        self.last_position = (0.0, 0.0);
        self.pressed = false;
        self.tracking_leave = false;
        self.resize = None;
        self.activate_on_click = activate_on_click;
        self.pending_move = None;
        self.move_handoff = false;
    }
}

fn install_pointer_bridge(
    window: &impl raw_window_handle::HasWindowHandle,
    pointer_handler: PopupPointerHandler,
    activate_on_click: bool,
) -> lexift_core::Result<()> {
    use windows::{
        Win32::{
            Foundation::HANDLE,
            UI::{
                Shell::SetWindowSubclass,
                WindowsAndMessaging::{GetPropW, RemovePropW, SetPropW},
            },
        },
        core::w,
    };

    let hwnd = required_hwnd(window)?;
    let existing = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if !existing.0.is_null() {
        clear_popup_resize(hwnd, false);
        let bridge = unsafe { &mut *(existing.0 as *mut PopupInputBridge) };
        bridge.replace_handler(pointer_handler, activate_on_click);
        return Ok(());
    }

    let bridge_ptr = Box::into_raw(Box::new(PopupInputBridge::new(
        pointer_handler,
        activate_on_click,
    )));
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.PopupInputBridge"),
            Some(HANDLE(bridge_ptr.cast())),
        )
    } {
        unsafe { drop(Box::from_raw(bridge_ptr)) };
        tracing::warn!(%error, "translation popup pointer state could not be attached to its HWND");
        return Err(lexift_core::Error::new(
            "Could not attach the popup native pointer bridge state",
        ));
    }
    if !unsafe {
        SetWindowSubclass(
            hwnd,
            Some(popup_input_subclass),
            POPUP_INPUT_SUBCLASS_ID,
            bridge_ptr as usize,
        )
    }
    .as_bool()
    {
        if unsafe { RemovePropW(hwnd, w!("Lexift.PopupInputBridge")) }.is_ok() {
            unsafe { drop(Box::from_raw(bridge_ptr)) };
        } else {
            tracing::warn!(
                "translation popup pointer state could not be detached after subclass failure"
            );
        }
        return Err(lexift_core::Error::new(
            "Could not install the popup native pointer bridge",
        ));
    }
    Ok(())
}

unsafe extern "system" fn popup_input_subclass(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    subclass_id: usize,
    reference_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::{HANDLE, LRESULT, POINT},
        Graphics::Gdi::ScreenToClient,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
                VK_LBUTTON,
            },
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::{
                GetPropW, MA_ACTIVATE, RemovePropW, WM_CANCELMODE, WM_CAPTURECHANGED,
                WM_EXITSIZEMOVE, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
                WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCHITTEST,
                WM_NCLBUTTONDOWN, WM_SHOWWINDOW, WM_SIZE,
            },
        },
    };
    use windows::core::w;

    if reference_data == 0 {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    let _input_timing = input_timing::begin(hwnd, message);
    if message == BEGIN_MOVE_MESSAGE {
        let bridge = unsafe { &mut *(reference_data as *mut PopupInputBridge) };
        if bridge.pending_move != Some(wparam.0) {
            return LRESULT(0);
        }
        let valid = async_key_is_pressed(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) })
            && unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetCapture() }.is_invalid()
            && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) }.as_bool();
        bridge.pending_move = None;
        if valid {
            return unsafe {
                DefSubclassProc(
                    hwnd,
                    WM_NCLBUTTONDOWN,
                    windows::Win32::Foundation::WPARAM(
                        windows::Win32::UI::WindowsAndMessaging::HTCAPTION as usize,
                    ),
                    lparam,
                )
            };
        }
        return LRESULT(0);
    }
    if message == WM_SHOWWINDOW && wparam.0 == 0 {
        unsafe { &mut *(reference_data as *mut PopupInputBridge) }.pending_move = None;
    }

    if message == WM_NCHITTEST {
        let popup = unsafe { GetPropW(hwnd, w!("Lexift.TranslationPopup")) };
        if !popup.0.is_null() {
            let mut rect = windows::Win32::Foundation::RECT::default();
            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect) }
                .is_ok()
            {
                let (x, y) = client_position(lparam.0);
                if native_top_resize_hit_test(
                    PopupWindowRect::from_rect(rect),
                    x as i32,
                    y as i32,
                    window_dpi(hwnd),
                ) {
                    return LRESULT(windows::Win32::UI::WindowsAndMessaging::HTTOP as isize);
                }
            }
        }
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }

    if message == WM_NCLBUTTONDOWN {
        let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
        if !state.0.is_null() {
            let popup = unsafe { GetPropW(hwnd, w!("Lexift.TranslationPopup")) };
            let top_press = !popup.0.is_null()
                && wparam.0 == windows::Win32::UI::WindowsAndMessaging::HTTOP as usize;
            let left_down = async_key_is_pressed(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) });
            if top_press
                && unsafe { &*(state.0 as *const PopupInputBridge) }
                    .resize
                    .is_none()
            {
                // The UI arms its resize session before DefWindowProc enters the sizing loop.
                let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
                (bridge.handler)(PopupPointerEvent::NativeTopResizeRequested);
            }
            let armed = unsafe { &*(state.0 as *const PopupInputBridge) }.resize;
            match armed {
                Some(resize) if wparam.0 == resize.edge.hit_test_code() as usize && !left_down => {
                    // The press that requested this resize is already over.
                    cancel_popup_resize(hwnd);
                    return LRESULT(0);
                }
                None if top_press => return LRESULT(0),
                _ => {}
            }
        }
    }

    if message == WM_EXITSIZEMOVE {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        finish_popup_resize(hwnd);
        return result;
    }

    if message == WM_SHOWWINDOW && wparam.0 == 0 && has_active_popup_resize(hwnd) {
        clear_popup_resize(hwnd, false);
    }

    if message == WM_CANCELMODE && has_active_popup_resize(hwnd) {
        clear_popup_resize(hwnd, true);
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }

    // Let the Windows sizing loop own geometry and pointer tracking. Do not
    // write a competing preferred size while the native loop is active.
    if message == WM_MOUSEMOVE {
        let bridge = unsafe { &mut *(reference_data as *mut PopupInputBridge) };
        if !bridge.tracking_leave {
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            if unsafe { TrackMouseEvent(&mut tracking) }.is_ok() {
                bridge.tracking_leave = true;
            }
        }
        let (x, y) = client_position(lparam.0);
        bridge.last_position = (x, y);
        (bridge.handler)(PopupPointerEvent::Moved { x, y });
        return LRESULT(0);
    }

    if message == WM_SIZE {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        let width = (lparam.0 as u16) as f32;
        let height = ((lparam.0 >> 16) as u16) as f32;
        {
            let bridge = unsafe { &mut *(reference_data as *mut PopupInputBridge) };
            (bridge.handler)(PopupPointerEvent::Resized { width, height });
        }
        return result;
    }

    let bridge = unsafe { &mut *(reference_data as *mut PopupInputBridge) };
    match message {
        POPUP_DISMISS_MESSAGE => {
            if dismissal_instance_matches(hwnd, lparam.0 as u64)
                && let Some(cause) = dismissal_cause(wparam.0)
            {
                (bridge.handler)(PopupPointerEvent::DismissRequested { cause });
            }
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => {
            if bridge.activate_on_click {
                activate_for_pointer_input(hwnd);
                return LRESULT(MA_ACTIVATE as isize);
            }
            return LRESULT(windows::Win32::UI::WindowsAndMessaging::MA_NOACTIVATE as isize);
        }
        0x02A3 => {
            bridge.tracking_leave = false;
            (bridge.handler)(PopupPointerEvent::Exited);
            return LRESULT(0);
        }
        // CS_DBLCLKS substitutes this message for the second down in a rapid click pair.
        // Slint receives the same press/release sequence and handles click counts itself.
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            bridge.pending_move = None;
            if bridge.activate_on_click {
                activate_for_pointer_input(hwnd);
            }
            let _ = unsafe { SetCapture(hwnd) };
            let (x, y) = client_position(lparam.0);
            bridge.last_position = (x, y);
            bridge.pressed = true;
            (bridge.handler)(PopupPointerEvent::Moved { x, y });
            (bridge.handler)(PopupPointerEvent::LeftPressed { x, y });
            return LRESULT(0);
        }
        windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN => {
            let (x, y) = client_position(lparam.0);
            (bridge.handler)(PopupPointerEvent::RightPressed { x, y });
            return LRESULT(0);
        }
        windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP => {
            let (x, y) = client_position(lparam.0);
            (bridge.handler)(PopupPointerEvent::RightReleased { x, y });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            bridge.pending_move = None;
            let (x, y) = client_position(lparam.0);
            bridge.last_position = (x, y);
            (bridge.handler)(PopupPointerEvent::Moved { x, y });
            if bridge.pressed {
                bridge.pressed = false;
                (bridge.handler)(PopupPointerEvent::LeftReleased { x, y });
            }
            release_owned_capture(hwnd);
            return LRESULT(0);
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            if message == WM_CANCELMODE || !bridge.move_handoff {
                bridge.pending_move = None;
            }
            if bridge.pressed {
                bridge.pressed = false;
                let (x, y) = bridge.last_position;
                (bridge.handler)(PopupPointerEvent::LeftCancelled { x, y });
            }
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let (screen_x, screen_y) = client_position(lparam.0);
            let mut point = POINT {
                x: screen_x as i32,
                y: screen_y as i32,
            };
            if unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
                let delta = wheel_delta_physical(wparam.0, window_dpi(hwnd));
                let (delta_x, delta_y) = if message == WM_MOUSEHWHEEL {
                    (delta, 0.0)
                } else {
                    (0.0, delta)
                };
                let (x, y) = (point.x as f32, point.y as f32);
                bridge.last_position = (x, y);
                (bridge.handler)(PopupPointerEvent::Scrolled {
                    x,
                    y,
                    delta_x,
                    delta_y,
                });
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            unregister_dismissal_hwnd(hwnd);
            clear_popup_resize(hwnd, false);
            let _ = unsafe { RemovePropW(hwnd, w!("Lexift.TranslationPopup")) };
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(popup_input_subclass), subclass_id) };
            let removed = unsafe { RemovePropW(hwnd, w!("Lexift.PopupInputBridge")) };
            if let Ok(HANDLE(pointer)) = removed
                && pointer != reference_data as *mut core::ffi::c_void
            {
                tracing::warn!("translation popup HWND stored an unexpected pointer bridge");
            }
            unsafe { drop(Box::from_raw(reference_data as *mut PopupInputBridge)) };
            return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn activate_for_pointer_input(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::{SetActiveWindow, SetFocus},
            WindowsAndMessaging::{
                BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
                SetForegroundWindow,
            },
        },
    };

    unsafe {
        let foreground = GetForegroundWindow();
        let current_thread = GetCurrentThreadId();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, true).as_bool();
        if let Err(error) = BringWindowToTop(hwnd) {
            tracing::debug!(%error, "translation popup could not be raised for pointer input");
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            tracing::debug!("Windows deferred translation popup foreground activation");
        }
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }
}

fn client_position(lparam: isize) -> (f32, f32) {
    (
        (lparam as u16) as i16 as f32,
        ((lparam >> 16) as u16) as i16 as f32,
    )
}

fn wheel_delta_physical(wparam: usize, dpi: u32) -> f32 {
    let raw_delta = ((wparam >> 16) as u16) as i16 as f32;
    raw_delta / WHEEL_DELTA * LOGICAL_SCROLL_PIXELS_PER_NOTCH * dpi.max(96) as f32 / 96.0
}

fn window_dpi(hwnd: windows::Win32::Foundation::HWND) -> u32 {
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    unsafe { GetDpiForWindow(hwnd) }.max(96)
}

pub(crate) fn activate_user_requested(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    let hwnd = required_hwnd(window)?;
    shell_policy::allow_activation_if_registered(hwnd);
    configure_extended_style(window, activation_extended_style)?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        BringWindowToTop(hwnd)
            .map_err(|_| lexift_core::Error::new("Could not raise the requested window"))?;
        if !SetForegroundWindow(hwnd).as_bool() {
            return Err(lexift_core::Error::new(
                "Could not activate the requested window",
            ));
        }
    }
    Ok(())
}

/// Release only this HWND's capture; a late up from A must not cancel B.
fn release_owned_capture(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture};
    if unsafe { GetCapture() } == hwnd {
        let _ = unsafe { ReleaseCapture() };
    }
}
pub(crate) fn cancel_interaction(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::{
        Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{GetPropW, SendMessageW, WM_CANCELMODE},
        },
        core::w,
    };
    let hwnd = required_hwnd(window)?;
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if !state.0.is_null() {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        bridge.pending_move = None;
        bridge.move_handoff = false;
    }
    // No Rust state borrow crosses the synchronous cancellation message.
    unsafe { SendMessageW(hwnd, WM_CANCELMODE, Some(WPARAM(0)), Some(LPARAM(0))) };
    release_owned_capture(hwnd);
    Ok(())
}
pub(crate) fn begin_drag(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::{
        Win32::{
            Foundation::{LPARAM, POINT, WPARAM},
            UI::{
                Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, VK_LBUTTON},
                WindowsAndMessaging::{GetCursorPos, GetPropW, PostMessageW},
            },
        },
        core::w,
    };
    let hwnd = required_hwnd(window)?;
    if unsafe { GetCapture() } != hwnd
        || !async_key_is_pressed(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) })
    {
        return Ok(());
    }
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return Ok(());
    }
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }
        .map_err(|_| lexift_core::Error::new("Could not read the pointer position"))?;
    static NEXT_MOVE: AtomicU64 = AtomicU64::new(1);
    let token = NEXT_MOVE.fetch_add(1, Ordering::Relaxed) as usize;
    {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        if !bridge.pressed {
            return Ok(());
        }
        bridge.pending_move = Some(token);
        bridge.move_handoff = true;
    }
    release_owned_capture(hwnd);
    // ReleaseCapture invokes the bridge synchronously; fetch its still-owned state again.
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return Ok(());
    }
    unsafe { &mut *(state.0 as *mut PopupInputBridge) }.move_handoff = false;
    if let Err(error) = unsafe {
        PostMessageW(
            Some(hwnd),
            BEGIN_MOVE_MESSAGE,
            WPARAM(token),
            LPARAM(pack_screen_position(cursor.x, cursor.y)),
        )
    } {
        unsafe { &mut *(state.0 as *mut PopupInputBridge) }.pending_move = None;
        return Err(lexift_core::Error::new(format!(
            "Could not begin moving the window: {error}"
        )));
    }
    Ok(())
}

pub(crate) fn prepare_resize_tracking(
    window: &impl raw_window_handle::HasWindowHandle,
    edge: PopupResizeEdge,
) -> lexift_core::Result<bool> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    let hwnd = required_hwnd(window)?;
    unsafe {
        if !async_key_is_pressed(GetAsyncKeyState(VK_LBUTTON.0 as i32)) {
            return Ok(false);
        }
        ensure_native_snap_styles(hwnd)?;
        set_resize_state(hwnd, PopupResizeSession { edge })?;
    }
    Ok(true)
}

pub(crate) fn cancel_resize_tracking(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    clear_popup_resize(required_hwnd(window)?, false);
    Ok(())
}

fn has_active_popup_resize(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    !state.0.is_null()
        && unsafe { &*(state.0 as *const PopupInputBridge) }
            .resize
            .is_some()
}

/// Reports the final client size to the UI once Windows leaves the sizing loop.
///
/// The UI owns the matching resize state, so every armed session must end with exactly one report
/// even when the press never changed the window geometry. Dropping the session silently would
/// leave the UI believing a resize is still in flight and refuse every later handle press.
fn finish_popup_resize(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{GetClientRect, GetPropW},
    };
    use windows::core::w;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    let Some(_resize) = bridge.resize.take() else {
        return;
    };

    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        (bridge.handler)(PopupPointerEvent::ResizeFinished {
            width: (client.right - client.left) as f32,
            height: (client.bottom - client.top) as f32,
        });
    }
}

fn cancel_popup_resize(hwnd: windows::Win32::Foundation::HWND) {
    clear_popup_resize(hwnd, true);
}

fn clear_popup_resize(hwnd: windows::Win32::Foundation::HWND, notify_ui: bool) {
    use windows::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{GetClientRect, GetPropW},
    };
    use windows::core::w;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    let Some(_resize) = bridge.resize.take() else {
        return;
    };
    if !notify_ui {
        return;
    }
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        (bridge.handler)(PopupPointerEvent::ResizeFinished {
            width: (client.right - client.left) as f32,
            height: (client.bottom - client.top) as f32,
        });
    }
}

fn set_resize_state(
    hwnd: windows::Win32::Foundation::HWND,
    resize: PopupResizeSession,
) -> lexift_core::Result<()> {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return Err(lexift_core::Error::new("Popup resize state is unavailable"));
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    if bridge.resize.is_some() {
        return Err(lexift_core::Error::new(
            "Popup native resizing is already active",
        ));
    }
    bridge.resize = Some(resize);
    Ok(())
}

fn async_key_is_pressed(state: i16) -> bool {
    state < 0
}

fn pack_screen_position(x: i32, y: i32) -> isize {
    let x = x as i16 as u16 as u32;
    let y = y as i16 as u16 as u32;
    ((y << 16) | x) as isize
}

fn configure_extended_style(
    window: &impl raw_window_handle::HasWindowHandle,
    transform: fn(isize) -> isize,
) -> lexift_core::Result<()> {
    let hwnd = required_hwnd(window)?;
    configure_hwnd_extended_style(hwnd, transform)
}

fn configure_hwnd_extended_style(
    hwnd: windows::Win32::Foundation::HWND,
    transform: fn(isize) -> isize,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
        },
    };
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let updated_style = transform(style);
        if updated_style != style {
            SetLastError(WIN32_ERROR(0));
            let previous = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, updated_style);
            if previous == 0 && GetLastError().0 != 0 {
                return Err(lexift_core::Error::new(
                    "Could not configure tool window activation policy",
                ));
            }
        }
        SetWindowPos(hwnd, None, 0, 0, 0, 0, passive_refresh_flags()).map_err(|_| {
            lexift_core::Error::new("Could not refresh tool window activation policy")
        })?;
    }
    Ok(())
}

fn popup_native_snap_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{WS_MAXIMIZEBOX, WS_THICKFRAME};

    style | WS_THICKFRAME.0 as isize | WS_MAXIMIZEBOX.0 as isize
}

fn ensure_native_snap_styles(hwnd: windows::Win32::Foundation::HWND) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos},
    };

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let updated_style = popup_native_snap_style(style);
        if updated_style == style {
            return Ok(());
        }
        SetLastError(WIN32_ERROR(0));
        let previous = SetWindowLongPtrW(hwnd, GWL_STYLE, updated_style);
        if previous == 0 && GetLastError().0 != 0 {
            return Err(lexift_core::Error::new(
                "Could not enable native resizing and snap behavior for the translation popup",
            ));
        }
        SetWindowPos(hwnd, None, 0, 0, 0, 0, passive_refresh_flags()).map_err(|_| {
            lexift_core::Error::new("Could not refresh translation popup native resize styles")
        })?;
    }
    Ok(())
}

pub(super) fn required_hwnd(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<windows::Win32::Foundation::HWND> {
    window_hwnd(window)?
        .ok_or_else(|| lexift_core::Error::new("Win32 window handle is unavailable"))
}

fn window_hwnd(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<Option<windows::Win32::Foundation::HWND>> {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;
    let handle = window
        .window_handle()
        .map_err(|_| lexift_core::Error::new("Window handle is unavailable"))?;
    Ok(match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut core::ffi::c_void)),
        _ => None,
    })
}

fn passive_extended_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE;
    shell_policy::tool_style(style) | WS_EX_NOACTIVATE.0 as isize
}

/// Disable input without WS_EX_LAYERED, which hides Slint's rendered client area on Windows.
/// The per-HWND guard preserves this policy when Winit rewrites its window styles on show.
pub(crate) fn configure_hint_input(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
        UI::Shell::SetWindowSubclass,
    };
    let hwnd = required_hwnd(window)?;
    unsafe {
        if !SetWindowSubclass(hwnd, Some(hint_input_proc), HINT_INPUT_SUBCLASS, 0).as_bool() {
            return Err(lexift_core::Error::new(
                "Could not protect hint input policy",
            ));
        }
        let _ = EnableWindow(hwnd, false);
        if IsWindowEnabled(hwnd).as_bool() {
            return Err(lexift_core::Error::new("Could not disable hint input"));
        }
    }
    Ok(())
}
const HINT_INPUT_SUBCLASS: usize = 0x4c584849;
unsafe extern "system" fn hint_input_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::LRESULT,
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::*,
        },
    };
    unsafe {
        match message {
            WM_STYLECHANGING if wparam.0 as i32 == GWL_STYLE.0 => {
                let styles = lparam.0 as *mut STYLESTRUCT;
                if !styles.is_null() {
                    (*styles).styleNew |= WS_DISABLED.0;
                }
            }
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(hint_input_proc), HINT_INPUT_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

pub(crate) fn set_click_through(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT;
    let hwnd = required_hwnd(window)?;
    super::annotation_window_order::canvas(hwnd).map_err(|error| {
        lexift_core::Error::new(format!(
            "Could not register annotation canvas order: {error}"
        ))
    })?;
    if enabled {
        configure_hwnd_extended_style(hwnd, |style| style | WS_EX_TRANSPARENT.0 as isize)
    } else {
        configure_hwnd_extended_style(hwnd, |style| style & !(WS_EX_TRANSPARENT.0 as isize))
    }
}

/// UpdateLayeredWindow owns the annotation pixels; Slint only owns input and window lifetime.
/// Its OpenGL swap chain does not reliably advance a transparent layered HWND during a drag.
pub(crate) fn present_annotation_frame(
    window: &impl raw_window_handle::HasWindowHandle,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{COLORREF, POINT, RECT, SIZE},
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ,
            SelectObject,
        },
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, ULW_ALPHA, UpdateLayeredWindow,
            WS_EX_LAYERED,
        },
    };

    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| lexift_core::Error::new("Annotation frame is too large"))?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(lexift_core::Error::new("Annotation frame size is invalid"));
    }
    let hwnd = required_hwnd(window)?;
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    if style & WS_EX_LAYERED.0 as isize == 0 {
        configure_hwnd_extended_style(hwnd, |style| style | WS_EX_LAYERED.0 as isize)?;
    }

    let mut bounds = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut bounds) }
        .map_err(|_| lexift_core::Error::new("Could not position annotation canvas"))?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|_| lexift_core::Error::new("Could not allocate annotation bitmap"))?;
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.0.is_null() {
        let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
        return Err(lexift_core::Error::new("Could not allocate annotation DC"));
    }
    let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
    let destination = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), expected) };
    for (source, output) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(destination.as_chunks_mut::<4>().0.iter_mut())
    {
        output[0] = source[2];
        output[1] = source[1];
        output[2] = source[0];
        // A visually transparent pixel still needs to receive mouse input in edit mode.
        output[3] = source[3].max(1);
    }
    let origin = POINT {
        x: bounds.left,
        y: bounds.top,
    };
    let size = SIZE {
        cx: width as i32,
        cy: height as i32,
    };
    let source = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let result = unsafe {
        UpdateLayeredWindow(
            hwnd,
            None,
            Some(&origin),
            Some(&size),
            Some(dc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    };
    unsafe {
        SelectObject(dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
    }
    result.map_err(|_| lexift_core::Error::new("Could not present annotation frame"))
}

// Normal application windows must keep their taskbar/Alt+Tab identity when raised.
fn activation_extended_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE;
    style & !(WS_EX_NOACTIVATE.0 as isize)
}

fn interactive_extended_style(style: isize) -> isize {
    shell_policy::tool_style(activation_extended_style(style))
}

fn passive_refresh_flags() -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };
    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER
}

#[cfg(test)]
mod tests {
    use std::time::Instant;
    use std::{cell::Cell, rc::Rc};

    use crate::{PopupPointerEvent, PopupResizeEdge};

    use super::{
        PopupDismissWatch, PopupInputBridge, PopupResizeSession, PopupWindowRect,
        async_key_is_pressed, client_position, configure_passive, interactive_extended_style,
        native_top_resize_hit_test, pack_screen_position, passive_extended_style,
        passive_refresh_flags, point_inside_rect, popup_native_snap_style, scale_dip_to_physical,
        should_dismiss_for_foreground, start_dismissal, stop_dismissal, wheel_delta_physical,
    };
    use crate::PassiveToolWindowPreparation;
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    #[test]
    #[ignore = "installs a global mouse hook on an interactive Windows desktop"]
    fn popup_dismissal_hook_can_stop_and_restart() {
        for _ in 0..2 {
            stop_dismissal(start_dismissal().unwrap());
        }
    }

    #[test]
    fn dismissal_message_preserves_reason_through_the_native_bridge() {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SetPropW, WINDOW_EX_STYLE, WS_POPUP,
        };
        struct Native(HWND);
        impl Drop for Native {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let native = Native(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                windows::core::w!("Popup instance test"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        unsafe {
            SetPropW(
                native.0,
                windows::core::w!("Lexift.PopupDismissInstance"),
                Some(windows::Win32::Foundation::HANDLE(1usize as _)),
            )
            .unwrap();
        }
        let received = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = received.clone();
        let mut bridge =
            PopupInputBridge::new(Box::new(move |event| sink.borrow_mut().push(event)), false);
        for cause in [
            crate::PopupDismissCause::PointerOutside,
            crate::PopupDismissCause::ForegroundChanged,
        ] {
            unsafe {
                super::popup_input_subclass(
                    native.0,
                    super::POPUP_DISMISS_MESSAGE,
                    WPARAM(super::dismissal_code(cause)),
                    LPARAM(1),
                    super::POPUP_INPUT_SUBCLASS_ID,
                    &mut bridge as *mut PopupInputBridge as usize,
                );
            }
        }
        unsafe {
            super::popup_input_subclass(
                native.0,
                super::POPUP_DISMISS_MESSAGE,
                WPARAM(0),
                LPARAM(2),
                super::POPUP_INPUT_SUBCLASS_ID,
                &mut bridge as *mut PopupInputBridge as usize,
            );
            super::popup_input_subclass(
                native.0,
                super::POPUP_DISMISS_MESSAGE,
                WPARAM(usize::MAX),
                LPARAM(1),
                super::POPUP_INPUT_SUBCLASS_ID,
                &mut bridge as *mut PopupInputBridge as usize,
            );
        }
        assert_eq!(
            *received.borrow(),
            vec![
                PopupPointerEvent::DismissRequested {
                    cause: crate::PopupDismissCause::PointerOutside
                },
                PopupPointerEvent::DismissRequested {
                    cause: crate::PopupDismissCause::ForegroundChanged
                },
            ]
        );
        received.borrow_mut().clear();
        unsafe {
            super::popup_input_subclass(
                native.0,
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDBLCLK,
                WPARAM(0),
                LPARAM(0),
                super::POPUP_INPUT_SUBCLASS_ID,
                &mut bridge as *mut PopupInputBridge as usize,
            );
            super::popup_input_subclass(
                native.0,
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP,
                WPARAM(0),
                LPARAM(0),
                super::POPUP_INPUT_SUBCLASS_ID,
                &mut bridge as *mut PopupInputBridge as usize,
            );
        }
        assert_eq!(
            *received.borrow(),
            [
                PopupPointerEvent::Moved { x: 0., y: 0. },
                PopupPointerEvent::LeftPressed { x: 0., y: 0. },
                PopupPointerEvent::Moved { x: 0., y: 0. },
                PopupPointerEvent::LeftReleased { x: 0., y: 0. }
            ]
        );
        assert!(!bridge.pressed);
    }

    #[test]
    fn tray_menu_style_excludes_taskbar_without_changing_activation_or_other_flags() {
        use windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW;
        let unrelated = 0x80000; // WS_EX_LAYERED
        for activation in [0, WS_EX_NOACTIVATE.0 as isize] {
            let original = WS_EX_APPWINDOW.0 as isize | unrelated | activation;
            let menu = super::menu_window_extended_style(original);
            assert_eq!(menu & WS_EX_APPWINDOW.0 as isize, 0);
            assert_ne!(menu & WS_EX_TOOLWINDOW.0 as isize, 0);
            assert_eq!(menu & unrelated, unrelated);
            assert_eq!(menu & WS_EX_NOACTIVATE.0 as isize, activation);
            assert_eq!(super::menu_window_extended_style(menu), menu);
            assert_eq!(
                interactive_extended_style(menu) & WS_EX_APPWINDOW.0 as isize,
                0
            );
        }
    }

    #[test]
    fn physical_wheel_delta_tracks_window_dpi() {
        for (dpi, expected) in [(96, 60.0), (120, 75.0), (144, 90.0), (192, 120.0)] {
            assert_eq!(wheel_delta_physical((120usize) << 16, dpi), expected);
        }
        assert_eq!(
            wheel_delta_physical(((-120i16) as u16 as usize) << 16, 120),
            -75.0
        );
    }

    #[test]
    fn client_coordinates_preserve_signed_values() {
        let packed = ((-20i16 as u16 as isize) << 16) | (-10i16 as u16 as isize);
        assert_eq!(client_position(packed), (-10.0, -20.0));
    }

    #[test]
    fn drag_screen_coordinates_preserve_multi_monitor_positions() {
        for (x, y) in [(123, 456), (-1920, -240), (32767, -32768)] {
            let packed = pack_screen_position(x, y);
            let unpacked_x = packed as u16 as i16 as i32;
            let unpacked_y = ((packed as u32 >> 16) as u16) as i16 as i32;
            assert_eq!((unpacked_x, unpacked_y), (x, y));
        }
    }

    #[test]
    fn drag_only_starts_while_the_left_button_is_still_pressed() {
        assert!(async_key_is_pressed(i16::MIN));
        assert!(!async_key_is_pressed(0));
        assert!(!async_key_is_pressed(1));
    }

    #[test]
    fn native_resize_edges_map_to_the_matching_win32_hit_test() {
        use windows::Win32::UI::WindowsAndMessaging::{
            HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        };

        let expected = [
            (PopupResizeEdge::Left, HTLEFT),
            (PopupResizeEdge::Right, HTRIGHT),
            (PopupResizeEdge::Top, HTTOP),
            (PopupResizeEdge::Bottom, HTBOTTOM),
            (PopupResizeEdge::TopLeft, HTTOPLEFT),
            (PopupResizeEdge::TopRight, HTTOPRIGHT),
            (PopupResizeEdge::BottomLeft, HTBOTTOMLEFT),
            (PopupResizeEdge::BottomRight, HTBOTTOMRIGHT),
        ];
        for (edge, hit_test) in expected {
            assert_eq!(edge.hit_test_code(), hit_test as isize, "{edge:?}");
        }
    }

    #[test]
    fn popup_adds_native_resize_and_snap_styles_without_dropping_existing_styles() {
        use windows::Win32::UI::WindowsAndMessaging::{
            WS_CAPTION, WS_MAXIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
        };

        let original = WS_SYSMENU.0 as isize;
        let updated = popup_native_snap_style(original);
        assert_eq!(updated & original, original);
        assert_ne!(updated & WS_THICKFRAME.0 as isize, 0);
        assert_ne!(updated & WS_MAXIMIZEBOX.0 as isize, 0);
        assert_eq!(updated & WS_CAPTION.0 as isize, 0);
    }

    #[test]
    fn only_the_middle_of_the_popup_top_strip_uses_native_hit_testing() {
        let rect = PopupWindowRect {
            left: 100,
            top: 200,
            right: 600,
            bottom: 600,
        };
        assert!(native_top_resize_hit_test(rect, 350, 204, 96));
        assert!(!native_top_resize_hit_test(rect, 350, 208, 96));
        assert!(!native_top_resize_hit_test(rect, 120, 204, 96));
        assert!(!native_top_resize_hit_test(rect, 580, 204, 96));
        assert!(native_top_resize_hit_test(rect, 350, 210, 144));
        assert!(!native_top_resize_hit_test(rect, 350, 212, 144));
    }

    #[test]
    fn native_hit_test_strip_scales_with_monitor_dpi() {
        for (dpi, expected_pixels) in [(96, 8), (120, 10), (144, 12), (192, 16)] {
            assert_eq!(scale_dip_to_physical(8.0, dpi), expected_pixels);
        }
    }

    #[test]
    fn popup_bounds_include_edges_except_the_exclusive_bottom_right() {
        assert!(point_inside_rect(10, 20, 10, 20, 110, 120));
        assert!(point_inside_rect(109, 119, 10, 20, 110, 120));
        assert!(!point_inside_rect(110, 119, 10, 20, 110, 120));
        assert!(!point_inside_rect(109, 120, 10, 20, 110, 120));
        assert!(!point_inside_rect(-1, 50, 10, 20, 110, 120));
    }

    #[test]
    fn dismissal_events_before_registration_do_not_modify_new_watch() {
        let now = Instant::now();
        let mut monitor = super::PopupDismissMonitor {
            watches: std::collections::HashMap::new(),
        };
        monitor.watch(123, 456, 7, now);
        let old = now - std::time::Duration::from_millis(1);
        super::post_dismissals_for_foreground(&mut monitor, 789, old);
        super::post_dismissals_for_outside_point(&mut monitor, 0, 0, old);
        let watch = &monitor.watches[&123];
        assert!(!watch.foreground_changed);
        assert!(!watch.dismissal_posted);
        assert_eq!(
            super::event_instant(now, 5, u32::MAX - 4),
            now - std::time::Duration::from_millis(10)
        );
    }

    #[test]
    fn initial_foreground_is_ignored_until_a_real_transition() {
        let mut watch = PopupDismissWatch {
            instance: 1,
            started_at: Instant::now(),
            initial_foreground: 10,
            foreground_changed: false,
            dismissal_posted: false,
        };

        assert!(!should_dismiss_for_foreground(&mut watch, 10));
        assert!(!watch.foreground_changed);
        assert!(should_dismiss_for_foreground(&mut watch, 20));
        assert!(watch.foreground_changed);
    }

    #[test]
    fn returning_to_the_initial_window_after_popup_activation_dismisses() {
        let mut watch = PopupDismissWatch {
            instance: 1,
            started_at: Instant::now(),
            initial_foreground: 10,
            foreground_changed: true,
            dismissal_posted: false,
        };

        assert!(should_dismiss_for_foreground(&mut watch, 10));
    }

    #[test]
    fn unavailable_native_window_is_pending_instead_of_failed() {
        struct UnavailableWindow;

        impl raw_window_handle::HasWindowHandle for UnavailableWindow {
            fn window_handle(
                &self,
            ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
            {
                Err(raw_window_handle::HandleError::Unavailable)
            }
        }

        assert_eq!(
            configure_passive(&UnavailableWindow).unwrap(),
            PassiveToolWindowPreparation::Pending
        );
    }

    #[test]
    fn not_yet_supported_native_window_is_pending_instead_of_failed() {
        struct NotSupportedWindow;

        impl raw_window_handle::HasWindowHandle for NotSupportedWindow {
            fn window_handle(
                &self,
            ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
            {
                Err(raw_window_handle::HandleError::NotSupported)
            }
        }

        assert_eq!(
            configure_passive(&NotSupportedWindow).unwrap(),
            PassiveToolWindowPreparation::Pending
        );
    }

    #[test]
    fn replacing_a_bridge_handler_resets_transient_pointer_state() {
        let old_calls = Rc::new(Cell::new(0));
        let old_calls_for_handler = Rc::clone(&old_calls);
        let mut bridge = PopupInputBridge::new(
            Box::new(move |_| {
                old_calls_for_handler.set(old_calls_for_handler.get() + 1);
            }),
            true,
        );
        bridge.last_position = (42.0, 21.0);
        bridge.pressed = true;
        bridge.tracking_leave = true;
        bridge.resize = Some(PopupResizeSession {
            edge: PopupResizeEdge::TopLeft,
        });

        let new_calls = Rc::new(Cell::new(0));
        let new_calls_for_handler = Rc::clone(&new_calls);
        bridge.replace_handler(
            Box::new(move |_| {
                new_calls_for_handler.set(new_calls_for_handler.get() + 1);
            }),
            true,
        );
        (bridge.handler)(PopupPointerEvent::Exited);

        assert_eq!(old_calls.get(), 0);
        assert_eq!(new_calls.get(), 1);
        assert_eq!(bridge.last_position, (0.0, 0.0));
        assert!(!bridge.pressed);
        assert!(!bridge.tracking_leave);
        assert!(bridge.resize.is_none());
    }

    #[test]
    fn activating_settings_preserves_its_shell_identity() {
        use windows::Win32::UI::WindowsAndMessaging::{WS_EX_APPWINDOW, WS_EX_LAYERED};
        let original = (WS_EX_APPWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED).0 as isize;
        let activated = super::activation_extended_style(original);
        assert_ne!(activated & WS_EX_APPWINDOW.0 as isize, 0);
        assert_ne!(activated & WS_EX_LAYERED.0 as isize, 0);
        assert_eq!(activated & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_eq!(activated & WS_EX_NOACTIVATE.0 as isize, 0);
    }

    #[test]
    fn passive_windows_do_not_activate_or_enter_the_task_switcher() {
        let style = passive_extended_style(0x100);
        assert_ne!(style & WS_EX_NOACTIVATE.0 as isize, 0);
        assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_ne!(style & 0x100, 0);
        let flags = passive_refresh_flags();
        assert!(flags.contains(SWP_NOACTIVATE));
        assert!(flags.contains(SWP_FRAMECHANGED));
    }

    #[test]
    fn completed_passive_show_accepts_clicks_but_keeps_tool_window_behavior() {
        let interactive = interactive_extended_style(passive_extended_style(0x100));
        assert_eq!(interactive & WS_EX_NOACTIVATE.0 as isize, 0);
        assert_ne!(interactive & WS_EX_TOOLWINDOW.0 as isize, 0);
    }
}

#[cfg(test)]
mod passive_style_regression {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
    use windows::{
        Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
        core::w,
    };
    struct OwnedWindow(HWND);
    impl HasWindowHandle for OwnedWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
            let raw =
                Win32WindowHandle::new(std::num::NonZeroIsize::new(self.0.0 as isize).unwrap());
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(raw)) })
        }
    }
    impl Drop for OwnedWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }
    fn capture_window() -> OwnedWindow {
        OwnedWindow(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Lexift capture regression"),
                    WS_POPUP,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        )
    }
    fn bridge(window: &OwnedWindow) -> *mut PopupInputBridge {
        unsafe { GetPropW(window.0, w!("Lexift.PopupInputBridge")) }.0 as *mut PopupInputBridge
    }
    #[test]
    fn older_window_up_cancel_and_stale_move_never_release_another_windows_capture() {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::Input::KeyboardAndMouse::{GetCapture, SetCapture},
        };
        let a = capture_window();
        let b = capture_window();
        enable_passive_interaction(&a, Box::new(|_| {})).unwrap();
        enable_passive_interaction(&b, Box::new(|_| {})).unwrap();
        unsafe {
            SetCapture(a.0);
            SetCapture(b.0);
        }
        assert_eq!(unsafe { GetCapture() }, b.0);
        unsafe {
            (*bridge(&b)).pending_move = Some(2);
            SendMessageW(a.0, WM_LBUTTONUP, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        assert_eq!(
            unsafe { GetCapture() },
            b.0,
            "late up from A must not release B"
        );
        cancel_interaction(&a).unwrap();
        assert_eq!(unsafe { GetCapture() }, b.0);
        assert_eq!(unsafe { (*bridge(&b)).pending_move }, Some(2));
        unsafe {
            (*bridge(&a)).pending_move = Some(7);
            SendMessageW(a.0, BEGIN_MOVE_MESSAGE, Some(WPARAM(6)), Some(LPARAM(0)));
        }
        assert_eq!(
            unsafe { (*bridge(&a)).pending_move },
            Some(7),
            "stale request must not consume a newer move"
        );
        unsafe {
            (*bridge(&a)).move_handoff = true;
            SendMessageW(
                a.0,
                WM_CAPTURECHANGED,
                Some(WPARAM(0)),
                Some(LPARAM(b.0.0 as isize)),
            );
        }
        assert_eq!(
            unsafe { (*bridge(&a)).pending_move },
            Some(7),
            "self handoff retains the armed request"
        );
        unsafe {
            (*bridge(&a)).move_handoff = false;
            SendMessageW(
                a.0,
                WM_CAPTURECHANGED,
                Some(WPARAM(0)),
                Some(LPARAM(b.0.0 as isize)),
            );
        }
        assert_eq!(unsafe { (*bridge(&a)).pending_move }, None);
        assert_eq!(unsafe { GetCapture() }, b.0);
        cancel_interaction(&b).unwrap();
        assert!(unsafe { GetCapture() }.is_invalid());
        assert_eq!(unsafe { (*bridge(&b)).pending_move }, None);
        let handle = b.0;
        drop(b);
        assert!(!unsafe { IsWindow(Some(handle)) }.as_bool());
        assert!(unsafe { IsWindow(Some(a.0)) }.as_bool());
    }
    #[test]
    fn hints_keep_disabled_input_through_backend_rewrites_without_layering() {
        use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
        let foreground = unsafe { GetForegroundWindow() };
        let window = OwnedWindow(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Lexift hint regression"),
                    WS_POPUP,
                    0,
                    0,
                    96,
                    28,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        );
        configure_passive(&window).unwrap();
        configure_hint_input(&window).unwrap();
        assert!(!unsafe { IsWindowEnabled(window.0) }.as_bool());
        for style in [WS_POPUP.0, WS_POPUP.0 | WS_VISIBLE.0] {
            unsafe { SetWindowLongPtrW(window.0, GWL_STYLE, style as isize) };
            assert!(!unsafe { IsWindowEnabled(window.0) }.as_bool());
            assert_eq!(
                unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) } & WS_EX_LAYERED.0 as isize,
                0
            );
        }
        let hit = unsafe {
            SendMessageW(
                window.0,
                WM_NCHITTEST,
                Some(windows::Win32::Foundation::WPARAM(0)),
                Some(windows::Win32::Foundation::LPARAM(0)),
            )
        };
        assert_eq!(hit.0, HTTRANSPARENT as isize);
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }

    #[test]
    fn backend_style_rewrites_preserve_passive_policy_until_explicit_interactive_transition() {
        let foreground = unsafe { GetForegroundWindow() };
        let window = OwnedWindow(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Lexift passive style regression"),
                    WS_POPUP,
                    0,
                    0,
                    96,
                    48,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        );
        assert_eq!(
            configure_passive(&window).unwrap(),
            PassiveToolWindowPreparation::Ready
        );
        // First show can rewrite styles before pointer forwarding is installed.
        unsafe { SetWindowLongPtrW(window.0, GWL_EXSTYLE, WS_EX_APPWINDOW.0 as isize) };
        assert_eq!(
            unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) } & WS_EX_APPWINDOW.0 as isize,
            0
        );
        enable_passive_interaction(&window, Box::new(|_| {})).unwrap();
        for backend_style in [0, WS_EX_APPWINDOW.0 as isize, WS_EX_LAYERED.0 as isize] {
            unsafe { SetWindowLongPtrW(window.0, GWL_EXSTYLE, backend_style) };
            let style = unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) };
            assert_ne!(style & WS_EX_NOACTIVATE.0 as isize, 0);
            assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
            assert_eq!(style & WS_EX_APPWINDOW.0 as isize, 0);
            assert_eq!(
                style & (backend_style & !(WS_EX_APPWINDOW.0 as isize)),
                backend_style & !(WS_EX_APPWINDOW.0 as isize)
            );
        }
        enable_interaction_without_activation(&window, Box::new(|_| {})).unwrap();
        assert_eq!(
            unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) } & WS_EX_NOACTIVATE.0 as isize,
            0
        );
        for backend_style in [
            0,
            WS_EX_APPWINDOW.0 as isize,
            (WS_EX_APPWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT).0 as isize,
        ] {
            unsafe { SetWindowLongPtrW(window.0, GWL_EXSTYLE, backend_style) };
            let style = unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) };
            assert_eq!(style & WS_EX_NOACTIVATE.0 as isize, 0);
            assert_eq!(style & WS_EX_APPWINDOW.0 as isize, 0);
            assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
            let retained = backend_style & !(WS_EX_APPWINDOW.0 as isize);
            assert_eq!(style & retained, retained);
        }
        enable_passive_interaction(&window, Box::new(|_| {})).unwrap();
        unsafe { SetWindowLongPtrW(window.0, GWL_EXSTYLE, 0) };
        assert_ne!(
            unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) } & WS_EX_NOACTIVATE.0 as isize,
            0
        );
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
    }
}
