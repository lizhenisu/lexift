//! Explicit capture disposition for live ink and excluded recording decorations.
use super::{popup, window_shell_policy};
use crate::RecordingWindowRole;
use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

const INPUT_SUBCLASS: usize = 0x4c58_5249;
const INPUT_PROPERTY: windows::core::PCWSTR = windows::core::w!("Lexift.RecordingInputPolicy");
const TRANSPARENT_PROPERTY: windows::core::PCWSTR =
    windows::core::w!("Lexift.RecordingInputTransparent");
const POLICY_OPAQUE: windows::Win32::Foundation::HANDLE =
    windows::Win32::Foundation::HANDLE(std::ptr::without_provenance_mut::<std::ffi::c_void>(1));
const POLICY_TRANSPARENT: windows::Win32::Foundation::HANDLE =
    windows::Win32::Foundation::HANDLE(std::ptr::without_provenance_mut::<std::ffi::c_void>(2));

/// Preserve the requested input mode when the UI backend rewrites window styles.
/// This scope is installed only on the recording's drawing input surface.
fn install_input(hwnd: HWND, transparent: bool) -> lexift_core::Result<()> {
    unsafe {
        let _ =
            windows::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(input_proc), INPUT_SUBCLASS);
    }
    if !unsafe {
        windows::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(input_proc), INPUT_SUBCLASS, 0)
    }
    .as_bool()
    {
        return Err(lexift_core::Error::new(
            "Recording input policy could not be installed",
        ));
    }
    // A HWND-owned marker avoids requiring GetWindowSubclass from the legacy
    // common-controls export set used by unmanifested native test processes.
    if unsafe { GetPropW(hwnd, INPUT_PROPERTY) }.is_invalid()
        && let Err(error) = unsafe {
            SetPropW(
                hwnd,
                INPUT_PROPERTY,
                Some(windows::Win32::Foundation::HANDLE(std::ptr::dangling_mut())),
            )
        }
    {
        unsafe {
            let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(
                hwnd,
                Some(input_proc),
                INPUT_SUBCLASS,
            );
        }
        return Err(lexift_core::Error::new(error.to_string()));
    }
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            TRANSPARENT_PROPERTY,
            Some(if transparent {
                POLICY_TRANSPARENT
            } else {
                POLICY_OPAQUE
            }),
        )
    } {
        unsafe {
            let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(
                hwnd,
                Some(input_proc),
                INPUT_SUBCLASS,
            );
        }
        return Err(lexift_core::Error::new(error.to_string()));
    }
    Ok(())
}

pub(super) fn update_input(hwnd: HWND, transparent: bool) -> lexift_core::Result<()> {
    if !unsafe { GetPropW(hwnd, INPUT_PROPERTY) }.is_invalid() {
        remove_input(hwnd);
    }
    install_input(hwnd, transparent)?;
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let desired = if transparent {
        ex_style | WS_EX_TRANSPARENT.0
    } else {
        ex_style & !WS_EX_TRANSPARENT.0
    };
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired as isize);
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    }
    .map_err(|error| lexift_core::Error::new(error.to_string()))
}

fn remove_input(hwnd: HWND) {
    unsafe {
        let _ = RemovePropW(hwnd, INPUT_PROPERTY);
        let _ = RemovePropW(hwnd, TRANSPARENT_PROPERTY);
        let _ =
            windows::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(input_proc), INPUT_SUBCLASS);
    }
}

fn transparent_input(hwnd: HWND) -> bool {
    (unsafe { GetPropW(hwnd, TRANSPARENT_PROPERTY) }) == POLICY_TRANSPARENT
}

unsafe extern "system" fn input_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _: usize,
    _: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{Foundation::LRESULT, UI::Shell::*};
    unsafe {
        match message {
            WM_STYLECHANGING if wparam.0 as i32 == GWL_EXSTYLE.0 => {
                if let Some(style) = (lparam.0 as *mut STYLESTRUCT).as_mut() {
                    if transparent_input(hwnd) {
                        style.styleNew |= WS_EX_TRANSPARENT.0;
                    } else {
                        style.styleNew &= !WS_EX_TRANSPARENT.0;
                    }
                }
            }
            WM_NCHITTEST if transparent_input(hwnd) => {
                return LRESULT(HTTRANSPARENT as isize);
            }
            WM_NCDESTROY => {
                let _ = RemovePropW(hwnd, INPUT_PROPERTY);
                let _ = RemovePropW(hwnd, TRANSPARENT_PROPERTY);
                let _ = RemoveWindowSubclass(hwnd, Some(input_proc), INPUT_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

pub(crate) fn configure(hwnd: HWND, role: RecordingWindowRole) -> lexift_core::Result<()> {
    let passive = matches!(
        role,
        RecordingWindowRole::Content | RecordingWindowRole::Feedback
    );
    window_shell_policy::install(
        hwnd,
        Some(if passive {
            window_shell_policy::Activation::Passive
        } else {
            window_shell_policy::Activation::Interactive
        }),
    )?;
    if passive {
        // Shared hidden preparation first treats popups as controls. Retire
        // that input guard before installing the final passive window role.
        remove_input(hwnd);
        popup::configure_capture_feedback_hwnd(hwnd)?;
        // Content/feedback never own input. Disabling these HWNDs also excludes
        // them from native window hit tests; a separate surface owns drawing.
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd, false);
        }
    } else {
        popup::configure_recording_interaction_hwnd(hwnd)?;
        if matches!(role, RecordingWindowRole::Interaction) {
            install_input(hwnd, false)?;
        } else {
            remove_input(hwnd);
        }
    }
    // WDA is installed before showing the window. Window style rewrites do not
    // change its independent capture affinity; shell/input policy is subclassed.
    unsafe {
        SetWindowDisplayAffinity(
            hwnd,
            if role == RecordingWindowRole::Content {
                WDA_NONE
            } else {
                WDA_EXCLUDEFROMCAPTURE
            },
        )
    }
    .map_err(|e| lexift_core::Error::new(format!("Recording window capture policy: {e}")))?;
    let mut actual = 0;
    unsafe { GetWindowDisplayAffinity(hwnd, &mut actual) }
        .map_err(|e| lexift_core::Error::new(e.to_string()))?;
    let expected = if role == RecordingWindowRole::Content {
        WDA_NONE.0
    } else {
        WDA_EXCLUDEFROMCAPTURE.0
    };
    if actual != expected {
        return Err(lexift_core::Error::new(
            "Recording capture policy was not installed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::LPARAM;
    use windows::core::w;

    #[test]
    fn recording_input_mode_survives_backend_style_rewrites() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Owned recording input policy test"),
                WS_POPUP,
                0,
                0,
                32,
                32,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            install_input(hwnd, true).unwrap();
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, WS_EX_LAYERED.0 as isize);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize,
                0
            );
            update_input(hwnd, false).unwrap();
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (WS_EX_LAYERED | WS_EX_TRANSPARENT).0 as isize,
            );
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize,
                0
            );
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_LAYERED.0 as isize,
                0
            );
            update_input(hwnd, true).unwrap();
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, WS_EX_LAYERED.0 as isize);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize,
                0,
                "opaque-to-transparent must replace the old subclass policy"
            );
            assert_eq!(
                SendMessageW(hwnd, WM_NCHITTEST, None, Some(LPARAM(0))).0,
                HTTRANSPARENT as isize
            );
            remove_input(hwnd);
            popup::configure_capture_feedback_hwnd(hwnd).unwrap();
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, 0);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize,
                0,
                "A prepared control becoming feedback must remain passive"
            );
            assert!(GetPropW(hwnd, INPUT_PROPERTY).is_invalid());
            DestroyWindow(hwnd).unwrap();
        }
    }
}
