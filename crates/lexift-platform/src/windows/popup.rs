use std::{cell::RefCell, collections::HashMap};

use crate::{
    PassiveToolWindowPreparation, PopupPointerEvent, PopupPointerHandler, PopupResizeEdge,
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
    configure_hwnd_extended_style(hwnd, passive_extended_style)?;
    Ok(PassiveToolWindowPreparation::Ready)
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
    configure_extended_style(window, interactive_extended_style)?;
    install_pointer_bridge(window, pointer_handler)
}

pub(crate) fn attach_owner(
    child: &impl raw_window_handle::HasWindowHandle,
    owner: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW},
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
    }
    Ok(())
}

const POPUP_INPUT_SUBCLASS_ID: usize = 0x4C58_4654;
const POPUP_DISMISS_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x4C;
const WHEEL_DELTA: f32 = 120.0;
const LOGICAL_SCROLL_PIXELS_PER_NOTCH: f32 = 60.0;

thread_local! {
    static POPUP_DISMISS_MONITOR: RefCell<Option<PopupDismissMonitor>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug)]
struct PopupDismissWatch {
    initial_foreground: usize,
    foreground_changed: bool,
    dismissal_posted: bool,
}

struct PopupDismissMonitor {
    watches: HashMap<usize, PopupDismissWatch>,
    mouse_hook: windows::Win32::UI::WindowsAndMessaging::HHOOK,
    foreground_hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
}

impl PopupDismissMonitor {
    fn install() -> lexift_core::Result<Self> {
        use windows::Win32::UI::{
            Accessibility::SetWinEventHook,
            WindowsAndMessaging::{
                EVENT_SYSTEM_FOREGROUND, SetWindowsHookExW, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT,
            },
        };

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
            let _ =
                unsafe { windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(mouse_hook) };
            return Err(lexift_core::Error::new(
                "Could not monitor foreground changes for popup",
            ));
        }
        Ok(Self {
            watches: HashMap::new(),
            mouse_hook,
            foreground_hook,
        })
    }

    fn watch(&mut self, hwnd: windows::Win32::Foundation::HWND) {
        let initial_foreground =
            hwnd_key(unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() });
        self.watches.insert(
            hwnd_key(hwnd),
            PopupDismissWatch {
                initial_foreground,
                foreground_changed: false,
                dismissal_posted: false,
            },
        );
    }

    fn unwatch(&mut self, hwnd: windows::Win32::Foundation::HWND) {
        self.watches.remove(&hwnd_key(hwnd));
    }
}

impl Drop for PopupDismissMonitor {
    fn drop(&mut self) {
        use windows::Win32::UI::{
            Accessibility::UnhookWinEvent, WindowsAndMessaging::UnhookWindowsHookEx,
        };
        if let Err(error) = unsafe { UnhookWindowsHookEx(self.mouse_hook) } {
            tracing::debug!(%error, "translation popup mouse monitor could not be removed");
        }
        if !unsafe { UnhookWinEvent(self.foreground_hook) }.as_bool() {
            tracing::debug!("translation popup foreground monitor could not be removed");
        }
    }
}

pub(crate) fn set_dismissal(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    let hwnd = required_hwnd(window)?;
    if enabled {
        POPUP_DISMISS_MONITOR.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(PopupDismissMonitor::install()?);
            }
            slot.as_mut()
                .expect("dismiss monitor was installed")
                .watch(hwnd);
            Ok(())
        })
    } else {
        unregister_dismissal_hwnd(hwnd);
        Ok(())
    }
}

fn unregister_dismissal_hwnd(hwnd: windows::Win32::Foundation::HWND) {
    POPUP_DISMISS_MONITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(monitor) = slot.as_mut() {
            monitor.unwatch(hwnd);
            if monitor.watches.is_empty() {
                *slot = None;
            }
        }
    });
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
    if code == HC_ACTION as i32
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
        && lparam.0 != 0
    {
        let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        post_dismissals_for_outside_point(data.pt.x, data.pt.y);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn popup_foreground_hook(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    _object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    post_dismissals_for_foreground(hwnd);
}

fn post_dismissals_for_outside_point(x: i32, y: i32) {
    use windows::Win32::{Foundation::RECT, UI::WindowsAndMessaging::GetWindowRect};
    POPUP_DISMISS_MONITOR.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return;
        };
        let Some(monitor) = slot.as_mut() else {
            return;
        };
        for (key, watch) in &mut monitor.watches {
            if watch.dismissal_posted {
                continue;
            }
            let hwnd = hwnd_from_key(*key);
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()
                && !point_inside_rect(x, y, rect.left, rect.top, rect.right, rect.bottom)
            {
                post_dismissal(hwnd, watch);
            }
        }
    });
}

fn post_dismissals_for_foreground(foreground: windows::Win32::Foundation::HWND) {
    let foreground = hwnd_key(foreground);
    POPUP_DISMISS_MONITOR.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return;
        };
        let Some(monitor) = slot.as_mut() else {
            return;
        };
        for (key, watch) in &mut monitor.watches {
            if watch.dismissal_posted || *key == foreground {
                if *key == foreground {
                    watch.foreground_changed = true;
                }
                continue;
            }
            if should_dismiss_for_foreground(watch, foreground) {
                post_dismissal(hwnd_from_key(*key), watch);
            }
        }
    });
}

fn should_dismiss_for_foreground(watch: &mut PopupDismissWatch, foreground: usize) -> bool {
    if foreground == watch.initial_foreground && !watch.foreground_changed {
        return false;
    }
    watch.foreground_changed = true;
    true
}

fn post_dismissal(hwnd: windows::Win32::Foundation::HWND, watch: &mut PopupDismissWatch) {
    if unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(hwnd),
            POPUP_DISMISS_MESSAGE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        )
    }
    .is_ok()
    {
        watch.dismissal_posted = true;
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

struct PopupInputBridge {
    handler: PopupPointerHandler,
    last_position: (f32, f32),
    pressed: bool,
    tracking_leave: bool,
    resize: Option<PopupResizeSession>,
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
    fn new(handler: PopupPointerHandler) -> Self {
        Self {
            handler,
            last_position: (0.0, 0.0),
            pressed: false,
            tracking_leave: false,
            resize: None,
        }
    }

    fn replace_handler(&mut self, handler: PopupPointerHandler) {
        self.handler = handler;
        self.last_position = (0.0, 0.0);
        self.pressed = false;
        self.tracking_leave = false;
        self.resize = None;
    }
}

fn install_pointer_bridge(
    window: &impl raw_window_handle::HasWindowHandle,
    pointer_handler: PopupPointerHandler,
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
        bridge.replace_handler(pointer_handler);
        return Ok(());
    }

    let bridge_ptr = Box::into_raw(Box::new(PopupInputBridge::new(pointer_handler)));
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
                GetAsyncKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT,
                TrackMouseEvent, VK_LBUTTON,
            },
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::{
                GetPropW, MA_ACTIVATE, RemovePropW, WM_CANCELMODE, WM_CAPTURECHANGED,
                WM_EXITSIZEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN,
                WM_SHOWWINDOW, WM_SIZE,
            },
        },
    };
    use windows::core::w;

    if reference_data == 0 {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
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
            (bridge.handler)(PopupPointerEvent::DismissRequested);
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => {
            activate_for_pointer_input(hwnd);
            return LRESULT(MA_ACTIVATE as isize);
        }
        0x02A3 => {
            bridge.tracking_leave = false;
            (bridge.handler)(PopupPointerEvent::Exited);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            activate_for_pointer_input(hwnd);
            let _ = unsafe { SetCapture(hwnd) };
            let (x, y) = client_position(lparam.0);
            bridge.last_position = (x, y);
            bridge.pressed = true;
            (bridge.handler)(PopupPointerEvent::Moved { x, y });
            (bridge.handler)(PopupPointerEvent::LeftPressed { x, y });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = client_position(lparam.0);
            bridge.last_position = (x, y);
            (bridge.handler)(PopupPointerEvent::Moved { x, y });
            if bridge.pressed {
                bridge.pressed = false;
                (bridge.handler)(PopupPointerEvent::LeftReleased { x, y });
            }
            let _ = unsafe { ReleaseCapture() };
            return LRESULT(0);
        }
        WM_CAPTURECHANGED => {
            if bridge.pressed {
                bridge.pressed = false;
                let (x, y) = bridge.last_position;
                (bridge.handler)(PopupPointerEvent::LeftReleased { x, y });
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
    configure_extended_style(window, interactive_extended_style)?;
    let hwnd = required_hwnd(window)?;
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

pub(crate) fn begin_drag(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{LPARAM, POINT, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_LBUTTON},
            WindowsAndMessaging::{GetCursorPos, HTCAPTION, PostMessageW, WM_NCLBUTTONDOWN},
        },
    };
    let hwnd = required_hwnd(window)?;
    unsafe {
        if !async_key_is_pressed(GetAsyncKeyState(VK_LBUTTON.0 as i32)) {
            return Ok(());
        }
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor)
            .map_err(|_| lexift_core::Error::new("Could not read the pointer position"))?;
        let _ = ReleaseCapture();
        PostMessageW(
            Some(hwnd),
            WM_NCLBUTTONDOWN,
            WPARAM(HTCAPTION as usize),
            LPARAM(pack_screen_position(cursor.x, cursor.y)),
        )
        .map_err(|_| lexift_core::Error::new("Could not begin moving the popup window"))?;
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

fn required_hwnd(
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
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
    style | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize
}

fn interactive_extended_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
    (style & !(WS_EX_NOACTIVATE.0 as isize)) | WS_EX_TOOLWINDOW.0 as isize
}

fn passive_refresh_flags() -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };
    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use crate::{PopupPointerEvent, PopupResizeEdge};

    use super::{
        PopupDismissWatch, PopupInputBridge, PopupResizeSession, PopupWindowRect,
        async_key_is_pressed, client_position, configure_passive, interactive_extended_style,
        native_top_resize_hit_test, pack_screen_position, passive_extended_style,
        passive_refresh_flags, point_inside_rect, popup_native_snap_style, scale_dip_to_physical,
        should_dismiss_for_foreground, wheel_delta_physical,
    };
    use crate::PassiveToolWindowPreparation;
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

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
    fn initial_foreground_is_ignored_until_a_real_transition() {
        let mut watch = PopupDismissWatch {
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
        let mut bridge = PopupInputBridge::new(Box::new(move |_| {
            old_calls_for_handler.set(old_calls_for_handler.get() + 1);
        }));
        bridge.last_position = (42.0, 21.0);
        bridge.pressed = true;
        bridge.tracking_leave = true;
        bridge.resize = Some(PopupResizeSession {
            edge: PopupResizeEdge::TopLeft,
        });

        let new_calls = Rc::new(Cell::new(0));
        let new_calls_for_handler = Rc::clone(&new_calls);
        bridge.replace_handler(Box::new(move |_| {
            new_calls_for_handler.set(new_calls_for_handler.get() + 1);
        }));
        (bridge.handler)(PopupPointerEvent::Exited);

        assert_eq!(old_calls.get(), 0);
        assert_eq!(new_calls.get(), 1);
        assert_eq!(bridge.last_position, (0.0, 0.0));
        assert!(!bridge.pressed);
        assert!(!bridge.tracking_leave);
        assert!(bridge.resize.is_none());
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
