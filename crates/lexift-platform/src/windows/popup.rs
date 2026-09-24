use std::{cell::RefCell, collections::HashMap};

use crate::{
    PassiveToolWindowPreparation, PopupPointerEvent, PopupPointerHandler, PopupResizeBounds,
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

    let hwnd = required_hwnd(window)?;
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
const POPUP_BEGIN_RESIZE_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x4D;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupResizePhase {
    AwaitingNativeStart,
    NativeSizing,
    Completed,
}

#[derive(Clone, Copy)]
struct PopupResizeSession {
    edge: PopupResizeEdge,
    initial_rect: PopupWindowRect,
    bounds: PopupResizeBounds,
    phase: PopupResizePhase,
    size_samples: u32,
    max_fixed_edge_drift: u32,
    frame_sync: PopupResizeFrameSyncStats,
}

#[derive(Clone, Copy, Debug, Default)]
struct PopupResizeFrameSyncStats {
    requested_frames: u32,
    presented_frames: u32,
    dwm_flushed_frames: u32,
    presentation_misses: u32,
    property_failures: u32,
    dwm_flush_failures: u32,
    total_dwm_flush_micros: u64,
    max_dwm_flush_micros: u64,
    generation: u32,
    expected_size: (u32, u32),
    last_presented_size: (u32, u32),
}

#[derive(Clone, Copy)]
struct PopupWindowRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
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

impl PopupResizeSession {
    fn native_start_allowed(self, left_button_down: bool) -> bool {
        self.phase == PopupResizePhase::AwaitingNativeStart && left_button_down
    }

    fn fixed_edge_drift(self, rect: &windows::Win32::Foundation::RECT) -> u32 {
        let drift = |actual: i32, expected: i32| actual.abs_diff(expected);
        match self.edge {
            PopupResizeEdge::Left => drift(rect.right, self.initial_rect.right),
            PopupResizeEdge::Right => drift(rect.left, self.initial_rect.left),
            PopupResizeEdge::Top => drift(rect.bottom, self.initial_rect.bottom),
            PopupResizeEdge::Bottom => drift(rect.top, self.initial_rect.top),
            PopupResizeEdge::TopLeft => drift(rect.right, self.initial_rect.right)
                .max(drift(rect.bottom, self.initial_rect.bottom)),
            PopupResizeEdge::TopRight => drift(rect.left, self.initial_rect.left)
                .max(drift(rect.bottom, self.initial_rect.bottom)),
            PopupResizeEdge::BottomLeft => drift(rect.right, self.initial_rect.right)
                .max(drift(rect.top, self.initial_rect.top)),
            PopupResizeEdge::BottomRight => {
                drift(rect.left, self.initial_rect.left).max(drift(rect.top, self.initial_rect.top))
            }
        }
    }

    fn enter_native_sizing(&mut self) -> bool {
        if self.phase != PopupResizePhase::AwaitingNativeStart {
            return false;
        }
        self.phase = PopupResizePhase::NativeSizing;
        true
    }

    fn complete_native_sizing(&mut self) -> bool {
        if self.phase != PopupResizePhase::NativeSizing {
            return false;
        }
        self.phase = PopupResizePhase::Completed;
        true
    }

    fn needs_frame_sync(self) -> bool {
        self.phase == PopupResizePhase::NativeSizing
    }
}

fn popup_resize_presentation_matches(
    requested_generation: u32,
    requested_size: (u32, u32),
    presented_generation: usize,
    presented_size: (usize, usize),
) -> bool {
    presented_generation == requested_generation as usize
        && presented_size == (requested_size.0 as usize, requested_size.1 as usize)
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
                GetPropW, MA_ACTIVATE, PostMessageW, RemovePropW, WM_CAPTURECHANGED,
                WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY,
                WM_NCLBUTTONDOWN, WM_SHOWWINDOW, WM_SIZE,
            },
        },
    };
    use windows::core::w;

    if reference_data == 0 {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }

    if message == WM_GETMINMAXINFO {
        let default_result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
        if !state.0.is_null() {
            let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
            if let Some(resize) = bridge.resize {
                let info = unsafe {
                    &mut *(lparam.0 as *mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO)
                };
                info.ptMinTrackSize.x = resize.bounds.min_width as i32;
                info.ptMinTrackSize.y = resize.bounds.min_height as i32;
                info.ptMaxTrackSize.x = resize.bounds.max_width as i32;
                info.ptMaxTrackSize.y = resize.bounds.max_height as i32;
                return LRESULT(0);
            }
        }
        return default_result;
    }

    if message == POPUP_BEGIN_RESIZE_MESSAGE {
        let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
        if state.0.is_null() {
            return LRESULT(0);
        }
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        let Some(resize) = bridge.resize else {
            return LRESULT(0);
        };
        if !resize.native_start_allowed(async_key_is_pressed(unsafe {
            GetAsyncKeyState(VK_LBUTTON.0 as i32)
        })) {
            cancel_popup_resize(hwnd);
            return LRESULT(0);
        }
        if let Err(error) = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_NCLBUTTONDOWN,
                windows::Win32::Foundation::WPARAM(resize.edge.hit_test_code() as usize),
                lparam,
            )
        } {
            tracing::warn!(%error, "translation popup native resize could not be started");
            cancel_popup_resize(hwnd);
        }
        return LRESULT(0);
    }

    if message == WM_NCLBUTTONDOWN {
        let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
        if !state.0.is_null()
            && let Some(resize) = unsafe { &*(state.0 as *const PopupInputBridge) }.resize
            && resize.phase == PopupResizePhase::AwaitingNativeStart
            && wparam.0 == resize.edge.hit_test_code() as usize
            && !async_key_is_pressed(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) })
        {
            cancel_popup_resize(hwnd);
            return LRESULT(0);
        }
    }

    if message == WM_ENTERSIZEMOVE {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        mark_native_resize_started(hwnd);
        return result;
    }

    if message == WM_EXITSIZEMOVE {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        finish_popup_resize(hwnd);
        if has_active_popup_resize(hwnd) {
            // A pending request may be superseded before Windows enters its
            // sizing loop. Do not leave the UI resize state armed.
            cancel_popup_resize(hwnd);
        }
        return result;
    }

    if message == WM_SHOWWINDOW && wparam.0 == 0 && has_active_popup_resize(hwnd) {
        clear_popup_resize(hwnd, false);
    }

    // Let the Windows sizing loop own geometry and pointer tracking. In
    // particular, do not resize the transparent Skia surface per WM_MOUSEMOVE:
    // DWM can otherwise show a stale surface clipped to the new HWND bounds.
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
        record_native_resize_sample(hwnd);
        if native_popup_resize_active(hwnd) {
            begin_native_resize_frame(
                hwnd,
                (lparam.0 as u16) as u32,
                ((lparam.0 >> 16) as u16) as u32,
            );
        }
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
        WM_SIZE => {
            let width = (lparam.0 as u16) as f32;
            let height = ((lparam.0 >> 16) as u16) as f32;
            (bridge.handler)(PopupPointerEvent::Resized { width, height });
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
            clear_native_resize_properties(hwnd);
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
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_SIZE && native_popup_resize_active(hwnd) {
        // Winit handles WM_SIZE first and forwards the new dimensions to Slint.
        // Force that resized surface to paint before returning to Windows' modal
        // sizing loop, then wait for DWM to consume the frame before the next
        // native geometry update can overtake it.
        synchronize_native_resize_frame(hwnd);
    }
    result
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

pub(crate) fn begin_resize(
    window: &impl raw_window_handle::HasWindowHandle,
    edge: PopupResizeEdge,
    bounds: PopupResizeBounds,
) -> lexift_core::Result<bool> {
    use windows::Win32::{
        Foundation::{POINT, RECT},
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_LBUTTON},
            WindowsAndMessaging::{GetCursorPos, GetWindowRect, PostMessageW},
        },
    };
    let hwnd = required_hwnd(window)?;
    unsafe {
        if !async_key_is_pressed(GetAsyncKeyState(VK_LBUTTON.0 as i32)) {
            return Ok(false);
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect)
            .map_err(|_| lexift_core::Error::new("Could not read the popup window bounds"))?;
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return Err(lexift_core::Error::new(
                "Could not read the pointer position",
            ));
        }
        set_resize_state(
            hwnd,
            PopupResizeSession {
                edge,
                initial_rect: PopupWindowRect {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                },
                bounds,
                phase: PopupResizePhase::AwaitingNativeStart,
                size_samples: 0,
                max_fixed_edge_drift: 0,
                frame_sync: Default::default(),
            },
        )?;
        // Release Slint's client-area capture first. The private message makes
        // the native non-client sizing request asynchronous and rechecks the
        // button state after any already-queued mouse-up has been dispatched.
        let _ = ReleaseCapture();
        if let Err(error) = PostMessageW(
            Some(hwnd),
            POPUP_BEGIN_RESIZE_MESSAGE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(pack_screen_position(cursor.x, cursor.y)),
        ) {
            clear_popup_resize(hwnd, false);
            return Err(lexift_core::Error::new(format!(
                "Could not queue native popup resizing: {error}"
            )));
        }
    }
    Ok(true)
}

fn has_active_popup_resize(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    !state.0.is_null()
        && unsafe { &*(state.0 as *const PopupInputBridge) }
            .resize
            .is_some()
}

fn mark_native_resize_started(hwnd: windows::Win32::Foundation::HWND) {
    use windows::{
        Win32::{
            Foundation::HANDLE,
            UI::WindowsAndMessaging::{GetPropW, RemovePropW, SetPropW},
        },
        core::w,
    };
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let entered = {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        bridge
            .resize
            .as_mut()
            .is_some_and(|resize| resize.enter_native_sizing())
    };
    if entered {
        let _ = unsafe { RemovePropW(hwnd, w!("Lexift.PopupPresentedGeneration")) };
        let _ = unsafe { RemovePropW(hwnd, w!("Lexift.PopupPresentedWidth")) };
        let _ = unsafe { RemovePropW(hwnd, w!("Lexift.PopupPresentedHeight")) };
        if let Err(error) = unsafe {
            SetPropW(
                hwnd,
                w!("Lexift.PopupNativeResize"),
                Some(HANDLE(std::ptr::dangling_mut::<core::ffi::c_void>())),
            )
        } {
            let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
            if !state.0.is_null()
                && let Some(resize) = unsafe { &mut *(state.0 as *mut PopupInputBridge) }
                    .resize
                    .as_mut()
            {
                resize.frame_sync.property_failures =
                    resize.frame_sync.property_failures.saturating_add(1);
            }
            tracing::error!(%error, "could not enable synchronous popup resize rendering");
        }
    }
}

fn native_popup_resize_active(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    !state.0.is_null()
        && unsafe { &*(state.0 as *const PopupInputBridge) }
            .resize
            .is_some_and(|resize| resize.needs_frame_sync())
}

fn begin_native_resize_frame(hwnd: windows::Win32::Foundation::HWND, width: u32, height: u32) {
    use windows::{
        Win32::{Foundation::HANDLE, UI::WindowsAndMessaging::SetPropW},
        core::w,
    };
    let state = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetPropW(hwnd, w!("Lexift.PopupInputBridge"))
    };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    let Some(resize) = bridge
        .resize
        .as_mut()
        .filter(|resize| resize.needs_frame_sync())
    else {
        return;
    };
    resize.frame_sync.requested_frames = resize.frame_sync.requested_frames.saturating_add(1);
    resize.frame_sync.generation = resize.frame_sync.generation.wrapping_add(1).max(1);
    resize.frame_sync.expected_size = (width, height);
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.PopupResizeGeneration"),
            Some(HANDLE(resize.frame_sync.generation as usize as *mut _)),
        )
    } {
        resize.frame_sync.property_failures = resize.frame_sync.property_failures.saturating_add(1);
        tracing::debug!(%error, "could not mark popup resize generation");
    }
}

fn synchronize_native_resize_frame(hwnd: windows::Win32::Foundation::HWND) {
    use windows::{
        Win32::{Graphics::Dwm::DwmFlush, UI::WindowsAndMessaging::GetPropW},
        core::w,
    };

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let (generation, expected_size) = {
        let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
        let Some(resize) = bridge.resize.filter(|resize| resize.needs_frame_sync()) else {
            return;
        };
        (
            resize.frame_sync.generation,
            resize.frame_sync.expected_size,
        )
    };
    let presented_generation = unsafe { GetPropW(hwnd, w!("Lexift.PopupPresentedGeneration")) };
    let presented_width = unsafe { GetPropW(hwnd, w!("Lexift.PopupPresentedWidth")) };
    let presented_height = unsafe { GetPropW(hwnd, w!("Lexift.PopupPresentedHeight")) };
    let presentation_matches = popup_resize_presentation_matches(
        generation,
        expected_size,
        presented_generation.0 as usize,
        (presented_width.0 as usize, presented_height.0 as usize),
    );

    // The Winit resize-event path now calls draw_with_outcome() before the
    // WM_SIZE dispatch returns. Wait for DWM only after that draw actually
    // swapped a frame for this exact generation and physical client size.
    let flush_start = std::time::Instant::now();
    let flush_succeeded = presentation_matches && unsafe { DwmFlush() }.is_ok();
    let flush_micros = flush_start.elapsed().as_micros().min(u64::MAX as u128) as u64;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    let Some(resize) = bridge
        .resize
        .as_mut()
        .filter(|resize| resize.needs_frame_sync())
    else {
        return;
    };
    if presentation_matches {
        resize.frame_sync.presented_frames = resize.frame_sync.presented_frames.saturating_add(1);
    } else {
        resize.frame_sync.presentation_misses =
            resize.frame_sync.presentation_misses.saturating_add(1);
    }
    if presentation_matches && flush_succeeded {
        resize.frame_sync.dwm_flushed_frames =
            resize.frame_sync.dwm_flushed_frames.saturating_add(1);
        resize.frame_sync.total_dwm_flush_micros = resize
            .frame_sync
            .total_dwm_flush_micros
            .saturating_add(flush_micros);
        resize.frame_sync.max_dwm_flush_micros =
            resize.frame_sync.max_dwm_flush_micros.max(flush_micros);
    } else if presentation_matches {
        resize.frame_sync.dwm_flush_failures =
            resize.frame_sync.dwm_flush_failures.saturating_add(1);
    }
    if presentation_matches {
        resize.frame_sync.last_presented_size = expected_size;
    }
}

fn record_native_resize_sample(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{GetPropW, GetWindowRect},
    };
    use windows::core::w;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    let Some(resize) = bridge.resize.as_mut() else {
        return;
    };
    if resize.phase != PopupResizePhase::NativeSizing {
        return;
    }
    resize.size_samples = resize.size_samples.saturating_add(1);
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
        resize.max_fixed_edge_drift = resize
            .max_fixed_edge_drift
            .max(resize.fixed_edge_drift(&rect));
    }
}

fn finish_popup_resize(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{GetClientRect, GetPropW, GetWindowRect},
    };
    use windows::core::w;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let mut resize = {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        if !bridge
            .resize
            .is_some_and(|resize| resize.phase == PopupResizePhase::NativeSizing)
        {
            return;
        }
        bridge.resize.take().expect("native resize was checked")
    };
    clear_native_resize_properties(hwnd);
    if !resize.complete_native_sizing() {
        return;
    }

    let mut client = RECT::default();
    let size = if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        (
            (client.right - client.left) as f32,
            (client.bottom - client.top) as f32,
        )
    } else {
        (0.0, 0.0)
    };
    let mut outer = RECT::default();
    let final_fixed_edge_drift = if unsafe { GetWindowRect(hwnd, &mut outer) }.is_ok() {
        resize.fixed_edge_drift(&outer)
    } else {
        u32::MAX
    };
    tracing::info!(
        edge = ?resize.edge,
        resize_samples = resize.size_samples,
        max_fixed_edge_drift_px = resize.max_fixed_edge_drift,
        final_fixed_edge_drift_px = final_fixed_edge_drift,
        resize_requests = resize.frame_sync.requested_frames,
        renderer_presented_frames = resize.frame_sync.presented_frames,
        dwm_flushed_frames = resize.frame_sync.dwm_flushed_frames,
        presentation_misses = resize.frame_sync.presentation_misses,
        last_requested_client_size_px = ?resize.frame_sync.expected_size,
        last_presented_client_size_px = ?resize.frame_sync.last_presented_size,
        sync_property_failures = resize.frame_sync.property_failures,
        dwm_flush_failures = resize.frame_sync.dwm_flush_failures,
        total_dwm_flush_us = resize.frame_sync.total_dwm_flush_micros,
        max_dwm_flush_us = resize.frame_sync.max_dwm_flush_micros,
        "translation popup native resize completed"
    );

    let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
    (bridge.handler)(PopupPointerEvent::ResizeFinished {
        width: size.0,
        height: size.1,
    });
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
    clear_native_resize_properties(hwnd);
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    if bridge.resize.take().is_none() || !notify_ui {
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

fn clear_native_resize_properties(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::RemovePropW;
    use windows::core::w;

    for property in [
        w!("Lexift.PopupNativeResize"),
        w!("Lexift.PopupResizeGeneration"),
        w!("Lexift.PopupPresentedGeneration"),
        w!("Lexift.PopupPresentedWidth"),
        w!("Lexift.PopupPresentedHeight"),
    ] {
        let _ = unsafe { RemovePropW(hwnd, property) };
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

    use crate::{PopupPointerEvent, PopupResizeBounds, PopupResizeEdge};

    use super::{
        PopupDismissWatch, PopupInputBridge, PopupResizePhase, PopupResizeSession, PopupWindowRect,
        async_key_is_pressed, client_position, configure_passive, interactive_extended_style,
        pack_screen_position, passive_extended_style, passive_refresh_flags, point_inside_rect,
        popup_resize_presentation_matches, should_dismiss_for_foreground, wheel_delta_physical,
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
    fn native_resize_tracks_the_expected_fixed_edges_in_all_directions() {
        use windows::Win32::Foundation::RECT;

        let initial_rect = PopupWindowRect {
            left: 100,
            top: 200,
            right: 500,
            bottom: 700,
        };
        let expected = [
            (
                PopupResizeEdge::Left,
                RECT {
                    left: 25,
                    top: 200,
                    right: 500,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::Right,
                RECT {
                    left: 100,
                    top: 200,
                    right: 575,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::Top,
                RECT {
                    left: 100,
                    top: 150,
                    right: 500,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::Bottom,
                RECT {
                    left: 100,
                    top: 200,
                    right: 500,
                    bottom: 750,
                },
            ),
            (
                PopupResizeEdge::TopLeft,
                RECT {
                    left: 25,
                    top: 150,
                    right: 500,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::TopRight,
                RECT {
                    left: 100,
                    top: 150,
                    right: 575,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::BottomLeft,
                RECT {
                    left: 25,
                    top: 200,
                    right: 500,
                    bottom: 750,
                },
            ),
            (
                PopupResizeEdge::BottomRight,
                RECT {
                    left: 100,
                    top: 200,
                    right: 575,
                    bottom: 750,
                },
            ),
        ];

        for (edge, actual) in expected {
            let resize = PopupResizeSession {
                edge,
                initial_rect,
                bounds: PopupResizeBounds {
                    min_width: 340,
                    min_height: 336,
                    max_width: 1_000,
                    max_height: 1_000,
                },
                phase: PopupResizePhase::NativeSizing,
                size_samples: 1,
                max_fixed_edge_drift: 0,
                frame_sync: Default::default(),
            };
            assert_eq!(resize.fixed_edge_drift(&actual), 0, "{edge:?}");
        }
    }

    #[test]
    fn native_resize_state_requires_a_pressed_button_and_completes_once() {
        let mut resize = PopupResizeSession {
            edge: PopupResizeEdge::TopLeft,
            initial_rect: PopupWindowRect {
                left: 100,
                top: 200,
                right: 500,
                bottom: 700,
            },
            bounds: PopupResizeBounds {
                min_width: 340,
                min_height: 336,
                max_width: 1_000,
                max_height: 1_000,
            },
            phase: PopupResizePhase::AwaitingNativeStart,
            size_samples: 0,
            max_fixed_edge_drift: 0,
            frame_sync: Default::default(),
        };

        assert!(!resize.native_start_allowed(false));
        assert!(resize.native_start_allowed(true));
        assert!(resize.enter_native_sizing());
        assert_eq!(resize.phase, PopupResizePhase::NativeSizing);
        assert!(resize.needs_frame_sync());
        assert!(!resize.enter_native_sizing());
        assert!(resize.complete_native_sizing());
        assert_eq!(resize.phase, PopupResizePhase::Completed);
        assert!(!resize.needs_frame_sync());
        assert!(!resize.complete_native_sizing());
    }

    #[test]
    fn native_resize_waits_for_a_presented_frame_of_the_requested_size() {
        let generation = 17;
        let requested_size = (1280, 720);

        assert!(popup_resize_presentation_matches(
            generation,
            requested_size,
            17,
            (1280, 720)
        ));
        assert!(!popup_resize_presentation_matches(
            generation,
            requested_size,
            16,
            (1280, 720)
        ));
        assert!(!popup_resize_presentation_matches(
            generation,
            requested_size,
            17,
            (1279, 720)
        ));
        assert!(!popup_resize_presentation_matches(
            generation,
            requested_size,
            17,
            (1280, 719)
        ));
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
            initial_rect: PopupWindowRect {
                left: 10,
                top: 20,
                right: 300,
                bottom: 400,
            },
            bounds: PopupResizeBounds {
                min_width: 340,
                min_height: 336,
                max_width: 1_000,
                max_height: 1_000,
            },
            phase: PopupResizePhase::AwaitingNativeStart,
            size_samples: 3,
            max_fixed_edge_drift: 2,
            frame_sync: Default::default(),
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
