//! Use the embedded icon's small bitmap entries for ordinary Win32 windows.
//!
//! Slint otherwise supplies its 256px icon as ICON_SMALL. Some consumers,
//! including Task Manager, downscale that HICON with visible dark specks.

use raw_window_handle::HasWindowHandle;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, LPARAM, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SendMessageW, WM_SETICON,
            },
        },
    },
    core::PCWSTR,
};

/// Sets DPI-sized small and large resource icons; Windows owns their lifetime.
pub(crate) fn configure(window: &impl HasWindowHandle) -> lexift_core::Result<()> {
    let hwnd = super::popup::required_hwnd(window)?;
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|_| lexift_core::Error::new("Application icon module is unavailable"))?;
    let instance = HINSTANCE(module.0);
    // winresource assigns the application's icon group resource ID 1.
    let resource = PCWSTR(std::ptr::without_provenance::<u16>(1));
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    for (kind, logical_size) in [(ICON_SMALL, 16u32), (ICON_BIG, 32)] {
        let size = ((logical_size * dpi + 48) / 96) as i32;
        let icon =
            unsafe { LoadImageW(Some(instance), resource, IMAGE_ICON, size, size, LR_SHARED) }
                .map_err(|_| {
                    lexift_core::Error::new("Could not load the embedded application icon")
                })?;
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(kind as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
    Ok(())
}
