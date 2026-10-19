//! HWND-owned borderless screen surface and first-paint screenshot bootstrap.
//! No OCR policy lives here; a future region-selection host can reuse this surface.
use lexift_core::{Error, Result, ports::screen_capture::CapturedRegion};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::{Dwm::DwmFlush, Gdi::*},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::w,
};
use winit::platform::windows::WindowExtWindows;
const SUBCLASS: usize = 0x4c58_5253;
const HANDOFF: u32 = WM_APP + 0x52;
const FEEDBACK: u32 = WM_APP + 0x53;
static NEXT_SURFACE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
struct Surface {
    instance: usize,
    group: Option<(u64, usize)>,
    width: i32,
    height: i32,
    bootstrap: Option<Vec<u8>>,
    visible_paint: bool,
    rendered: bool,
    feedback_queued: bool,
    feedback_notified: bool,
}
// Prepared pixel ownership is independent of HWNDs; cancellation releases it before display.
struct PreparedPixels {
    session: u64,
    count: usize,
    bounds: lexift_core::domain::geometry::Rect,
    key: usize,
    pixels: Vec<u8>,
}
static PIXELS: std::sync::Mutex<Vec<PreparedPixels>> = std::sync::Mutex::new(Vec::new());
thread_local! {static GROUPS:std::cell::RefCell<std::collections::HashMap<u64,Vec<(HWND,usize)>>> = std::cell::RefCell::new(std::collections::HashMap::new());}
fn bootstrap_pixels(frame: &CapturedRegion) -> Result<Vec<u8>> {
    let width = frame
        .bounds
        .right
        .checked_sub(frame.bounds.left)
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::new("Invalid region width"))?;
    let height = frame
        .bounds
        .bottom
        .checked_sub(frame.bounds.top)
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::new("Invalid region height"))?;
    if (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        != Some(frame.pixels.len())
    {
        return Err(Error::new("Invalid region pixels"));
    }
    let mut pixels = Vec::with_capacity(frame.pixels.len());
    for p in frame.pixels.as_chunks::<4>().0 {
        pixels.extend_from_slice(&[
            (u16::from(p[2]) * 119 / 255) as u8,
            (u16::from(p[1]) * 119 / 255) as u8,
            (u16::from(p[0]) * 119 / 255) as u8,
            255,
        ]);
    }
    Ok(pixels)
}
pub(crate) fn prepare_pixels(
    session: u64,
    frames: &[CapturedRegion],
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    let started = std::time::Instant::now();
    let mut result = vec![];
    for frame in frames {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        result.push(PreparedPixels {
            session,
            count: frames.len(),
            bounds: frame.bounds,
            key: frame.pixels.as_ptr() as usize,
            pixels: bootstrap_pixels(frame)?,
        });
    }
    let mut cache = PIXELS.lock().unwrap_or_else(|p| p.into_inner());
    if !cancelled.load(std::sync::atomic::Ordering::Acquire) {
        cache.retain(|p| p.session != session);
        cache.extend(result);
    }
    tracing::debug!(
        session,
        elapsed_ms = started.elapsed().as_millis(),
        "Screenshot bootstrap pixels prepared"
    );
    Ok(())
}
pub(crate) fn clear_pixels(session: u64) {
    PIXELS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|p| p.session != session);
}
/// Each group crosses its native desktop barrier once per phase, never once per monitor.
fn complete_group(hwnd: HWND, data: *mut Surface, feedback: bool) -> bool {
    let members = if let Some((group, count)) = unsafe { (*data).group } {
        let values = GROUPS.with(|g| g.borrow().get(&group).cloned().unwrap_or_default());
        if values.len() != count {
            return false;
        }
        values
    } else {
        vec![(hwnd, unsafe { (*data).instance })]
    };
    let mut surfaces = vec![];
    for (window, instance) in members {
        let Some(surface) = reference(window) else {
            return false;
        };
        let ready = unsafe {
            (*surface).instance == instance
                && if feedback {
                    (*surface).bootstrap.is_none()
                        && (*surface).feedback_queued
                        && !(*surface).feedback_notified
                } else {
                    (*surface).visible_paint
                        && (*surface).rendered
                        && (*surface).bootstrap.is_some()
                }
        };
        if !ready {
            return false;
        }
        surfaces.push((window, surface));
    }
    let started = std::time::Instant::now();
    if unsafe { DwmFlush() }.is_err() {
        return false;
    }
    tracing::debug!(
        feedback,
        windows = surfaces.len(),
        elapsed_us = started.elapsed().as_micros(),
        "Screenshot desktop group synchronized"
    );
    for &(window, surface) in &surfaces {
        unsafe {
            if feedback {
                (*surface).feedback_notified = true;
            } else {
                (*surface).bootstrap.take();
                let _ = InvalidateRect(Some(window), None, false);
            }
        }
    }
    for (window, _) in surfaces {
        if feedback {
            super::popup::notify_feedback_frame_presented(window);
        } else {
            super::popup::notify_frame_presented(window);
        }
    }
    true
}
fn reference(hwnd: HWND) -> Option<*mut Surface> {
    let p = unsafe { GetPropW(hwnd, w!("Lexift.RegionSelectionSurface")) };
    (!p.0.is_null()).then_some(p.0.cast())
}
fn borderless(style: u32) -> u32 {
    style & !(WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME).0
}
thread_local! { static CREATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// Backend attributes are captured while the Slint adapter is constructed.
/// Restore the previous scope even if construction unwinds or nests.
pub(crate) fn creation_scope(factory: &mut dyn FnMut()) {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            CREATING.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(CREATING.with(|v| v.replace(true)));
    factory();
}
pub(crate) fn attributes(
    attrs: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    if CREATING.with(|v| v.get()) {
        attrs
            .with_visible(false)
            .with_decorations(false)
            .with_active(false)
    } else {
        attrs
    }
}
/// Prepare an invisible screen host without depending on OCR or annotation state.
pub(crate) fn prepare_surface(
    window: &winit::window::Window,
    bounds: lexift_core::domain::geometry::Rect,
    frame: Option<&CapturedRegion>,
) -> Result<()> {
    let hwnd = super::popup::required_hwnd(window)?;
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return Err(Error::new("Region surface must be prepared while hidden"));
    }
    window.set_decorations(false);
    window.set_skip_taskbar(true);
    prepare_surface_native(window, bounds, frame)
}
#[cfg(test)]
fn prepare_native(
    window: &impl raw_window_handle::HasWindowHandle,
    frame: &CapturedRegion,
) -> Result<()> {
    prepare_surface_native(window, frame.bounds, Some(frame))
}
fn prepare_surface_native(
    window: &impl raw_window_handle::HasWindowHandle,
    bounds: lexift_core::domain::geometry::Rect,
    frame: Option<&CapturedRegion>,
) -> Result<()> {
    let hwnd = super::popup::required_hwnd(window)?;
    if reference(hwnd).is_some() {
        return Ok(());
    }
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return Err(Error::new("Region surface must be prepared while hidden"));
    }
    let width = bounds
        .right
        .checked_sub(bounds.left)
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new("Invalid region surface"))?;
    let height = bounds
        .bottom
        .checked_sub(bounds.top)
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new("Invalid region surface"))?;
    let mut group = None;
    let bootstrap = if let Some(frame) = frame {
        if frame.bounds != bounds {
            return Err(Error::new("Invalid region surface bounds"));
        }
        let cached = {
            let mut cache = PIXELS.lock().unwrap_or_else(|p| p.into_inner());
            cache
                .iter()
                .position(|p| p.bounds == bounds && p.key == frame.pixels.as_ptr() as usize)
                .map(|i| cache.swap_remove(i))
        };
        Some(if let Some(prepared) = cached {
            group = Some((prepared.session, prepared.count));
            prepared.pixels
        } else {
            bootstrap_pixels(frame)?
        })
    } else {
        None
    };
    let data = Box::into_raw(Box::new(Surface {
        instance: NEXT_SURFACE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        group,
        width,
        height,
        bootstrap,
        visible_paint: false,
        rendered: false,
        feedback_queued: false,
        feedback_notified: false,
    }));
    if let Err(e) = unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.RegionSelectionSurface"),
            Some(HANDLE(data.cast())),
        )
    } {
        unsafe {
            drop(Box::from_raw(data));
        }
        return Err(Error::new(format!("Region surface: {e}")));
    }
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, data as usize) }.as_bool() {
        unsafe {
            let _ = RemovePropW(hwnd, w!("Lexift.RegionSelectionSurface"));
            drop(Box::from_raw(data));
        }
        return Err(Error::new("Could not protect region surface"));
    }
    // Keep backend state and native state aligned before its next property update.
    super::window_shell_policy::install(
        hwnd,
        Some(super::window_shell_policy::Activation::Passive),
    )?;
    super::popup::configure_passive(window)?;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, borderless(style) as isize);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            bounds.left,
            bounds.top,
            width,
            height,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
        .map_err(|e| Error::new(format!("Region geometry: {e}")))?;
    }
    tracing::debug!(width, height, "Region surface prepared while hidden");
    if let Some((group, _)) = group {
        GROUPS.with(|g| {
            g.borrow_mut()
                .entry(group)
                .or_default()
                .push((hwnd, unsafe { (*data).instance }))
        });
    }
    Ok(())
}
/// Called in an event-loop turn after the renderer returns, not inside its notifier.
/// WM_PAINT must also have occurred while visible; hidden pre-rendering alone is insufficient.
pub(crate) fn complete(window: &impl raw_window_handle::HasWindowHandle) -> Result<bool> {
    let hwnd = super::popup::required_hwnd(window)?;
    let Some(data) = reference(hwnd) else {
        return Err(Error::new("Region surface expired"));
    };
    unsafe {
        if (*data).bootstrap.is_none() {
            return Ok(true);
        }
        (*data).rendered = true;
        if !(*data).visible_paint {
            return Ok(false);
        }
        PostMessageW(Some(hwnd), HANDOFF, WPARAM((*data).instance), LPARAM(0))
            .map_err(|e| Error::new(format!("Region handoff: {e}")))?;
    }
    Ok(true)
}
unsafe fn paint(hwnd: HWND, dc: HDC, state: &Surface) {
    let Some(pixels) = &state.bootstrap else {
        return;
    };
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: state.width,
            biHeight: -state.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut client = windows::Win32::Foundation::RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        unsafe {
            StretchDIBits(
                dc,
                0,
                0,
                client.right,
                client.bottom,
                0,
                0,
                state.width,
                state.height,
                Some(pixels.as_ptr().cast()),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }
}
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    match message {
        WM_STYLECHANGING if wp.0 as i32 == GWL_STYLE.0 && lp.0 != 0 => {
            let style = unsafe { &mut *(lp.0 as *mut STYLESTRUCT) };
            style.styleNew = borderless(style.styleNew);
        }
        WM_NCCALCSIZE => return LRESULT(0),
        WM_NCPAINT => return LRESULT(0),
        WM_NCACTIVATE => {
            // Winit still needs the activation transition for keyboard focus;
            // -1 prevents DefWindowProc from painting a native caption over our frame.
            return unsafe { DefSubclassProc(hwnd, message, wp, LPARAM(-1)) };
        }
        WM_ERASEBKGND => {
            unsafe {
                paint(hwnd, HDC(wp.0 as _), &*(data as *const Surface));
            }
            // Never let the class brush erase an already presented frame.
            return LRESULT(1);
        }
        WM_SHOWWINDOW if wp.0 != 0 => {
            let dc = unsafe { GetDC(Some(hwnd)) };
            if !dc.is_invalid() {
                unsafe {
                    paint(hwnd, dc, &*(data as *const Surface));
                    ReleaseDC(Some(hwnd), dc);
                }
            }
            tracing::debug!("Prepared region surface becoming visible");
        }
        WM_PAINT => {
            let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
            let first_visible_paint =
                visible && !unsafe { (*(data as *const Surface)).visible_paint };
            if visible {
                unsafe {
                    (*(data as *mut Surface)).visible_paint = true;
                }
            }
            if first_visible_paint {
                let dc = unsafe { GetDC(Some(hwnd)) };
                if !dc.is_invalid() {
                    unsafe {
                        paint(hwnd, dc, &*(data as *const Surface));
                        ReleaseDC(Some(hwnd), dc);
                    }
                }
            }
            let started = std::time::Instant::now();
            let result = unsafe { DefSubclassProc(hwnd, message, wp, lp) };
            if first_visible_paint {
                tracing::debug!(
                    elapsed_us = started.elapsed().as_micros(),
                    "Screenshot first client paint returned"
                );
            }
            if visible && unsafe { (*(data as *const Surface)).feedback_notified } {
                super::popup::notify_feedback_frame_presented(hwnd);
            }
            // Software Slint has no rendering notifier. Its first client paint
            // after bootstrap removal must finish before editor prewarming.
            if visible
                && unsafe {
                    (*(data as *const Surface)).rendered
                        && (*(data as *const Surface)).bootstrap.is_none()
                        && !(*(data as *const Surface)).feedback_queued
                }
            {
                unsafe {
                    (*(data as *mut Surface)).feedback_queued = true;
                    let _ = PostMessageW(
                        Some(hwnd),
                        FEEDBACK,
                        WPARAM((*(data as *const Surface)).instance),
                        LPARAM(0),
                    );
                }
            }
            if visible
                && unsafe {
                    (*(data as *const Surface)).rendered
                        && (*(data as *const Surface)).bootstrap.is_some()
                }
            {
                let _ = unsafe {
                    PostMessageW(
                        Some(hwnd),
                        HANDOFF,
                        WPARAM((*(data as *const Surface)).instance),
                        LPARAM(0),
                    )
                };
            }
            return result;
        }
        HANDOFF => {
            if wp.0 == unsafe { (*(data as *const Surface)).instance } {
                complete_group(hwnd, data as *mut Surface, false);
            }
            return LRESULT(0);
        }
        FEEDBACK => {
            if wp.0 == unsafe { (*(data as *const Surface)).instance } {
                complete_group(hwnd, data as *mut Surface, true);
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => unsafe {
            if let Some((group, _)) = (*(data as *const Surface)).group {
                GROUPS.with(|g| {
                    let mut g = g.borrow_mut();
                    if let Some(m) = g.get_mut(&group) {
                        m.retain(|(h, _)| *h != hwnd);
                        if m.is_empty() {
                            g.remove(&group);
                        }
                    }
                });
            }
            let _ = RemovePropW(hwnd, w!("Lexift.RegionSelectionSurface"));
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), id);
            drop(Box::from_raw(data as *mut Surface));
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creation_scope_is_nested_and_restored_without_affecting_settings() {
        let defaults = winit::window::WindowAttributes::default();
        assert!(attributes(defaults.clone()).decorations);
        creation_scope(&mut || {
            let a = attributes(defaults.clone());
            assert!(!a.decorations && !a.visible && !a.active);
            creation_scope(&mut || assert!(!attributes(defaults.clone()).decorations));
            assert!(!attributes(defaults.clone()).decorations);
        });
        assert!(attributes(defaults.clone()).decorations);
        let _ =
            std::panic::catch_unwind(|| creation_scope(&mut || panic!("construction cancelled")));
        assert!(attributes(defaults).decorations);
    }
    struct OwnedWindow(HWND);
    impl raw_window_handle::HasWindowHandle for OwnedWindow {
        fn window_handle(
            &self,
        ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
        {
            let raw = raw_window_handle::Win32WindowHandle::new(
                std::num::NonZeroIsize::new(self.0.0 as isize).unwrap(),
            );
            Ok(unsafe {
                raw_window_handle::WindowHandle::borrow_raw(
                    raw_window_handle::RawWindowHandle::Win32(raw),
                )
            })
        }
    }
    impl Drop for OwnedWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }
    #[test]
    fn screenshot_group_waits_for_both_monitors_and_releases_prepared_buffers() {
        use lexift_core::domain::geometry::Rect;
        use std::sync::atomic::AtomicBool;
        let frames = vec![
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 32,
                    bottom: 24,
                },
                pixels: vec![255; 32 * 24 * 4],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 32,
                    top: 0,
                    right: 64,
                    bottom: 24,
                },
                pixels: vec![128; 32 * 24 * 4],
            },
        ];
        prepare_pixels(700, &frames, &AtomicBool::new(false)).unwrap();
        let make = || {
            OwnedWindow(
                unsafe {
                    CreateWindowExW(
                        WS_EX_APPWINDOW,
                        w!("STATIC"),
                        w!("Grouped screenshot test"),
                        WS_OVERLAPPEDWINDOW,
                        0,
                        0,
                        32,
                        24,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                .unwrap(),
            )
        };
        let a = make();
        let b = make();
        prepare_native(&a, &frames[0]).unwrap();
        prepare_native(&b, &frames[1]).unwrap();
        assert!(!PIXELS.lock().unwrap().iter().any(|p| p.session == 700));
        let pa = reference(a.0).unwrap();
        let pb = reference(b.0).unwrap();
        unsafe {
            (*pa).visible_paint = true;
            (*pa).rendered = true;
            (*pb).visible_paint = true;
        }
        assert!(!complete_group(a.0, pa, false));
        assert!(unsafe { (*pa).bootstrap.is_some() });
        unsafe {
            (*pb).rendered = true;
        }
        assert!(complete_group(b.0, pb, false));
        assert!(unsafe { (*pa).bootstrap.is_none() && (*pb).bootstrap.is_none() });
        unsafe {
            (*pa).feedback_queued = true;
        }
        assert!(!complete_group(a.0, pa, true));
        assert!(!unsafe { (*pa).feedback_notified });
        unsafe {
            (*pb).feedback_queued = true;
        }
        assert!(complete_group(b.0, pb, true));
        assert!(unsafe { (*pa).feedback_notified && (*pb).feedback_notified });
        assert!(!complete_group(a.0, pa, true));
        drop(a);
        drop(b);
        assert!(!GROUPS.with(|g| g.borrow().contains_key(&700)));
        prepare_pixels(701, &frames, &AtomicBool::new(false)).unwrap();
        clear_pixels(701);
        assert!(!PIXELS.lock().unwrap().iter().any(|p| p.session == 701));
        prepare_pixels(702, &frames, &AtomicBool::new(true)).unwrap();
        assert!(!PIXELS.lock().unwrap().iter().any(|p| p.session == 702));
    }

    #[test]
    fn hidden_native_surface_has_exact_client_bounds_and_guards_backend_rewrites() {
        unsafe extern "system" fn observe_activation(
            hwnd: HWND,
            message: u32,
            wp: WPARAM,
            lp: LPARAM,
            _id: usize,
            data: usize,
        ) -> LRESULT {
            if message == WM_NCACTIVATE {
                let events = unsafe { &*(data as *const std::cell::RefCell<Vec<(usize, isize)>>) };
                events.borrow_mut().push((wp.0, lp.0));
            }
            unsafe { DefSubclassProc(hwnd, message, wp, lp) }
        }
        let activations = std::cell::RefCell::new(Vec::<(usize, isize)>::new());
        let foreground = unsafe { GetForegroundWindow() };
        let window = OwnedWindow(
            unsafe {
                CreateWindowExW(
                    WS_EX_APPWINDOW,
                    w!("STATIC"),
                    w!("Region surface regression"),
                    WS_OVERLAPPEDWINDOW,
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
        let frame = CapturedRegion {
            bounds: lexift_core::domain::geometry::Rect {
                left: -200,
                top: 30,
                right: -104,
                bottom: 78,
            },
            pixels: vec![255; 96 * 48 * 4],
        };
        assert!(
            unsafe {
                SetWindowSubclass(
                    window.0,
                    Some(observe_activation),
                    SUBCLASS + 1,
                    &activations as *const _ as usize,
                )
            }
            .as_bool()
        );
        prepare_native(&window, &frame).unwrap();
        assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
        unsafe {
            SetWindowLongPtrW(
                window.0,
                GWL_STYLE,
                (WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN).0 as isize,
            );
            SetWindowLongPtrW(window.0, GWL_EXSTYLE, WS_EX_APPWINDOW.0 as isize);
        }
        let style = unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) } as u32;
        assert_eq!(
            style & (WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME).0,
            0
        );
        assert_ne!(style & WS_CLIPCHILDREN.0, 0);
        let ex = unsafe { GetWindowLongPtrW(window.0, GWL_EXSTYLE) } as u32;
        assert_eq!(ex & WS_EX_APPWINDOW.0, 0);
        assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0);
        assert_ne!(ex & WS_EX_NOACTIVATE.0, 0);
        for active in [1, 0, 1] {
            unsafe {
                SendMessageW(
                    window.0,
                    WM_NCACTIVATE,
                    Some(WPARAM(active)),
                    Some(LPARAM(0x1234)),
                );
            }
        }
        assert_eq!(
            *activations.borrow(),
            [(1, -1), (0, -1), (1, -1)],
            "activation must reach the backend without default non-client painting"
        );
        assert_eq!(
            unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) } as u32,
            style
        );
        let mut bounds = windows::Win32::Foundation::RECT::default();
        let mut origin = windows::Win32::Foundation::POINT::default();
        unsafe { GetClientRect(window.0, &mut bounds) }.unwrap();
        assert_eq!((bounds.right, bounds.bottom), (96, 48));
        assert!(unsafe { ClientToScreen(window.0, &mut origin) }.as_bool());
        assert_eq!((origin.x, origin.y), (-200, 30));
        assert!(!complete(&window).unwrap());
        let data = reference(window.0).unwrap();
        assert!(unsafe { &*data }.bootstrap.is_some());
        let frames = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = frames.clone();
        let feedbacks = std::rc::Rc::new(std::cell::Cell::new(0));
        let feedback_count = feedbacks.clone();
        super::super::popup::enable_passive_interaction(
            &window,
            Box::new(move |event| {
                if event == crate::PopupPointerEvent::FramePresented {
                    count.set(count.get() + 1);
                } else if event == crate::PopupPointerEvent::FeedbackFramePresented {
                    feedback_count.set(feedback_count.get() + 1);
                }
            }),
        )
        .unwrap();
        // Control the completion point independently of the machine's paint queue.
        unsafe {
            (*data).visible_paint = true;
            (*data).rendered = true;
            let instance = (*data).instance;
            SendMessageW(window.0, HANDOFF, Some(WPARAM(instance)), Some(LPARAM(0)));
            SendMessageW(window.0, HANDOFF, Some(WPARAM(instance)), Some(LPARAM(0)));
        }
        assert_eq!(frames.get(), 1);
        unsafe {
            let instance = (*data).instance;
            SendMessageW(window.0, FEEDBACK, Some(WPARAM(instance)), Some(LPARAM(0)));
            assert_eq!(
                feedbacks.get(),
                0,
                "bootstrap handoff is not a client paint"
            );
            (*data).feedback_queued = true;
            SendMessageW(
                window.0,
                FEEDBACK,
                Some(WPARAM(instance + 1)),
                Some(LPARAM(0)),
            );
            assert_eq!(feedbacks.get(), 0, "retired surface identities are ignored");
            SendMessageW(window.0, FEEDBACK, Some(WPARAM(instance)), Some(LPARAM(0)));
            SendMessageW(window.0, FEEDBACK, Some(WPARAM(instance)), Some(LPARAM(0)));
        }
        assert_eq!(feedbacks.get(), 1);
        let hwnd = window.0;
        drop(window);
        assert!(reference(hwnd).is_none());
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }
    #[test]
    fn backend_style_rewrites_cannot_restore_nonclient_frame_or_remove_other_flags() {
        let style = (WS_CAPTION
            | WS_THICKFRAME
            | WS_BORDER
            | WS_DLGFRAME
            | WS_VISIBLE
            | WS_POPUP
            | WS_CLIPCHILDREN)
            .0;
        assert_eq!(
            borderless(style),
            (WS_VISIBLE | WS_POPUP | WS_CLIPCHILDREN).0
        );
    }
}
