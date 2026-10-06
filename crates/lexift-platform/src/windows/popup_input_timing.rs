//! Optional bounded OS-message diagnostics, independent of the UI input payload.
use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HWND,
    System::SystemInformation::GetTickCount,
    UI::WindowsAndMessaging::{
        GetMessageTime, WM_CANCELMODE, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY,
    },
};
struct Drag {
    at: Instant,
    ages: Vec<u32>,
    callbacks: Vec<f64>,
}
thread_local! { static DRAGS: RefCell<HashMap<usize, Drag>> = RefCell::new(HashMap::new()); }
pub(super) struct Span {
    hwnd: usize,
    at: Instant,
    finish: bool,
}
pub(super) fn begin(hwnd: HWND, message: u32) -> Option<Span> {
    if !tracing::enabled!(target: "lexift_platform::annotation_drag_timing", tracing::Level::DEBUG)
    {
        return None;
    }
    let key = hwnd.0 as usize;
    DRAGS
        .try_with(|slot| {
            let mut slot = slot.borrow_mut();
            if message == WM_NCDESTROY {
                slot.remove(&key);
                return None;
            }
            if matches!(message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) {
                slot.insert(
                    key,
                    Drag {
                        at: Instant::now(),
                        ages: Vec::with_capacity(128),
                        callbacks: Vec::with_capacity(128),
                    },
                );
            }
            if !matches!(
                message,
                WM_LBUTTONDOWN
                    | WM_LBUTTONDBLCLK
                    | WM_LBUTTONUP
                    | WM_MOUSEMOVE
                    | WM_CANCELMODE
                    | WM_CAPTURECHANGED
            ) {
                return None;
            }
            let drag = slot.get_mut(&key)?;
            let within = drag.at.elapsed() <= Duration::from_millis(500);
            if within && drag.ages.len() < 128 {
                // GetMessageTime is millisecond-quantized and refers to the dequeued
                // OS message, not device hardware or compositor presentation time.
                drag.ages
                    .push(unsafe { GetTickCount().wrapping_sub(GetMessageTime() as u32) });
            }
            let finish = matches!(message, WM_LBUTTONUP | WM_CANCELMODE | WM_CAPTURECHANGED);
            (within || finish).then(|| Span {
                hwnd: key,
                at: Instant::now(),
                finish,
            })
        })
        .ok()
        .flatten()
}
impl Drop for Span {
    fn drop(&mut self) {
        let report = DRAGS
            .try_with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    return None;
                };
                let drag = slot.get_mut(&self.hwnd)?;
                if drag.at.elapsed() <= Duration::from_millis(500) && drag.callbacks.len() < 128 {
                    drag.callbacks.push(self.at.elapsed().as_secs_f64() * 1000.);
                }
                self.finish.then(|| slot.remove(&self.hwnd)).flatten()
            })
            .ok()
            .flatten();
        if let Some(mut drag) = report {
            drag.ages.sort_unstable();
            drag.callbacks.sort_unstable_by(f64::total_cmp);
            let age_max_ms = drag.ages.last().copied().unwrap_or(0);
            let callback_max_ms = drag.callbacks.last().copied().unwrap_or(0.);
            tracing::debug!(target: "lexift_platform::annotation_drag_timing", hwnd = self.hwnd,
                samples = drag.ages.len(), age_max_ms, callback_max_ms,
                "Native pointer first 500ms; age is OS message queue age, not hardware/photon latency");
        }
    }
}
