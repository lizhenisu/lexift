//! Opt-in native App regression: owned-window hit probes and real pointer gestures.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, w},
};
pub(crate) fn probe(window: &impl HasWindowHandle, label: &str, click: bool) -> bool {
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(handle.hwnd.get() as _);
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != unsafe { GetCurrentProcessId() } {
        return false;
    }
    if !tracked().lock().unwrap().contains(&(hwnd.0 as usize)) {
        tracked().lock().unwrap().push(hwnd.0 as usize);
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return false;
    }
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    let inset = (16 * dpi / 96) as i32;
    let body = POINT {
        x: (rect.left + rect.right) / 2,
        y: (rect.top + rect.bottom) / 2,
    };
    let corner = POINT {
        x: rect.right - inset,
        y: rect.top + inset,
    };
    let body_hit = unsafe { WindowFromPoint(body) };
    let corner_hit = unsafe { WindowFromPoint(corner) };
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetGUIThreadInfo(thread, &mut gui) };
    println!(
        "PIN_NATIVE label={label} hwnd={:?} rect={rect:?} body_hit={body_hit:?} corner_hit={corner_hit:?} capture={:?} focus={:?} flags={:?}",
        hwnd, gui.hwndCapture, gui.hwndFocus, gui.flags
    );
    unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> BOOL {
        let pid = param.0 as u32;
        let mut actual = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut actual)) };
        if actual == pid {
            let mut r = RECT::default();
            let _ = unsafe { GetWindowRect(hwnd, &mut r) };
            let mut text = [0u16; 128];
            let n = unsafe { GetWindowTextW(hwnd, &mut text) };
            println!(
                "PIN_WINDOW hwnd={hwnd:?} visible={} title={} rect={r:?} ex={:x} annotation={}",
                unsafe { IsWindowVisible(hwnd) }.as_bool(),
                String::from_utf16_lossy(&text[..n.max(0) as usize]),
                unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) },
                !unsafe { GetPropW(hwnd, w!("Lexift.AnnotationOrder")) }.is_invalid()
            );
        }
        true.into()
    }
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(pid as isize)) };
    if (label.starts_with("drag-") || label == "draw-pin") && body_hit == hwnd {
        return interact(hwnd, body, true);
    }
    if click && corner_hit == hwnd {
        return interact(hwnd, body, false);
    }
    if label == "through-drawing" {
        return body_hit != hwnd
            && (unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TRANSPARENT.0) != 0;
    }
    if label.starts_with("menu-") && body_hit == hwnd {
        let select = match label {
            "menu-annotate" => 3,
            "menu-hide" => 4,
            "menu-shadow" => 5,
            "menu-topmost" => 6,
            _ => 0,
        };
        return menu_escape(hwnd, body, select);
    }
    if label == "not-topmost" {
        return unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0 == 0;
    }
    body_hit == hwnd && corner_hit == hwnd
}

/// Exercise the production pointer bridge and native move loop, targeting only a verified owned pin.
fn interact(hwnd: HWND, body: POINT, drag: bool) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    if unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0 {
        return false;
    }
    let mut original = POINT::default();
    if unsafe { GetCursorPos(&mut original) }.is_err() {
        return false;
    }
    let key = hwnd.0 as usize;
    std::thread::spawn(move || {
        let hwnd = HWND(key as _);
        if unsafe { WindowFromPoint(body) } != hwnd {
            return;
        }
        let mouse = |flags| {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        };
        let _ = unsafe { SetCursorPos(body.x, body.y) };
        std::thread::sleep(std::time::Duration::from_millis(150));
        mouse(MOUSEEVENTF_LEFTDOWN);
        std::thread::sleep(std::time::Duration::from_millis(150));
        for offset in [10, 20, 30, 40].into_iter().filter(|_| drag) {
            let _ = unsafe { SetCursorPos(body.x + offset, body.y + offset) };
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        mouse(MOUSEEVENTF_LEFTUP);
        if !drag {
            std::thread::sleep(std::time::Duration::from_millis(100));
            send_key(VK_ESCAPE);
        }
        let _ = unsafe { SetCursorPos(original.x, original.y) };
    });
    true
}

fn tracked() -> &'static std::sync::Mutex<Vec<usize>> {
    static WINDOWS: std::sync::OnceLock<std::sync::Mutex<Vec<usize>>> = std::sync::OnceLock::new();
    WINDOWS.get_or_init(Default::default)
}
pub(crate) fn destroyed() -> bool {
    tracked().lock().unwrap().iter().all(|key| {
        let hwnd = HWND(*key as _);
        let destroyed = !unsafe { IsWindow(Some(hwnd)) }.as_bool();
        println!("PIN_HWND_DESTROYED hwnd={hwnd:?} destroyed={destroyed}");
        destroyed
    })
}

fn send_key(key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    for flags in [KEYBD_EVENT_FLAGS(0), KEYEVENTF_KEYUP] {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    }
}
fn menu_escape(hwnd: HWND, point: POINT, select: usize) -> bool {
    let key = hwnd.0 as usize;
    std::thread::spawn(move || {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let hwnd = HWND(key as _);
        if unsafe { WindowFromPoint(point) } != hwnd {
            return;
        }
        let mut old = POINT::default();
        let _ = unsafe { GetCursorPos(&mut old) };
        let _ = unsafe { SetCursorPos(point.x, point.y) };
        for flags in [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP] {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let _ = unsafe { GetGUIThreadInfo(GetWindowThreadProcessId(hwnd, None), &mut gui) };
        println!(
            "PIN_MENU_NATIVE flags={:?} owner={:?}",
            gui.flags, gui.hwndMenuOwner
        );
        if select == 0 {
            send_key(VK_ESCAPE);
        } else {
            for _ in 0..select {
                send_key(VK_DOWN);
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            send_key(VK_RETURN);
        }
        let _ = unsafe { SetCursorPos(old.x, old.y) };
    });
    true
}
