//! Keeps an opaque window opaque, and fills the client area a native resize exposes.
//!
//! Slint asks winit for a transparent window on every platform, and winit implements that on
//! Windows with `DwmEnableBlurBehindWindow` plus an empty blur region. That switches the window to
//! per-pixel alpha, so every pixel the application has not painted — including a band a resize
//! exposed, or a surface the compositor recreated after the window left the screen — shows the
//! desktop straight through. The Popup and Settings are opaque panels, so this module turns the
//! alpha back off and then covers the bands a size change uncovers with the window's own
//! background colour.
//!
//! The module deliberately paints nothing else. In particular it does not handle `WM_ERASEBKGND`:
//! the window class has no background brush, so the default handler leaves the pixels alone. Erasing
//! the update region used to be harmless while the window was per-pixel alpha, but once Windows
//! invalidates a region because the window moved (for example after dragging it off the screen and
//! back) it would paint the flat background over content the software renderer never repaints,
//! leaving a blank band.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::{COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{
        Dwm::{DWM_BB_ENABLE, DWM_BLURBEHIND, DwmEnableBlurBehindWindow},
        Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, HBRUSH, ReleaseDC},
    },
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, GetPropW, RemovePropW, SetPropW, WM_CANCELMODE, WM_EXITSIZEMOVE,
            WM_NCDESTROY, WM_PAINT, WM_SHOWWINDOW, WM_SIZE,
        },
    },
};
use windows::core::w;

const SUBCLASS_ID: usize = 0x4C_58_52_42;

/// State carried by the window subclass for the lifetime of its HWND.
struct ResizeBackground {
    color_rgb: [u8; 3],
    exposure: ExposureState,
    /// Cached brush for the band fills, so the resize path does not create and destroy it.
    brush: Option<HBRUSH>,
    /// Runs once after Windows finishes an interactive move or resize for this window.
    on_geometry_change: Option<std::rc::Rc<dyn Fn()>>,
}

/// One rectangle, in client coordinates, that a size change uncovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientBand {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Rectangles a grow step exposes: the right band, then the bottom band.
///
/// Both bands span the current axis length so a shrink on one axis cannot leave a row or column
/// behind, and the shared corner is intentionally filled twice.
fn exposed_bands(previous: (i32, i32), current: (i32, i32)) -> [Option<ClientBand>; 2] {
    let (current_width, current_height) = current;
    if current_width <= 0 || current_height <= 0 {
        return [None, None];
    }
    let (previous_width, previous_height) = (previous.0.max(0), previous.1.max(0));
    let right = (current_width > previous_width).then_some(ClientBand {
        left: previous_width,
        top: 0,
        right: current_width,
        bottom: current_height,
    });
    let bottom = (current_height > previous_height).then_some(ClientBand {
        left: 0,
        top: previous_height,
        right: current_width,
        bottom: current_height,
    });
    [right, bottom]
}

/// Tracks what a client-size growth uncovered and when it has been covered.
///
/// Windows marks the whole window valid once the application presents a frame, so a band the
/// application never painted would keep the compositor's stale pixels until the next size change.
/// The band is therefore covered by the paint that follows the size message — the frame the
/// application may still render at the previous size. Covering it again on a later paint would
/// erase content the application no longer redraws, because its damage tracking only includes the
/// items that changed.
#[derive(Clone, Copy, Default)]
struct ExposureState {
    previous_client: Option<(i32, i32)>,
    pending: Option<[Option<ClientBand>; 2]>,
}

impl ExposureState {
    /// Records a size message and reports the bands to cover immediately.
    fn on_size(&mut self, current: (i32, i32)) -> Option<[Option<ClientBand>; 2]> {
        let bands = self
            .previous_client
            .map(|previous| exposed_bands(previous, current));
        self.previous_client = Some(current);
        let Some(bands) = bands.filter(|bands| bands.iter().any(Option::is_some)) else {
            // A shrink or an unchanged size leaves nothing uncovered: the frame the application
            // already painted is at least as large as the client area.
            self.pending = None;
            return None;
        };
        self.pending = Some(bands);
        Some(bands)
    }

    /// Reports the bands to cover before the application paints this cycle.
    fn on_paint(&mut self) -> Option<[Option<ClientBand>; 2]> {
        self.pending.take()
    }

    /// Drops the pending band, for example when the window is hidden or a drag is cancelled.
    fn clear_pending(&mut self) {
        self.pending = None;
    }
}

fn colorref(rgb: [u8; 3]) -> COLORREF {
    let [red, green, blue] = rgb;
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

/// Client area of the window, or `None` while it has no usable size.
fn client_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) }
        .ok()
        .map(|()| (client.right - client.left, client.bottom - client.top))
}

fn hwnd(window: &impl HasWindowHandle) -> lexift_core::Result<HWND> {
    let handle = window
        .window_handle()
        .map_err(|_| lexift_core::Error::new("Win32 window handle is unavailable"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(lexift_core::Error::new(
            "Win32 window handle is unavailable",
        ));
    };
    Ok(HWND(handle.hwnd.get() as *mut core::ffi::c_void))
}

fn state(hwnd: HWND) -> Option<*mut ResizeBackground> {
    let value = unsafe { GetPropW(hwnd, w!("Lexift.ResizeBackground")) };
    (!value.0.is_null()).then_some(value.0 as *mut ResizeBackground)
}

/// Restores DWM's opaque composition for a window Slint created as transparent.
///
/// Winit switches a transparent window to per-pixel alpha through a blur-behind call with an empty
/// region. Disabling the blur again makes the compositor ignore that alpha, so an unpainted pixel
/// can only show the window's own colour instead of the desktop behind it.
fn disable_per_pixel_alpha(hwnd: HWND) {
    let blur = DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE,
        fEnable: false.into(),
        hRgnBlur: Default::default(),
        fTransitionOnMaximized: false.into(),
    };
    if let Err(error) = unsafe { DwmEnableBlurBehindWindow(hwnd, &blur) } {
        tracing::warn!(%error, "per-pixel alpha could not be disabled for an opaque window");
    }
}

/// Solid brush for the window background, created once and reused for every fill.
fn background_brush(state: &mut ResizeBackground) -> Option<HBRUSH> {
    if state.brush.is_none() {
        let created = unsafe { CreateSolidBrush(colorref(state.color_rgb)) };
        if created.0.is_null() {
            return None;
        }
        state.brush = Some(created);
    }
    state.brush
}

/// Paints the bands a grow step uncovered with the window background colour.
fn fill_bands(state: &mut ResizeBackground, hwnd: HWND, bands: [Option<ClientBand>; 2]) {
    let Some(brush) = background_brush(state) else {
        return;
    };
    let device = unsafe { GetDC(Some(hwnd)) };
    if device.0.is_null() {
        return;
    }
    for band in bands.into_iter().flatten() {
        let rect = RECT {
            left: band.left,
            top: band.top,
            right: band.right,
            bottom: band.bottom,
        };
        let _ = unsafe { FillRect(device, &rect, brush) };
    }
    let _ = unsafe { ReleaseDC(Some(hwnd), device) };
}

pub(crate) fn install(
    window: &impl HasWindowHandle,
    color_rgb: [u8; 3],
) -> lexift_core::Result<()> {
    let hwnd = hwnd(window)?;
    disable_per_pixel_alpha(hwnd);
    if let Some(pointer) = state(hwnd) {
        // The window already carries the fill: refresh the colour later fills should use.
        let state = unsafe { &mut *pointer };
        if state.color_rgb != color_rgb {
            state.color_rgb = color_rgb;
            if let Some(brush) = state.brush.take() {
                let _ = unsafe { DeleteObject(brush.into()) };
            }
        }
        return Ok(());
    }
    let pointer = Box::into_raw(Box::new(ResizeBackground {
        color_rgb,
        exposure: ExposureState::default(),
        brush: None,
        on_geometry_change: None,
    }));
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.ResizeBackground"),
            Some(HANDLE(pointer.cast())),
        )
    } {
        unsafe { drop(Box::from_raw(pointer)) };
        return Err(lexift_core::Error::new(format!(
            "Could not attach resize background state: {error}"
        )));
    }
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, pointer as usize) }.as_bool()
    {
        if unsafe { RemovePropW(hwnd, w!("Lexift.ResizeBackground")) }.is_ok() {
            unsafe { drop(Box::from_raw(pointer)) };
        }
        return Err(lexift_core::Error::new(
            "Could not install the resize background subclass",
        ));
    }
    Ok(())
}

/// Stores the callback that runs after Windows finishes an interactive move or resize.
///
/// Windows can resize a window on its own while the modal loop runs — an edge snap, and the restore
/// that follows when the user keeps dragging — and coalesces the size messages so the application may
/// never learn about the intermediate geometry. The pixels painted for that geometry stay on screen,
/// so the window's owner uses this hook to repaint the whole client area once the loop ends.
pub(crate) fn set_geometry_repair(
    window: &impl HasWindowHandle,
    repair: Box<dyn Fn()>,
) -> lexift_core::Result<()> {
    let hwnd = hwnd(window)?;
    let Some(pointer) = state(hwnd) else {
        return Err(lexift_core::Error::new(
            "The window has no resize background state yet",
        ));
    };
    unsafe { (*pointer).on_geometry_change = Some(std::rc::Rc::from(repair)) };
    Ok(())
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if reference_data == 0 {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    match message {
        WM_SIZE => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            let state = unsafe { &mut *(reference_data as *mut ResizeBackground) };
            if let Some(current) = client_size(hwnd) {
                let bands = state.exposure.on_size(current);
                if let Some(bands) = bands {
                    fill_bands(state, hwnd, bands);
                }
            }
            result
        }
        WM_PAINT => {
            // Cover the band before the application paints: Windows validates the whole window
            // once it presents, so a band nobody painted would keep stale pixels on screen.
            let state = unsafe { &mut *(reference_data as *mut ResizeBackground) };
            let bands = state.exposure.on_paint();
            if let Some(bands) = bands {
                fill_bands(state, hwnd, bands);
            }
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_CANCELMODE => {
            let state = unsafe { &mut *(reference_data as *mut ResizeBackground) };
            state.exposure.clear_pending();
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_EXITSIZEMOVE => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            let repair = {
                let state = unsafe { &*(reference_data as *const ResizeBackground) };
                state.on_geometry_change.clone()
            };
            if let Some(repair) = repair {
                repair();
            }
            result
        }
        WM_SHOWWINDOW if wparam.0 == 0 => {
            let state = unsafe { &mut *(reference_data as *mut ResizeBackground) };
            state.exposure.clear_pending();
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_NCDESTROY => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
                let _ = RemovePropW(hwnd, w!("Lexift.ResizeBackground"));
                let state = Box::from_raw(reference_data as *mut ResizeBackground);
                if let Some(brush) = state.brush {
                    let _ = DeleteObject(brush.into());
                }
            }
            result
        }
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientBand, ExposureState, exposed_bands};

    fn band(left: i32, top: i32, right: i32, bottom: i32) -> ClientBand {
        ClientBand {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn growing_both_axes_exposes_a_right_and_a_bottom_band() {
        assert_eq!(
            exposed_bands((100, 50), (140, 90)),
            [Some(band(100, 0, 140, 90)), Some(band(0, 50, 140, 90))]
        );
    }

    #[test]
    fn growing_one_axis_exposes_only_that_band() {
        assert_eq!(
            exposed_bands((100, 50), (140, 50)),
            [Some(band(100, 0, 140, 50)), None]
        );
        assert_eq!(
            exposed_bands((100, 50), (100, 90)),
            [None, Some(band(0, 50, 100, 90))]
        );
    }

    #[test]
    fn taller_but_narrower_window_still_fills_the_full_new_width() {
        assert_eq!(
            exposed_bands((140, 50), (100, 90)),
            [None, Some(band(0, 50, 100, 90))]
        );
    }

    #[test]
    fn shrinking_or_unchanged_windows_expose_nothing() {
        assert_eq!(exposed_bands((100, 50), (100, 50)), [None, None]);
        assert_eq!(exposed_bands((100, 50), (60, 50)), [None, None]);
        assert_eq!(exposed_bands((100, 50), (100, 20)), [None, None]);
        assert_eq!(exposed_bands((100, 50), (0, 0)), [None, None]);
        assert_eq!(exposed_bands((100, 50), (-1, 40)), [None, None]);
    }

    #[test]
    fn growth_is_covered_once_by_the_paint_after_the_size_message() {
        let mut exposure = ExposureState::default();

        // The first size message only establishes the baseline.
        assert_eq!(exposure.on_size((100, 50)), None);
        assert_eq!(exposure.on_paint(), None);

        let bands = exposure
            .on_size((140, 90))
            .expect("a grow step reports the uncovered bands");
        assert_eq!(exposure.on_paint(), Some(bands));
        // Covering the band again would erase content the application never redraws.
        assert_eq!(exposure.on_paint(), None);
    }

    #[test]
    fn continuous_growth_keeps_covering_every_step() {
        let mut exposure = ExposureState::default();
        exposure.on_size((100, 50));

        let first = exposure.on_size((120, 50)).expect("first step");
        assert_eq!(exposure.on_paint(), Some(first));
        let second = exposure.on_size((140, 60)).expect("second step");
        assert_eq!(exposure.on_paint(), Some(second));
        assert_eq!(exposure.on_paint(), None);
    }

    #[test]
    fn shrinking_clears_the_pending_band_and_cancelling_drops_it() {
        let mut exposure = ExposureState::default();
        exposure.on_size((100, 50));
        assert!(exposure.on_size((140, 90)).is_some());
        assert_eq!(exposure.on_size((120, 90)), None);
        assert_eq!(exposure.on_paint(), None);

        assert!(exposure.on_size((160, 90)).is_some());
        exposure.clear_pending();
        assert_eq!(exposure.on_paint(), None);
    }
}
