use std::mem::size_of;

use lexift_core::{
    Error, Result,
    domain::geometry::{Point, Rect},
    ports::screen::ScreenPort,
};
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    UI::WindowsAndMessaging::GetCursorPos,
};

/// Provides physical cursor and monitor work-area coordinates on Windows.
pub(crate) struct WindowsScreenPort;

impl WindowsScreenPort {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl ScreenPort for WindowsScreenPort {
    fn cursor_position(&self) -> Result<Point> {
        cursor_position()
    }

    fn work_area_for_point(&self, point: Point) -> Result<Rect> {
        let native_point = POINT {
            x: point.x,
            y: point.y,
        };
        let monitor = unsafe { MonitorFromPoint(native_point, MONITOR_DEFAULTTONEAREST) };
        if monitor.0.is_null() {
            tracing::warn!("MonitorFromPoint did not return a Windows monitor");
            return Err(Error::new("Could not find a monitor for the popup"));
        }

        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let error = windows::core::Error::from_thread();
            tracing::warn!(
                hresult = format_args!("{:#010X}", error.code().0 as u32),
                "GetMonitorInfoW failed"
            );
            return Err(Error::new("Could not get the monitor work area"));
        }

        Ok(Rect {
            left: info.rcWork.left,
            top: info.rcWork.top,
            right: info.rcWork.right,
            bottom: info.rcWork.bottom,
        })
    }
}

pub(super) fn cursor_position() -> Result<Point> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.map_err(|error| {
        tracing::warn!(
            hresult = format_args!("{:#010X}", error.code().0 as u32),
            "GetCursorPos failed"
        );
        Error::new("Could not get the cursor position")
    })?;
    Ok(Point {
        x: point.x,
        y: point.y,
    })
}
