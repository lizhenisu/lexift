//! Foreground-only wheel injection for the long capture worker.
use lexift_core::{
    Error, Result,
    domain::{
        geometry::{Point, Rect},
        long_capture::Axis,
    },
    ports::{magnifier::WindowToken, screen_capture::LongScrollTarget},
};
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint},
    System::Threading::GetCurrentProcessId,
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL,
            MOUSEINPUT, SendInput, VK_CONTROL, VK_LBUTTON, VK_LWIN, VK_MBUTTON, VK_MENU,
            VK_RBUTTON, VK_RWIN, VK_SHIFT, VK_XBUTTON1, VK_XBUTTON2,
        },
        WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetForegroundWindow, GetPhysicalCursorPos,
            GetWindowThreadProcessId, WHEEL_DELTA, WindowFromPoint,
        },
    },
};

pub(super) fn target(bounds: Rect) -> Option<LongScrollTarget> {
    let mut pos = POINT::default();
    unsafe { GetPhysicalCursorPos(&mut pos) }.ok()?;
    if !(bounds.left..bounds.right).contains(&pos.x)
        || !(bounds.top..bounds.bottom).contains(&pos.y)
        || [
            VK_CONTROL,
            VK_MENU,
            VK_SHIFT,
            VK_LWIN,
            VK_RWIN,
            VK_LBUTTON,
            VK_RBUTTON,
            VK_MBUTTON,
            VK_XBUTTON1,
            VK_XBUTTON2,
        ]
        .iter()
        .any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0)
    {
        return None;
    }
    let hit = unsafe { WindowFromPoint(pos) };
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    let foreground = unsafe { GetForegroundWindow() };
    if root.is_invalid()
        || foreground.is_invalid()
        || unsafe { GetAncestor(foreground, GA_ROOT) } != root
    {
        return None;
    }
    let mut process = 0;
    let mut root_process = 0;
    let thread = unsafe { GetWindowThreadProcessId(hit, Some(&mut process)) };
    unsafe { GetWindowThreadProcessId(root, Some(&mut root_process)) };
    let own_process = unsafe { GetCurrentProcessId() };
    if thread == 0
        || process == 0
        || root_process == 0
        || process == own_process
        || root_process == own_process
    {
        return None;
    }
    let monitor = unsafe { MonitorFromPoint(pos, MONITOR_DEFAULTTONULL) };
    let (mut dpi, mut vertical_dpi) = (0, 0);
    // The display scale may differ from the target HWND's DPI awareness.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut vertical_dpi) }.ok()?;
    (dpi != 0).then_some(LongScrollTarget {
        hit: WindowToken(hit.0 as isize),
        root: WindowToken(root.0 as isize),
        process,
        thread,
        point: Point { x: pos.x, y: pos.y },
        dpi,
    })
}

pub(super) fn matches(bounds: Rect, original: LongScrollTarget) -> bool {
    target(bounds).is_some_and(|current| {
        let dx = f64::from(current.point.x) - f64::from(original.point.x);
        let dy = f64::from(current.point.y) - f64::from(original.point.y);
        let radius = f64::from(original.dpi) * 8.0 / 96.0;
        dx * dx + dy * dy <= radius * radius
            && LongScrollTarget {
                point: original.point,
                ..current
            } == original
    })
}

pub(super) fn step(bounds: Rect, original: LongScrollTarget, axis: Axis) -> Result<()> {
    if !matches(bounds, original) {
        return Err(Error::new("Automatic scrolling target changed"));
    }
    if unsafe { SendInput(&[wheel_input(axis)], std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err(Error::new("Automatic scrolling input was rejected"));
    }
    Ok(())
}

fn wheel_input(axis: Axis) -> INPUT {
    let (flags, delta) = match axis {
        Axis::Vertical => (MOUSEEVENTF_WHEEL, -(WHEEL_DELTA as i32)),
        Axis::Horizontal => (MOUSEEVENTF_HWHEEL, WHEEL_DELTA as i32),
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                mouseData: delta as u32,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_input_sends_one_detent_down_or_right_without_moving_the_pointer() {
        for (axis, flags, delta) in [
            (Axis::Vertical, MOUSEEVENTF_WHEEL, -120),
            (Axis::Horizontal, MOUSEEVENTF_HWHEEL, 120),
        ] {
            let input = wheel_input(axis);
            assert_eq!(input.r#type, INPUT_MOUSE);
            let mouse = unsafe { input.Anonymous.mi };
            assert_eq!(mouse.dwFlags, flags);
            assert_eq!((mouse.mouseData as i32, mouse.dx, mouse.dy), (delta, 0, 0));
        }
    }
}
