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
static NEXT_SURFACE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
struct Surface {
    instance: usize,
    width: i32,
    height: i32,
    bootstrap: Option<Vec<u8>>,
    visible_paint: bool,
    rendered: bool,
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
    let bootstrap = if let Some(frame) = frame {
        if frame.bounds != bounds
            || (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(4))
                != Some(frame.pixels.len())
        {
            return Err(Error::new("Invalid region surface pixels"));
        }
        let mut pixels = Vec::with_capacity(frame.pixels.len());
        // Same frozen pixels and #0008 dimming as the Slint selection overlay.
        for p in frame.pixels.as_chunks::<4>().0 {
            pixels.extend_from_slice(&[
                (u16::from(p[2]) * 119 / 255) as u8,
                (u16::from(p[1]) * 119 / 255) as u8,
                (u16::from(p[0]) * 119 / 255) as u8,
                255,
            ]);
        }
        Some(pixels)
    } else {
        None
    };
    let data = Box::into_raw(Box::new(Surface {
        instance: NEXT_SURFACE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        width,
        height,
        bootstrap,
        visible_paint: false,
        rendered: false,
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
            let result = unsafe { DefSubclassProc(hwnd, message, wp, lp) };
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
            if wp.0 != unsafe { (*(data as *const Surface)).instance } {
                return LRESULT(0);
            }
            if unsafe {
                (*(data as *const Surface)).visible_paint
                    && (*(data as *const Surface)).rendered
                    && (*(data as *const Surface)).bootstrap.is_some()
            } && unsafe { DwmFlush() }.is_ok()
            {
                unsafe {
                    (*(data as *mut Surface)).bootstrap.take();
                    // Expose the renderer's prepared frame after the one-shot GDI bootstrap.
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                tracing::debug!("Region initial frame handed off after visible paint");
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => unsafe {
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
    fn hidden_native_surface_has_exact_client_bounds_and_guards_backend_rewrites() {
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
        let mut bounds = windows::Win32::Foundation::RECT::default();
        let mut origin = windows::Win32::Foundation::POINT::default();
        unsafe { GetClientRect(window.0, &mut bounds) }.unwrap();
        assert_eq!((bounds.right, bounds.bottom), (96, 48));
        assert!(unsafe { ClientToScreen(window.0, &mut origin) }.as_bool());
        assert_eq!((origin.x, origin.y), (-200, 30));
        assert!(!complete(&window).unwrap());
        let data = reference(window.0).unwrap();
        assert!(unsafe { &*data }.bootstrap.is_some());
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
