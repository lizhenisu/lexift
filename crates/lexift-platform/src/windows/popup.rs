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
const POPUP_RESIZE_PREVIEW_TIMER_ID: usize = 0x4C58_5250;
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
    resize_bounds: Option<PopupResizeBounds>,
    resize_anchor: Option<PopupResizeAnchor>,
    resize_samples: u32,
    max_proposed_anchor_drift: u32,
    resize_preview_hwnd: Option<windows::Win32::Foundation::HWND>,
    resize_preview_rect: Option<windows::Win32::Foundation::RECT>,
    resize_preview_failed: bool,
}

#[derive(Clone, Copy)]
struct PopupResizeAnchor {
    edge: PopupResizeEdge,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    cursor_x: i32,
    cursor_y: i32,
}

impl PopupResizeAnchor {
    fn resized_rect(
        self,
        cursor_x: i32,
        cursor_y: i32,
        bounds: PopupResizeBounds,
        dpi: u32,
    ) -> windows::Win32::Foundation::RECT {
        use windows::Win32::Foundation::RECT;

        let dx = cursor_x - self.cursor_x;
        let dy = cursor_y - self.cursor_y;
        let mut rect = RECT {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        };
        match self.edge {
            PopupResizeEdge::Left => rect.left += dx,
            PopupResizeEdge::Right => rect.right += dx,
            PopupResizeEdge::Top => rect.top += dy,
            PopupResizeEdge::Bottom => rect.bottom += dy,
            PopupResizeEdge::TopLeft => {
                rect.left += dx;
                rect.top += dy;
            }
            PopupResizeEdge::TopRight => {
                rect.right += dx;
                rect.top += dy;
            }
            PopupResizeEdge::BottomLeft => {
                rect.left += dx;
                rect.bottom += dy;
            }
            PopupResizeEdge::BottomRight => {
                rect.right += dx;
                rect.bottom += dy;
            }
        }

        let physical = |logical: u32| {
            ((u64::from(logical) * u64::from(dpi.max(96)) + 48) / 96).min(i32::MAX as u64) as i32
        };
        let min_width = physical(bounds.min_width).max(1);
        let max_width = physical(bounds.max_width).max(min_width);
        let min_height = physical(bounds.min_height).max(1);
        let max_height = physical(bounds.max_height).max(min_height);
        let width = (rect.right - rect.left).clamp(min_width, max_width);
        let height = (rect.bottom - rect.top).clamp(min_height, max_height);

        if matches!(
            self.edge,
            PopupResizeEdge::Left | PopupResizeEdge::TopLeft | PopupResizeEdge::BottomLeft
        ) {
            rect.left = rect.right - width;
        } else {
            rect.right = rect.left + width;
        }
        if matches!(
            self.edge,
            PopupResizeEdge::Top | PopupResizeEdge::TopLeft | PopupResizeEdge::TopRight
        ) {
            rect.top = rect.bottom - height;
        } else {
            rect.bottom = rect.top + height;
        }
        rect
    }

    fn fixed_edge_drift(self, rect: &windows::Win32::Foundation::RECT) -> u32 {
        let drift = |actual: i32, expected: i32| actual.abs_diff(expected);
        match self.edge {
            PopupResizeEdge::Left => drift(rect.right, self.right),
            PopupResizeEdge::Right => drift(rect.left, self.left),
            PopupResizeEdge::Top => drift(rect.bottom, self.bottom),
            PopupResizeEdge::Bottom => drift(rect.top, self.top),
            PopupResizeEdge::TopLeft => {
                drift(rect.right, self.right).max(drift(rect.bottom, self.bottom))
            }
            PopupResizeEdge::TopRight => {
                drift(rect.left, self.left).max(drift(rect.bottom, self.bottom))
            }
            PopupResizeEdge::BottomLeft => {
                drift(rect.right, self.right).max(drift(rect.top, self.top))
            }
            PopupResizeEdge::BottomRight => {
                drift(rect.left, self.left).max(drift(rect.top, self.top))
            }
        }
    }
}

impl PopupInputBridge {
    fn new(handler: PopupPointerHandler) -> Self {
        Self {
            handler,
            last_position: (0.0, 0.0),
            pressed: false,
            tracking_leave: false,
            resize_bounds: None,
            resize_anchor: None,
            resize_samples: 0,
            max_proposed_anchor_drift: 0,
            resize_preview_hwnd: None,
            resize_preview_rect: None,
            resize_preview_failed: false,
        }
    }

    fn replace_handler(&mut self, handler: PopupPointerHandler) {
        self.handler = handler;
        self.last_position = (0.0, 0.0);
        self.pressed = false;
        self.tracking_leave = false;
        self.resize_bounds = None;
        self.resize_anchor = None;
        self.resize_samples = 0;
        self.max_proposed_anchor_drift = 0;
        self.resize_preview_hwnd = None;
        self.resize_preview_rect = None;
        self.resize_preview_failed = false;
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
        destroy_resize_preview(hwnd);
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
                ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            },
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::{
                GetPropW, MA_ACTIVATE, RemovePropW, WM_CAPTURECHANGED, WM_EXITSIZEMOVE,
                WM_GETMINMAXINFO, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_SHOWWINDOW, WM_SIZE, WM_TIMER,
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
            if bridge.resize_anchor.is_some()
                && let Some(bounds) = bridge.resize_bounds
            {
                let info = unsafe {
                    &mut *(lparam.0 as *mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO)
                };
                info.ptMinTrackSize.x = bounds.min_width as i32;
                info.ptMinTrackSize.y = bounds.min_height as i32;
                info.ptMaxTrackSize.x = bounds.max_width as i32;
                info.ptMaxTrackSize.y = bounds.max_height as i32;
                return LRESULT(0);
            }
        }
        return default_result;
    }

    if message == WM_TIMER && wparam.0 == POPUP_RESIZE_PREVIEW_TIMER_ID {
        destroy_resize_preview(hwnd);
        return LRESULT(0);
    }

    if message == WM_SHOWWINDOW && wparam.0 == 0 && has_active_popup_resize(hwnd) {
        cancel_popup_resize(hwnd);
    }

    // Popup resizing is driven by ordinary captured mouse events instead of
    // WM_NCLBUTTONDOWN. Windows' modal sizing loop can display stale transparent
    // frames while Slint is waiting for the next Winit event-loop iteration.
    if message == WM_MOUSEMOVE {
        let (x, y) = client_position(lparam.0);
        let left_button_down = unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows::Win32::UI::Input::KeyboardAndMouse::VK_LBUTTON.0 as i32,
            )
        } < 0;
        if has_active_popup_resize(hwnd) {
            if left_button_down {
                update_resize_preview(hwnd);
            } else {
                finish_popup_resize(hwnd);
            }
        }

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
        bridge.last_position = (x, y);
        (bridge.handler)(PopupPointerEvent::Moved { x, y });
        return LRESULT(0);
    }

    if message == WM_LBUTTONUP && has_active_popup_resize(hwnd) {
        update_resize_preview(hwnd);
    }
    if message == WM_LBUTTONUP || message == WM_CAPTURECHANGED || message == WM_EXITSIZEMOVE {
        finish_popup_resize(hwnd);
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
            destroy_resize_preview(hwnd);
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

pub(crate) fn begin_resize(
    window: &impl raw_window_handle::HasWindowHandle,
    edge: PopupResizeEdge,
    bounds: PopupResizeBounds,
) -> lexift_core::Result<bool> {
    use windows::Win32::{
        Foundation::{POINT, RECT},
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
            WindowsAndMessaging::{GetCursorPos, GetWindowRect},
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
        let anchor = PopupResizeAnchor {
            edge,
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            cursor_x: 0,
            cursor_y: 0,
        };
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return Err(lexift_core::Error::new(
                "Could not read the pointer position",
            ));
        }
        set_resize_state(
            hwnd,
            bounds,
            PopupResizeAnchor {
                cursor_x: cursor.x,
                cursor_y: cursor.y,
                ..anchor
            },
        )?;
    }
    Ok(true)
}

fn has_active_popup_resize(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    !state.0.is_null()
        && unsafe { &*(state.0 as *const PopupInputBridge) }
            .resize_anchor
            .is_some()
}

fn update_resize_preview(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{Foundation::POINT, UI::HiDpi::GetDpiForWindow};

    let state = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetPropW(
            hwnd,
            windows::core::w!("Lexift.PopupInputBridge"),
        )
    };
    if state.0.is_null() {
        return;
    }
    let (anchor, bounds, preview, failed) = {
        let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
        let (Some(anchor), Some(bounds)) = (bridge.resize_anchor, bridge.resize_bounds) else {
            return;
        };
        (
            anchor,
            bounds,
            bridge.resize_preview_hwnd,
            bridge.resize_preview_failed,
        )
    };

    let mut cursor = POINT::default();
    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut cursor) }.is_err() {
        return;
    }
    let desired = anchor.resized_rect(cursor.x, cursor.y, bounds, unsafe { GetDpiForWindow(hwnd) });
    {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        bridge.resize_samples = bridge.resize_samples.saturating_add(1);
        bridge.resize_preview_rect = Some(desired);
        bridge.max_proposed_anchor_drift = bridge
            .max_proposed_anchor_drift
            .max(anchor.fixed_edge_drift(&desired));
    }
    if failed {
        return;
    }

    let result = match preview {
        Some(preview) => {
            update_resize_preview_window(preview, desired, unsafe { GetDpiForWindow(hwnd) })
        }
        None => create_resize_preview_window(hwnd, desired, unsafe { GetDpiForWindow(hwnd) }).map(
            |preview| {
                let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
                bridge.resize_preview_hwnd = Some(preview);
            },
        ),
    };
    if let Err(error) = result {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        bridge.resize_preview_failed = true;
        if let Some(preview) = bridge.resize_preview_hwnd.take() {
            unsafe { destroy_resize_preview_window(preview) };
        }
        tracing::warn!(%error, "translation popup resize preview could not be updated");
    }
}

fn finish_popup_resize(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{
            GetClientRect, GetPropW, GetWindowRect, SWP_NOACTIVATE, SWP_NOOWNERZORDER,
            SWP_NOZORDER, SetTimer, SetWindowPos,
        },
    };
    use windows::core::w;

    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let desired = {
        let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
        let Some(anchor) = bridge.resize_anchor else {
            return;
        };
        bridge.resize_preview_rect.unwrap_or(RECT {
            left: anchor.left,
            top: anchor.top,
            right: anchor.right,
            bottom: anchor.bottom,
        })
    };

    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            desired.left,
            desired.top,
            desired.right - desired.left,
            desired.bottom - desired.top,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        )
    }
    .map_err(|error| {
        tracing::warn!(%error, "translation popup final resize could not be applied");
    });

    let (anchor, samples, max_anchor_drift, has_preview) = {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        let Some(anchor) = bridge.resize_anchor.take() else {
            return;
        };
        bridge.resize_bounds = None;
        bridge.resize_preview_rect = None;
        let samples = std::mem::take(&mut bridge.resize_samples);
        let max_anchor_drift = std::mem::take(&mut bridge.max_proposed_anchor_drift);
        (
            anchor,
            samples,
            max_anchor_drift,
            bridge.resize_preview_hwnd.is_some(),
        )
    };
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
    let final_anchor_drift = if unsafe { GetWindowRect(hwnd, &mut outer) }.is_ok() {
        anchor.fixed_edge_drift(&outer)
    } else {
        u32::MAX
    };
    tracing::info!(
        edge = ?anchor.edge,
        preview_samples = samples,
        max_preview_anchor_drift_px = max_anchor_drift,
        final_fixed_edge_drift_px = final_anchor_drift,
        "translation popup preview resize completed"
    );

    if has_preview {
        let timer = unsafe { SetTimer(Some(hwnd), POPUP_RESIZE_PREVIEW_TIMER_ID, 96, None) };
        if timer == 0 {
            destroy_resize_preview(hwnd);
        }
    }
    let bridge = unsafe { &*(state.0 as *const PopupInputBridge) };
    (bridge.handler)(PopupPointerEvent::ResizeFinished {
        width: size.0,
        height: size.1,
    });
}

fn cancel_popup_resize(hwnd: windows::Win32::Foundation::HWND) {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    {
        let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
        bridge.resize_bounds = None;
        bridge.resize_anchor = None;
        bridge.resize_preview_rect = None;
        bridge.resize_samples = 0;
        bridge.max_proposed_anchor_drift = 0;
    }
    destroy_resize_preview(hwnd);
}

fn destroy_resize_preview(hwnd: windows::Win32::Foundation::HWND) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{GetPropW, KillTimer},
        core::w,
    };
    let _ = unsafe { KillTimer(Some(hwnd), POPUP_RESIZE_PREVIEW_TIMER_ID) };
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return;
    }
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    bridge.resize_preview_rect = None;
    bridge.resize_preview_failed = false;
    if let Some(preview) = bridge.resize_preview_hwnd.take() {
        unsafe { destroy_resize_preview_window(preview) };
    }
}

fn ensure_resize_preview_class() -> lexift_core::Result<windows::Win32::Foundation::HINSTANCE> {
    use std::sync::OnceLock;
    use windows::Win32::{
        Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{RegisterClassExW, WNDCLASSEXW},
    };
    use windows::core::w;

    static CLASS_REGISTERED: OnceLock<bool> = OnceLock::new();
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|_| lexift_core::Error::new("Could not access the Lexift module"))?;
    let registered = *CLASS_REGISTERED.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            hInstance: HINSTANCE(instance.0),
            lpfnWndProc: Some(resize_preview_window_proc),
            lpszClassName: w!("Lexift.PopupResizePreview"),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassExW(&class) };
        if atom != 0 {
            true
        } else {
            unsafe { GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
        }
    });
    if !registered {
        return Err(lexift_core::Error::new(
            "Could not register the popup resize preview window",
        ));
    }
    Ok(HINSTANCE(instance.0))
}

unsafe extern "system" fn resize_preview_window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::LRESULT,
        UI::WindowsAndMessaging::{DefWindowProcW, MA_NOACTIVATE, WM_NCHITTEST},
    };

    match message {
        WM_NCHITTEST => LRESULT(windows::Win32::UI::WindowsAndMessaging::HTTRANSPARENT as isize),
        windows::Win32::UI::WindowsAndMessaging::WM_MOUSEACTIVATE => {
            LRESULT(MA_NOACTIVATE as isize)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn create_resize_preview_window(
    owner: windows::Win32::Foundation::HWND,
    rect: windows::Win32::Foundation::RECT,
    dpi: u32,
) -> lexift_core::Result<windows::Win32::Foundation::HWND> {
    use windows::Win32::{
        Foundation::HINSTANCE,
        UI::WindowsAndMessaging::{
            CreateWindowExW, SW_SHOWNOACTIVATE, ShowWindow, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    };
    use windows::core::w;

    let instance = ensure_resize_preview_class()?;
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let preview = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("Lexift.PopupResizePreview"),
            w!(""),
            WS_POPUP,
            rect.left,
            rect.top,
            width,
            height,
            Some(owner),
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
    }
    .map_err(|_| lexift_core::Error::new("Could not create the popup resize preview"))?;
    if let Err(error) = update_resize_preview_window(preview, rect, dpi) {
        unsafe { destroy_resize_preview_window(preview) };
        return Err(error);
    }
    let _ = unsafe { ShowWindow(preview, SW_SHOWNOACTIVATE) };
    let _ = unsafe { windows::Win32::Graphics::Gdi::UpdateWindow(preview) };
    Ok(preview)
}

fn update_resize_preview_window(
    preview: windows::Win32::Foundation::HWND,
    rect: windows::Win32::Foundation::RECT,
    dpi: u32,
) -> lexift_core::Result<()> {
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::{COLORREF, POINT, SIZE},
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
            HGDIOBJ, ReleaseDC, SelectObject,
        },
        UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow},
    };

    let width = rect.right.saturating_sub(rect.left).max(1);
    let height = rect.bottom.saturating_sub(rect.top).max(1);
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .filter(|count| *count <= 64 * 1024 * 1024)
        .ok_or_else(|| lexift_core::Error::new("Popup resize preview is too large"))?;

    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.0.is_null() {
        return Err(lexift_core::Error::new(
            "Could not create a device context for the popup resize preview",
        ));
    }
    let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if memory_dc.0.is_null() {
        let _ = unsafe { ReleaseDC(None, screen_dc) };
        return Err(lexift_core::Error::new(
            "Could not create a bitmap context for the popup resize preview",
        ));
    }

    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = unsafe {
        CreateDIBSection(
            Some(screen_dc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    };
    let bitmap = match bitmap {
        Ok(bitmap) if !bits.is_null() => bitmap,
        _ => {
            let _ = unsafe { DeleteDC(memory_dc) };
            let _ = unsafe { ReleaseDC(None, screen_dc) };
            return Err(lexift_core::Error::new(
                "Could not allocate the popup resize preview bitmap",
            ));
        }
    };
    let previous = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    if previous.0.is_null() || previous.0 as isize == -1 {
        let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
        let _ = unsafe { DeleteDC(memory_dc) };
        let _ = unsafe { ReleaseDC(None, screen_dc) };
        return Err(lexift_core::Error::new(
            "Could not select the popup resize preview bitmap",
        ));
    }

    let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixel_count) };
    render_resize_preview_pixels(pixels, width as usize, height as usize, dpi);
    let destination = POINT {
        x: rect.left,
        y: rect.top,
    };
    let size = SIZE {
        cx: width,
        cy: height,
    };
    let source = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let update = unsafe {
        UpdateLayeredWindow(
            preview,
            Some(screen_dc),
            Some(&destination),
            Some(&size),
            Some(memory_dc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    };
    let _ = unsafe { SelectObject(memory_dc, previous) };
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
    let _ = unsafe { DeleteDC(memory_dc) };
    let _ = unsafe { ReleaseDC(None, screen_dc) };
    update.map_err(|_| {
        lexift_core::Error::new("Could not atomically update the popup resize preview")
    })
}

fn render_resize_preview_pixels(pixels: &mut [u32], width: usize, height: usize, dpi: u32) {
    if width == 0 || height == 0 || pixels.len() != width.saturating_mul(height) {
        return;
    }
    pixels.fill(0);
    let scale = dpi.max(96) as f32 / 96.0;
    let border = (2.0 * scale).round().max(1.0);
    let radius = (16.0 * scale).round().min(width.min(height) as f32 / 2.0);
    let inner_radius = (radius - border).max(0.0);
    let alpha = 190.0_f32;

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let outer = rounded_rect_coverage(px, py, width as f32, height as f32, radius);
            if outer <= 0.0 {
                continue;
            }
            let inner = rounded_rect_coverage(
                px - border,
                py - border,
                width as f32 - 2.0 * border,
                height as f32 - 2.0 * border,
                inner_radius,
            );
            let coverage = (outer - inner).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let a = (alpha * coverage).round() as u32;
            let r = 25 * a / 255;
            let g = 103 * a / 255;
            let b = 210 * a / 255;
            pixels[y * width + x] = (a << 24) | (r << 16) | (g << 8) | b;
        }
    }
}

fn rounded_rect_coverage(x: f32, y: f32, width: f32, height: f32, radius: f32) -> f32 {
    if width <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    let radius = radius.clamp(0.0, width.min(height) / 2.0);
    let qx = (x - width / 2.0).abs() - (width / 2.0 - radius);
    let qy = (y - height / 2.0).abs() - (height / 2.0 - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    (0.5 - (outside + inside - radius)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod resize_preview_pixel_tests {
    use super::render_resize_preview_pixels;

    #[test]
    fn outline_bitmap_keeps_all_edges_and_corners_in_one_frame() {
        let (width, height) = (160, 120);
        let mut pixels = vec![u32::MAX; width * height];
        render_resize_preview_pixels(&mut pixels, width, height, 120);

        let visible = |x: usize, y: usize| pixels[y * width + x] >> 24 != 0;
        assert!(visible(width / 2, 0));
        assert!(visible(width / 2, height - 1));
        assert!(visible(0, height / 2));
        assert!(visible(width - 1, height / 2));
        assert!(!visible(width / 2, height / 2));
        assert!(!visible(0, 0));
        assert!(!visible(width - 1, 0));
        assert!(!visible(0, height - 1));
        assert!(!visible(width - 1, height - 1));
    }
}

unsafe fn destroy_resize_preview_window(preview: windows::Win32::Foundation::HWND) {
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(preview) };
}

fn set_resize_state(
    hwnd: windows::Win32::Foundation::HWND,
    bounds: PopupResizeBounds,
    anchor: PopupResizeAnchor,
) -> lexift_core::Result<()> {
    use windows::{Win32::UI::WindowsAndMessaging::GetPropW, core::w};
    let state = unsafe { GetPropW(hwnd, w!("Lexift.PopupInputBridge")) };
    if state.0.is_null() {
        return Err(lexift_core::Error::new("Popup resize state is unavailable"));
    }
    destroy_resize_preview(hwnd);
    let bridge = unsafe { &mut *(state.0 as *mut PopupInputBridge) };
    bridge.resize_bounds = Some(bounds);
    bridge.resize_anchor = Some(anchor);
    bridge.resize_samples = 0;
    bridge.max_proposed_anchor_drift = 0;
    bridge.resize_preview_rect = None;
    bridge.resize_preview_failed = false;
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
        PopupDismissWatch, PopupInputBridge, PopupResizeAnchor, async_key_is_pressed,
        client_position, configure_passive, interactive_extended_style, pack_screen_position,
        passive_extended_style, passive_refresh_flags, point_inside_rect,
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
    fn interactive_resize_preserves_opposite_edges_in_all_directions() {
        use windows::Win32::Foundation::RECT;

        let base = PopupResizeAnchor {
            edge: PopupResizeEdge::Left,
            left: 100,
            top: 200,
            right: 500,
            bottom: 700,
            cursor_x: 300,
            cursor_y: 400,
        };
        let bounds = PopupResizeBounds {
            min_width: 100,
            min_height: 100,
            max_width: 1_000,
            max_height: 1_000,
        };
        let expected = [
            (
                PopupResizeEdge::Left,
                RECT {
                    left: 60,
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
                    right: 460,
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
                    bottom: 650,
                },
            ),
            (
                PopupResizeEdge::TopLeft,
                RECT {
                    left: 60,
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
                    right: 460,
                    bottom: 700,
                },
            ),
            (
                PopupResizeEdge::BottomLeft,
                RECT {
                    left: 60,
                    top: 200,
                    right: 500,
                    bottom: 650,
                },
            ),
            (
                PopupResizeEdge::BottomRight,
                RECT {
                    left: 100,
                    top: 200,
                    right: 460,
                    bottom: 650,
                },
            ),
        ];

        for (edge, expected_rect) in expected {
            let anchor = PopupResizeAnchor { edge, ..base };
            let actual = anchor.resized_rect(260, 350, bounds, 96);
            assert_eq!(
                (actual.left, actual.top, actual.right, actual.bottom),
                (
                    expected_rect.left,
                    expected_rect.top,
                    expected_rect.right,
                    expected_rect.bottom,
                ),
                "{edge:?}"
            );
            assert_eq!(anchor.fixed_edge_drift(&actual), 0, "{edge:?}");
        }
    }

    #[test]
    fn interactive_resize_clamps_logical_bounds_at_window_dpi() {
        use windows::Win32::Foundation::RECT;

        let anchor = PopupResizeAnchor {
            edge: PopupResizeEdge::Left,
            left: 100,
            top: 200,
            right: 500,
            bottom: 700,
            cursor_x: 300,
            cursor_y: 400,
        };
        let bounds = PopupResizeBounds {
            min_width: 340,
            min_height: 300,
            max_width: 600,
            max_height: 500,
        };

        assert_eq!(
            anchor.resized_rect(2_000, 400, bounds, 120),
            RECT {
                left: 75,
                top: 200,
                right: 500,
                bottom: 700,
            }
        );
        assert_eq!(
            anchor.resized_rect(-2_000, 400, bounds, 120),
            RECT {
                left: -250,
                top: 200,
                right: 500,
                bottom: 700,
            }
        );
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
        bridge.resize_preview_rect = Some(windows::Win32::Foundation::RECT {
            left: 10,
            top: 20,
            right: 300,
            bottom: 400,
        });
        bridge.resize_preview_failed = true;

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
        assert!(bridge.resize_preview_rect.is_none());
        assert!(!bridge.resize_preview_failed);
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
