pub(crate) fn configure(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::{
        Foundation::{GetLastError, HWND, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE,
        },
    };
    let handle = window
        .window_handle()
        .map_err(|_| lexift_core::Error::new("Popup window handle is unavailable"))?;
    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
        let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);
        // The borrowed handle keeps the window alive; the caller runs on its UI thread.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if style & WS_EX_NOACTIVATE.0 as isize == 0 {
                SetLastError(WIN32_ERROR(0));
                let previous =
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
                if previous == 0 && GetLastError().0 != 0 {
                    return Err(lexift_core::Error::new(
                        "Could not configure popup activation policy",
                    ));
                }
            }
        }
    }
    Ok(())
}
