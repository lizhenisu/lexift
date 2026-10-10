//! HWND-owned shell/activation policy, installed before the first backend show.
use windows::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GetPropW, RemovePropW, STYLESTRUCT, SetPropW, WM_NCDESTROY,
            WM_STYLECHANGING, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
};
const SUBCLASS_ID: usize = 0x4c58_5348;
#[derive(Clone, Copy)]
pub(crate) enum Activation {
    Passive,
    Interactive,
}
struct Policy {
    settings: bool,
    activation: Option<Activation>,
}
/// Shell visibility is invariant; keyboard activation is an independent role.
pub(super) fn tool_style(style: isize) -> isize {
    (style & !(WS_EX_APPWINDOW.0 as isize)) | WS_EX_TOOLWINDOW.0 as isize
}
fn apply(style: isize, settings: bool, activation: Option<Activation>) -> isize {
    let style = if settings {
        (style & !(WS_EX_TOOLWINDOW.0 as isize)) | WS_EX_APPWINDOW.0 as isize
    } else {
        tool_style(style)
    };
    match activation {
        Some(Activation::Passive) => style | WS_EX_NOACTIVATE.0 as isize,
        Some(Activation::Interactive) => style & !(WS_EX_NOACTIVATE.0 as isize),
        None => style,
    }
}
fn reference(hwnd: HWND) -> Option<*mut Policy> {
    let data = unsafe { GetPropW(hwnd, windows::core::w!("Lexift.AuxiliaryShellPolicy")) };
    (!data.is_invalid()).then_some(data.0.cast())
}
/// Repeated preparation updates the role without allocating another subclass.
/// A menu-only preparation preserves any previously assigned activation role.
pub(crate) fn install(hwnd: HWND, activation: Option<Activation>) -> lexift_core::Result<()> {
    if let Some(policy) = reference(hwnd) {
        if activation.is_some() {
            unsafe {
                (*policy).activation = activation;
            }
        }
        return Ok(());
    }
    let policy = Box::into_raw(Box::new(Policy {
        settings: false,
        activation,
    }));
    if let Err(error) = unsafe {
        SetPropW(
            hwnd,
            windows::core::w!("Lexift.AuxiliaryShellPolicy"),
            Some(HANDLE(policy.cast())),
        )
    } {
        unsafe {
            drop(Box::from_raw(policy));
        }
        return Err(lexift_core::Error::new(format!(
            "Could not attach auxiliary window policy: {error}"
        )));
    }
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, policy as usize) }.as_bool() {
        unsafe {
            let _ = RemovePropW(hwnd, windows::core::w!("Lexift.AuxiliaryShellPolicy"));
            drop(Box::from_raw(policy));
        }
        return Err(lexift_core::Error::new(
            "Could not protect auxiliary window shell policy",
        ));
    }
    Ok(())
}
/// Only the explicitly registered Settings instance receives Shell membership.
pub(crate) fn set_settings_role(hwnd: HWND, settings: bool) -> lexift_core::Result<()> {
    install(hwnd, None)?;
    let policy = reference(hwnd).expect("installed window policy");
    unsafe {
        (*policy).settings = settings;
    }
    enforce(hwnd);
    Ok(())
}
fn enforce(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW};
    if let Some(policy) = reference(hwnd) {
        unsafe {
            let old = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let new = apply(old, (*policy).settings, (*policy).activation);
            if old != new {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
            }
        }
    }
}

/// Thread-local creation/show safety net for native dialogs and future windows.
/// It never hooks another process and is removed before its owning thread exits.
pub struct ThreadGuard {
    hook: windows::Win32::UI::WindowsAndMessaging::HHOOK,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl ThreadGuard {
    pub fn new() -> lexift_core::Result<Self> {
        use windows::Win32::{System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*};
        let hook = unsafe {
            SetWindowsHookExW(
                WH_CALLWNDPROC,
                Some(before_window_message),
                None,
                GetCurrentThreadId(),
            )
        }
        .map_err(|e| {
            lexift_core::Error::new(format!("Could not install window Shell policy: {e}"))
        })?;
        Ok(Self {
            hook,
            _thread: std::marker::PhantomData,
        })
    }
}
impl Drop for ThreadGuard {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.hook) };
    }
}
unsafe extern "system" fn before_window_message(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code >= 0 && lparam.0 != 0 {
        let message = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if matches!(
            message.message,
            WM_CREATE | WM_SHOWWINDOW | WM_WINDOWPOSCHANGED | WM_STYLECHANGED
        ) && unsafe { GetWindowLongPtrW(message.hwnd, GWL_STYLE) } & WS_CHILD.0 as isize == 0
            && tracing::enabled!(tracing::Level::DEBUG)
        {
            let mut rect = windows::Win32::Foundation::RECT::default();
            let _ = unsafe { GetWindowRect(message.hwnd, &mut rect) };
            tracing::debug!(hwnd = ?message.hwnd, message = message.message,
                style = unsafe { GetWindowLongPtrW(message.hwnd,GWL_STYLE) },
                ex_style = unsafe { GetWindowLongPtrW(message.hwnd,GWL_EXSTYLE) },
                left=rect.left, top=rect.top, width=rect.right-rect.left, height=rect.bottom-rect.top,
                "Top-level window lifecycle");
        }
        if message.message == WM_WINDOWPOSCHANGING && message.lParam.0 != 0 {
            let position = unsafe { &mut *(message.lParam.0 as *mut WINDOWPOS) };
            if position.flags.contains(SWP_SHOWWINDOW)
                && unsafe { GetWindowLongPtrW(message.hwnd, GWL_STYLE) } & WS_CHILD.0 as isize == 0
            {
                if let Err(error) = install(message.hwnd, None) {
                    position.flags = (position.flags & !SWP_SHOWWINDOW) | SWP_HIDEWINDOW;
                    tracing::error!(%error, "Window hidden because Shell preparation failed");
                } else {
                    enforce(message.hwnd);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub(super) fn allow_activation_if_registered(hwnd: HWND) {
    if let Some(policy) = reference(hwnd) {
        unsafe {
            (*policy).activation = Some(Activation::Interactive);
        }
    }
}
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_STYLECHANGING && wparam.0 as isize == GWL_EXSTYLE.0 as isize && lparam.0 != 0 {
        let policy = unsafe { &*(data as *const Policy) };
        let styles = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        styles.styleNew =
            apply(styles.styleNew as isize, policy.settings, policy.activation) as u32;
    }
    if message == WM_NCDESTROY {
        // The state belongs exclusively to this HWND, never to a reused handle.
        unsafe {
            let _ = RemovePropW(hwnd, windows::core::w!("Lexift.AuxiliaryShellPolicy"));
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), id);
            drop(Box::from_raw(data as *mut Policy));
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{Win32::UI::WindowsAndMessaging::*, core::w};
    #[test]
    fn only_settings_keeps_shell_membership_through_backend_rewrites_and_activation() {
        unsafe {
            let main = CreateWindowExW(
                WS_EX_APPWINDOW,
                w!("STATIC"),
                w!("same title"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let settings = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("same title"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            set_settings_role(main, false).unwrap();
            set_settings_role(settings, true).unwrap();
            for h in [main, settings] {
                allow_activation_if_registered(h);
                for style in [0, WS_EX_APPWINDOW.0, (WS_EX_TOOLWINDOW | WS_EX_LAYERED).0] {
                    SetWindowLongPtrW(h, GWL_EXSTYLE, style as isize);
                    let actual = GetWindowLongPtrW(h, GWL_EXSTYLE);
                    assert_eq!(actual & WS_EX_APPWINDOW.0 as isize != 0, h == settings);
                    assert_eq!(actual & WS_EX_TOOLWINDOW.0 as isize != 0, h != settings);
                    assert_eq!(
                        actual & WS_EX_LAYERED.0 as isize,
                        (style & WS_EX_LAYERED.0) as isize
                    );
                    assert_eq!(actual & WS_EX_NOACTIVATE.0 as isize, 0);
                }
                assert!(!IsWindowVisible(h).as_bool());
                DestroyWindow(h).unwrap();
                assert!(reference(h).is_none());
            }
        }
    }
    #[test]
    fn native_thread_guard_excludes_unregistered_dialog_and_releases_hook() {
        unsafe {
            let guard = ThreadGuard::new().unwrap();
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW,
                w!("STATIC"),
                w!("native dialog"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let mut position = WINDOWPOS {
                hwnd,
                flags: SWP_SHOWWINDOW | SWP_NOACTIVATE,
                ..Default::default()
            };
            SendMessageW(
                hwnd,
                WM_WINDOWPOSCHANGING,
                None,
                Some(LPARAM((&mut position as *mut WINDOWPOS) as isize)),
            );
            assert!(reference(hwnd).is_some());
            let actual = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            assert_ne!(actual & WS_EX_TOOLWINDOW.0 as isize, 0);
            assert_eq!(actual & WS_EX_APPWINDOW.0 as isize, 0);
            drop(guard);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, WS_EX_APPWINDOW.0 as isize);
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_APPWINDOW.0 as isize,
                0
            );
            DestroyWindow(hwnd).unwrap();
            assert!(reference(hwnd).is_none());
        }
    }
    #[test]
    fn tool_style_excludes_appwindow_and_preserves_unrelated_flags() {
        for flags in [
            0,
            WS_EX_APPWINDOW.0,
            (WS_EX_APPWINDOW | WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TRANSPARENT).0,
        ] {
            let style = tool_style(flags as isize);
            assert_eq!(style & WS_EX_APPWINDOW.0 as isize, 0);
            assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
            let retained = flags & !WS_EX_APPWINDOW.0;
            assert_eq!(style & retained as isize, retained as isize);
            assert_eq!(tool_style(style), style);
        }
    }
    #[test]
    fn hidden_preparation_is_idempotent_and_destroy_releases_subclass() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW,
                w!("STATIC"),
                w!("auxiliary policy regression"),
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
            install(hwnd, Some(Activation::Passive)).unwrap();
            let first = reference(hwnd).unwrap();
            install(hwnd, None).unwrap();
            assert_eq!(reference(hwnd), Some(first));
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, WS_EX_APPWINDOW.0 as isize);
            let passive = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            assert_ne!(passive & WS_EX_NOACTIVATE.0 as isize, 0);
            assert_eq!(passive & WS_EX_APPWINDOW.0 as isize, 0);
            install(hwnd, Some(Activation::Interactive)).unwrap();
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (WS_EX_APPWINDOW | WS_EX_NOACTIVATE).0 as isize,
            );
            let interactive = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            assert_eq!(
                interactive & (WS_EX_NOACTIVATE | WS_EX_APPWINDOW).0 as isize,
                0
            );
            assert_ne!(interactive & WS_EX_TOOLWINDOW.0 as isize, 0);
            assert!(!IsWindowVisible(hwnd).as_bool());
            DestroyWindow(hwnd).unwrap();
            assert!(reference(hwnd).is_none());
        }
    }
}
