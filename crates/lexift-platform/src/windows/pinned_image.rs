//! Per-pin input bounds, independent of annotation exclusion and canvas roles.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{Result, w},
};
const SUBCLASS: usize = 0x4c585049;
struct State {
    interactive: bool,
    inset: i32,
    dpi: u32,
}
pub(crate) fn configure(
    window: &impl HasWindowHandle,
    topmost: bool,
    interactive: bool,
    inset: i32,
) -> lexift_core::Result<()> {
    let handle = window
        .window_handle()
        .map_err(|e| lexift_core::Error::new(e.to_string()))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(lexift_core::Error::new("expected Win32 pin"));
    };
    let hwnd = HWND(handle.hwnd.get() as _);
    apply(hwnd, topmost, interactive, inset).map_err(|e| lexift_core::Error::new(e.to_string()))
}
fn apply(hwnd: HWND, topmost: bool, interactive: bool, inset: i32) -> Result<()> {
    super::annotation_window_order::set_pin_level(hwnd, topmost)?;
    unsafe {
        let existing = GetPropW(hwnd, w!("Lexift.PinnedImagePolicy"));
        if !existing.is_invalid() {
            let s = &mut *(existing.0 as *mut State);
            s.interactive = interactive;
            s.inset = inset;
            s.dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
        } else {
            let pointer = Box::into_raw(Box::new(State {
                interactive,
                inset,
                dpi: windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96),
            }));
            if let Err(e) = SetPropW(
                hwnd,
                w!("Lexift.PinnedImagePolicy"),
                Some(HANDLE(pointer as _)),
            ) {
                drop(Box::from_raw(pointer));
                return Err(e);
            }
            if !SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, pointer as usize).as_bool() {
                let _ = RemovePropW(hwnd, w!("Lexift.PinnedImagePolicy"));
                drop(Box::from_raw(pointer));
                return Err(windows::core::Error::from_thread());
            }
        }
        // WS_EX_TRANSPARENT handles layered-window hit routing across application threads.
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            if interactive {
                ex & !WS_EX_TRANSPARENT.0
            } else {
                ex | WS_EX_TRANSPARENT.0
            } as isize,
        );
        SetWindowPos(
            hwnd,
            Some(if topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )?;
    }
    Ok(())
}
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    pointer: usize,
) -> LRESULT {
    if message == WM_DPICHANGED {
        let s = unsafe { &mut *(pointer as *mut State) };
        let dpi = (wparam.0 as u32 & 0xffff).max(96);
        s.inset = (s.inset as i64 * dpi as i64 / s.dpi as i64) as i32;
        s.dpi = dpi;
    }
    if message == WM_STYLECHANGING && wparam.0 == GWL_EXSTYLE.0 as isize as usize && lparam.0 != 0 {
        let s = unsafe { &*(pointer as *const State) };
        let styles = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        styles.styleNew = if s.interactive {
            styles.styleNew & !WS_EX_TRANSPARENT.0
        } else {
            styles.styleNew | WS_EX_TRANSPARENT.0
        };
    }
    if message == WM_NCHITTEST {
        let s = unsafe { &*(pointer as *const State) };
        let mut rect = windows::Win32::Foundation::RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
            let x = lparam.0 as u16 as i16 as i32;
            let y = (lparam.0 >> 16) as u16 as i16 as i32;
            if !s.interactive
                || x < rect.left + s.inset
                || x >= rect.right - s.inset
                || y < rect.top + s.inset
                || y >= rect.bottom - s.inset
            {
                return LRESULT(HTTRANSPARENT as isize);
            }
        }
    }
    if message == WM_NCDESTROY {
        unsafe {
            let _ = RemovePropW(hwnd, w!("Lexift.PinnedImagePolicy"));
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), id);
            drop(Box::from_raw(pointer as *mut State));
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_pin_policy_guards_input_without_capture_exclusion() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("STATIC"),
                w!("pin policy test"),
                WS_POPUP,
                100,
                100,
                200,
                100,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            apply(hwnd, true, true, 10).unwrap();
            assert!(!super::super::annotation_overlay::contains(hwnd));
            let edge = LPARAM(((105u32 << 16) | 105) as isize);
            assert_eq!(
                SendMessageW(hwnd, WM_NCHITTEST, None, Some(edge)).0,
                HTTRANSPARENT as isize
            );
            apply(hwnd, false, false, 10).unwrap();
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TRANSPARENT.0) as isize);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0,
                0,
                "backend style rewrites must not restore drawing interception"
            );
            let center = LPARAM(((150u32 << 16) | 150) as isize);
            assert_eq!(
                SendMessageW(hwnd, WM_NCHITTEST, None, Some(center)).0,
                HTTRANSPARENT as isize
            );
            apply(hwnd, false, true, 10).unwrap();
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32
                    & (WS_EX_TRANSPARENT.0 | WS_EX_TOPMOST.0),
                0
            );
            DestroyWindow(hwnd).unwrap();
            assert!(!IsWindow(Some(hwnd)).as_bool());
        }
    }
}
