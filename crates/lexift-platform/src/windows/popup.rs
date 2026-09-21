pub(crate) fn configure_passive(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    configure_extended_style(window, passive_extended_style)
}

pub(crate) fn configure_interactive(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    configure_extended_style(window, interactive_extended_style)
}

pub(crate) fn activate_user_requested(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

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

fn configure_extended_style(
    window: &impl raw_window_handle::HasWindowHandle,
    transform: fn(isize) -> isize,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
        },
    };
    if let Some(hwnd) = window_hwnd(window)? {
        // The borrowed handle keeps the window alive; the caller runs on its UI thread.
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
    }
    Ok(())
}

pub(crate) fn set_transient_owner(
    child: &impl raw_window_handle::HasWindowHandle,
    owner: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{GWLP_HWNDPARENT, GetWindowLongPtrW, SetWindowLongPtrW},
    };

    let child = required_hwnd(child)?;
    let owner = required_hwnd(owner)?;
    unsafe {
        let current_owner = GetWindowLongPtrW(child, GWLP_HWNDPARENT);
        let owner_value = owner.0 as isize;
        if current_owner != owner_value {
            SetLastError(WIN32_ERROR(0));
            let previous = SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner_value);
            if previous == 0 && GetLastError().0 != 0 {
                return Err(lexift_core::Error::new(
                    "Could not configure transient window owner",
                ));
            }
        }
        if GetWindowLongPtrW(child, GWLP_HWNDPARENT) != owner_value {
            return Err(lexift_core::Error::new(
                "Transient window owner did not take effect",
            ));
        }
    }
    Ok(())
}

pub(crate) fn required_hwnd(
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
        RawWindowHandle::Win32(handle) => {
            let hwnd = handle.hwnd;
            Some(HWND(hwnd.get() as *mut core::ffi::c_void))
        }
        _ => None,
    })
}

fn passive_extended_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};

    style | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize
}

fn interactive_extended_style(style: isize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};

    (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_NOACTIVATE.0 as isize)
}

fn passive_refresh_flags() -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };

    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    use super::{interactive_extended_style, passive_extended_style, passive_refresh_flags};

    #[test]
    fn passive_windows_do_not_activate_or_enter_the_task_switcher() {
        let style = passive_extended_style(0x100);
        assert_ne!(style & WS_EX_NOACTIVATE.0 as isize, 0);
        assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_ne!(style & 0x100, 0);
        let refresh_flags = passive_refresh_flags();
        assert!(refresh_flags.contains(SWP_NOACTIVATE));
        assert!(refresh_flags.contains(SWP_FRAMECHANGED));
    }

    #[test]
    fn interactive_tool_windows_can_activate_without_entering_the_task_switcher() {
        let preserved_style = 0x100;
        let style = interactive_extended_style(preserved_style | WS_EX_NOACTIVATE.0 as isize);
        assert_eq!(style & WS_EX_NOACTIVATE.0 as isize, 0);
        assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_ne!(style & preserved_style, 0);
    }
}
