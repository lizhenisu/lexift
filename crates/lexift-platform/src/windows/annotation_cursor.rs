//! DPI-aware cursor for editing a highlighter rectangle's corner radius.
//! The subclass is attached only to its canvas HWND and owns the cursor until WM_NCDESTROY.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HGDIOBJ,
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            CreateIconIndirect, DestroyCursor, GetCursor, GetPropW, HCURSOR, ICONINFO, IDC_ARROW,
            LoadCursorW, RemovePropW, SetCursor, SetPropW, WM_NCDESTROY, WM_SETCURSOR,
        },
    },
};
use windows::core::w;

const SUBCLASS_ID: usize = 0x4c_58_41_43;
// The SVG pointer tip is at (2, 2) in its 32-unit view box.
const POINTER_TIP_UNITS: u32 = 2;

struct CursorState {
    cursor: HCURSOR,
    size: i32,
    active: bool,
}

fn hwnd(window: &impl HasWindowHandle) -> lexift_core::Result<HWND> {
    let handle = window
        .window_handle()
        .map_err(|_| lexift_core::Error::new("Annotation canvas handle is unavailable"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(lexift_core::Error::new(
            "Annotation canvas has no Win32 handle",
        ));
    };
    Ok(HWND(handle.hwnd.get() as *mut core::ffi::c_void))
}

fn cursor_size(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    if dpi >= 168 {
        64
    } else if dpi >= 120 {
        48
    } else {
        32
    }
}

fn pixels(size: i32) -> &'static [u8] {
    match size {
        64 => include_bytes!(concat!(
            env!("OUT_DIR"),
            "/annotation-corner-radius-64.rgba"
        )),
        48 => include_bytes!(concat!(
            env!("OUT_DIR"),
            "/annotation-corner-radius-48.rgba"
        )),
        _ => include_bytes!(concat!(
            env!("OUT_DIR"),
            "/annotation-corner-radius-32.rgba"
        )),
    }
}

fn make_cursor(size: i32) -> lexift_core::Result<HCURSOR> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let color = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|_| lexift_core::Error::new("Could not create annotation cursor bitmap"))?;
    let source = pixels(size);
    let destination = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), source.len()) };
    // resvg's output is already premultiplied RGBA; only reorder for Win32 BGRA.
    for (rgba, bgra) in source
        .as_chunks::<4>()
        .0
        .iter()
        .zip(destination.as_chunks_mut::<4>().0.iter_mut())
    {
        bgra[0] = rgba[2];
        bgra[1] = rgba[1];
        bgra[2] = rgba[0];
        bgra[3] = rgba[3];
    }
    let mask_bytes = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    let mask = unsafe { CreateBitmap(size, size, 1, 1, Some(mask_bytes.as_ptr().cast())) };
    if mask.0.is_null() {
        let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
        return Err(lexift_core::Error::new(
            "Could not create annotation cursor mask",
        ));
    }
    let icon = unsafe {
        CreateIconIndirect(&ICONINFO {
            fIcon: false.into(),
            xHotspot: size as u32 * POINTER_TIP_UNITS / 32,
            yHotspot: size as u32 * POINTER_TIP_UNITS / 32,
            hbmMask: mask,
            hbmColor: color,
        })
    };
    let _ = unsafe { DeleteObject(HGDIOBJ(mask.0)) };
    let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
    let icon = icon.map_err(|_| lexift_core::Error::new("Could not create annotation cursor"))?;
    Ok(HCURSOR(icon.0))
}

fn release_cursor(cursor: HCURSOR, replacement: Option<HCURSOR>) {
    if unsafe { GetCursor() }.0 == cursor.0 {
        let next = replacement.or_else(|| unsafe { LoadCursorW(None, IDC_ARROW) }.ok());
        if let Some(next) = next {
            unsafe { SetCursor(Some(next)) };
        }
    }
    let _ = unsafe { DestroyCursor(cursor) };
}

/// Enables the custom cursor; failure leaves Slint free to show its ordinary move cursor.
pub(crate) fn set(window: &impl HasWindowHandle, active: bool) -> lexift_core::Result<()> {
    let hwnd = hwnd(window)?;
    let pointer = unsafe { GetPropW(hwnd, w!("Lexift.AnnotationCornerCursor")) }.0 as usize;
    if pointer != 0 {
        let state = unsafe { &mut *(pointer as *mut CursorState) };
        if !active {
            state.active = false;
            if !unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID) }.as_bool() {
                return Err(lexift_core::Error::new(
                    "Could not remove annotation cursor",
                ));
            }
            let _ = unsafe { RemovePropW(hwnd, w!("Lexift.AnnotationCornerCursor")) };
            let state = unsafe { Box::from_raw(pointer as *mut CursorState) };
            release_cursor(state.cursor, None);
            return Ok(());
        }
        let size = cursor_size(hwnd);
        if state.size != size {
            let cursor = make_cursor(size)?;
            release_cursor(state.cursor, Some(cursor));
            state.cursor = cursor;
            state.size = size;
        }
        state.active = true;
        return Ok(());
    }
    if !active {
        return Ok(());
    }
    let size = cursor_size(hwnd);
    let cursor = make_cursor(size)?;
    let pointer = Box::into_raw(Box::new(CursorState {
        cursor,
        size,
        active,
    }));
    if unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.AnnotationCornerCursor"),
            Some(HANDLE(pointer.cast())),
        )
    }
    .is_err()
    {
        let state = unsafe { Box::from_raw(pointer) };
        release_cursor(state.cursor, None);
        return Err(lexift_core::Error::new(
            "Could not store annotation cursor state",
        ));
    }
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, pointer as usize) }.as_bool()
    {
        let _ = unsafe { RemovePropW(hwnd, w!("Lexift.AnnotationCornerCursor")) };
        let state = unsafe { Box::from_raw(pointer) };
        release_cursor(state.cursor, None);
        return Err(lexift_core::Error::new(
            "Could not install annotation cursor",
        ));
    }
    Ok(())
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    if reference == 0 {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    if message == WM_SETCURSOR {
        let state = unsafe { &*(reference as *const CursorState) };
        // The client-area check excludes resize borders and other native hit regions.
        if state.active && (lparam.0 as u32 & 0xffff) == 1 && wparam.0 == hwnd.0 as usize {
            unsafe { SetCursor(Some(state.cursor)) };
            return LRESULT(1);
        }
    }
    if message == WM_NCDESTROY {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            let _ = RemovePropW(hwnd, w!("Lexift.AnnotationCornerCursor"));
            let state = Box::from_raw(reference as *mut CursorState);
            release_cursor(state.cursor, None);
        }
        return result;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, HICON};

    #[test]
    fn cursor_resource_has_a_hotspot_at_all_dpi_sizes() {
        for size in [32, 48, 64] {
            let cursor = make_cursor(size).unwrap();
            let mut info = ICONINFO::default();
            unsafe { GetIconInfo(HICON(cursor.0), &mut info) }.unwrap();
            assert_eq!(info.xHotspot, size as u32 * POINTER_TIP_UNITS / 32);
            assert_eq!(info.yHotspot, size as u32 * POINTER_TIP_UNITS / 32);
            let _ = unsafe { DeleteObject(HGDIOBJ(info.hbmMask.0)) };
            let _ = unsafe { DeleteObject(HGDIOBJ(info.hbmColor.0)) };
            unsafe { DestroyCursor(cursor) }.unwrap();
        }
    }

    #[test]
    fn generated_cursor_pixels_are_premultiplied_at_all_dpi_sizes() {
        for size in [32, 48, 64] {
            let rgba = pixels(size);
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            let (pixels, remainder) = rgba.as_chunks::<4>();
            assert!(remainder.is_empty());
            assert!(pixels.iter().any(|pixel| pixel[3] == 255));
            assert!(pixels.iter().any(|pixel| pixel[3] > 0 && pixel[3] < 255));
            assert!(
                pixels
                    .iter()
                    .all(|pixel| pixel[..3].iter().all(|channel| *channel <= pixel[3]))
            );
        }
    }
}
