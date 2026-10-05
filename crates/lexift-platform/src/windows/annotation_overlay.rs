//! A window-local marker for native annotation visuals; it never changes global capture policy.
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND},
        UI::WindowsAndMessaging::{GetPropW, SetPropW},
    },
    core::{Result, w},
};

/// Windows removes the property when the HWND is destroyed, so reused handles are unmarked.
pub(super) fn mark(hwnd: HWND) -> Result<()> {
    unsafe {
        SetPropW(
            hwnd,
            w!("Lexift.AnnotationVisual"),
            Some(HANDLE(std::ptr::dangling_mut())),
        )
    }
}

pub(super) fn contains(hwnd: HWND) -> bool {
    !unsafe { GetPropW(hwnd, w!("Lexift.AnnotationVisual")) }.is_invalid()
}
