//! Window-local WGC sampling, independent of desktop composition.
use super::*;
use lexift_core::domain::pinned_image::live::Failure;
use lexift_core::ports::screen_capture::{WindowPinCapture, WindowPinPause, WindowPinUpdate};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::SizeInt32,
    Win32::{
        Foundation::POINT,
        Graphics::Gdi::ClientToScreen,
        System::{
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::GetCurrentProcessId,
        },
        UI::HiDpi::GetDpiForWindow,
    },
};

const BUDGET: u64 = 128 * 1024 * 1024;
const FRAME_INTERVAL: Duration = Duration::from_nanos(33_333_333);

fn clock() -> i64 {
    let (mut now, mut frequency) = (0, 0);
    unsafe {
        QueryPerformanceCounter(&mut now).expect("Windows provides QPC");
        QueryPerformanceFrequency(&mut frequency).expect("Windows provides QPC frequency");
    }
    (i128::from(now) * 10_000_000 / i128::from(frequency)) as i64
}

#[derive(Clone, Copy)]
struct Geometry {
    frame: RECT,
    client: RECT,
    origin: POINT,
    dpi: u32,
}
impl Geometry {
    fn get(hwnd: HWND) -> Option<Self> {
        let (mut frame, mut client, mut origin) =
            (RECT::default(), RECT::default(), POINT::default());
        unsafe {
            GetClientRect(hwnd, &mut client).ok()?;
            if !ClientToScreen(hwnd, &mut origin).as_bool() {
                return None;
            }
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut frame as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )
            .ok()?;
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        (dpi != 0).then_some(Self {
            frame,
            client,
            origin,
            dpi,
        })
    }

    fn size(self) -> SizeInt32 {
        SizeInt32 {
            Width: self.frame.right - self.frame.left,
            Height: self.frame.bottom - self.frame.top,
        }
    }

    fn crop(self, local: Rect, initial_dpi: u32) -> Option<Rect> {
        // Rounding a negative offset toward zero could admit a pixel outside the client.
        if local.left < 0 || local.top < 0 {
            return None;
        }
        let scale = |edge| {
            ((i64::from(edge) * i64::from(self.dpi) + i64::from(initial_dpi / 2))
                / i64::from(initial_dpi)) as i32
        };
        let (left, top, right, bottom) = (
            scale(local.left),
            scale(local.top),
            scale(local.right),
            scale(local.bottom),
        );
        if left < 0
            || top < 0
            || right > self.client.right
            || bottom > self.client.bottom
            || left >= right
            || top >= bottom
        {
            return None;
        }
        let (x, y) = (
            self.origin.x - self.frame.left,
            self.origin.y - self.frame.top,
        );
        let r = Rect {
            left: x + left,
            top: y + top,
            right: x + right,
            bottom: y + bottom,
        };
        let size = self.size();
        (r.left >= 0 && r.top >= 0 && r.right <= size.Width && r.bottom <= size.Height).then_some(r)
    }
}

fn within_budget(size: SizeInt32, crop: Rect, previous_crop_bytes: u64) -> bool {
    let surface = (size.Width.max(0) as u64)
        .saturating_mul(size.Height.max(0) as u64)
        .saturating_mul(4);
    let local = ((crop.right - crop.left) as u64)
        .saturating_mul((crop.bottom - crop.top) as u64)
        .saturating_mul(4);
    // Pool, staging, CPU readback, pending, staged, redrawn, installed and renderer.
    surface
        .saturating_mul(CAPTURE_BUFFERS as u64)
        .saturating_add(local.max(previous_crop_bytes).saturating_mul(7))
        <= BUDGET
}

fn identity(hwnd: HWND) -> (u32, u32) {
    let mut process = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
    (thread, process)
}

pub(in crate::windows) fn run(
    request: WindowPinCapture,
    stop: &AtomicBool,
    publish: &mut dyn FnMut(WindowPinUpdate),
) -> Result<(), Failure> {
    capture(request, stop, publish).map_err(|error| match error {
        CaptureError::Start(reason) => reason,
        CaptureError::Native(error) => {
            tracing::debug!(%error, "Window pin capture ended");
            Failure::Unavailable
        }
    })
}

enum CaptureError {
    Start(Failure),
    Native(windows::core::Error),
}

impl From<windows::core::Error> for CaptureError {
    fn from(value: windows::core::Error) -> Self {
        Self::Native(value)
    }
}

struct Subscription {
    source: WindowCapture,
    handler: i64,
}

#[derive(Default)]
struct Timing {
    frames: u64,
    read: Duration,
    slowest_read: Duration,
}

impl Drop for Timing {
    fn drop(&mut self) {
        tracing::debug!(
            frames = self.frames,
            read_us = self.read.as_micros(),
            slowest_read_us = self.slowest_read.as_micros(),
            "Window pin readback totals"
        );
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.source.item.RemoveClosed(self.handler);
    }
}

fn capture(
    request: WindowPinCapture,
    stop: &AtomicBool,
    publish: &mut dyn FnMut(WindowPinUpdate),
) -> Result<(), CaptureError> {
    let invalid = || windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG);
    let hwnd = HWND(request.window.window.0 as _);
    let initial_identity = identity(hwnd);
    if initial_identity.0 == 0
        || initial_identity.1 != request.window.process_id
        || initial_identity.1 == unsafe { GetCurrentProcessId() }
        || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || unsafe { IsIconic(hwnd) }.as_bool()
    {
        return Err(CaptureError::Start(Failure::Changed));
    }
    let backend = Backend::new()?;
    let initial = Geometry::get(hwnd).ok_or(CaptureError::Start(Failure::Changed))?;
    let bounds = request.window.bounds;
    if (
        initial.frame.left,
        initial.frame.top,
        initial.frame.right,
        initial.frame.bottom,
    ) != (bounds.left, bounds.top, bounds.right, bounds.bottom)
    {
        return Err(CaptureError::Start(Failure::Changed));
    }
    let selection = request.selection;
    let local = Rect {
        left: selection.left - initial.origin.x,
        top: selection.top - initial.origin.y,
        right: selection.right - initial.origin.x,
        bottom: selection.bottom - initial.origin.y,
    };
    let crop = initial
        .crop(local, initial.dpi)
        .ok_or(CaptureError::Start(Failure::Selection))?;
    if !within_budget(initial.size(), crop, 0) {
        return Err(CaptureError::Start(Failure::BufferLimit));
    }
    let owner = thread::current();
    let notice = crate::windows::screen_capture::arrival::Notice::new({
        let owner = owner.clone();
        move || owner.unpark()
    });
    let closed = Arc::new(AtomicBool::new(false));
    let mut first = true;
    let mut current: Option<Subscription> = None;
    let mut confirmed = None;
    let mut paused = None;
    let mut fresh_after = clock();
    let mut deadline = Instant::now();
    let mut previous_crop_bytes = 0;
    let startup = Instant::now();
    let mut timing = Timing::default();
    while !stop.load(Ordering::Acquire)
        && !closed.load(Ordering::Acquire)
        && identity(hwnd) == initial_identity
    {
        if first && startup.elapsed() >= Duration::from_secs(3) {
            return Err(invalid().into());
        }
        let current_geometry = if unsafe { IsIconic(hwnd) }.as_bool() {
            Err(WindowPinPause::Minimized)
        } else {
            let g = Geometry::get(hwnd).ok_or_else(invalid)?;
            g.crop(local, initial.dpi)
                .ok_or(WindowPinPause::OutsideClient)
                .and_then(|crop| {
                    if within_budget(g.size(), crop, previous_crop_bytes) {
                        Ok((g.size(), crop, g.dpi))
                    } else {
                        Err(WindowPinPause::BufferLimit)
                    }
                })
        };
        let reason = current_geometry.as_ref().err().copied();
        if paused != reason {
            paused = reason;
            if let Some(reason) = reason {
                publish(WindowPinUpdate::Pause(reason));
            }
        }
        let next = current_geometry.ok();
        if confirmed != next {
            confirmed = next;
            fresh_after = clock();
            if let Some(subscription) = &mut current {
                let source = &mut subscription.source;
                drop(latest_pending(CAPTURE_BUFFERS as usize, || {
                    source.pool.TryGetNextFrame().ok().map(FrameLease)
                }));
            }
        }
        if let Some((size, region, dpi)) = next {
            if current.is_none() {
                let source = WindowCapture::new(
                    hwnd.0 as isize,
                    &backend.interop,
                    &backend.winrt,
                    Some(&notice),
                )?;
                let closed = closed.clone();
                let owner = owner.clone();
                let handler = source.item.Closed(&TypedEventHandler::new(move |_, _| {
                    closed.store(true, Ordering::Release);
                    owner.unpark();
                    Ok(())
                }))?;
                current = Some(Subscription { source, handler });
            }
            let source = &mut current.as_mut().unwrap().source;
            if source.size != size {
                source.staging = None;
                source.pool.Recreate(
                    &backend.winrt,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    CAPTURE_BUFFERS,
                    size,
                )?;
                source.size = size;
                fresh_after = clock();
            }
            notice.take();
            if Instant::now() >= deadline
                && let Some(frame) = latest_pending(CAPTURE_BUFFERS as usize, || {
                    source.pool.TryGetNextFrame().ok().map(FrameLease)
                })
                && frame.0.ContentSize()? == size
                && frame.0.SystemRelativeTime()?.Duration >= fresh_after
            {
                let sampled = Instant::now();
                let access: IDirect3DDxgiInterfaceAccess = frame.0.Surface()?.cast()?;
                source.texture = Some(unsafe { access.GetInterface()? });
                let mut pixels = CapturedRegion {
                    bounds: region,
                    pixels: vec![],
                };
                let bounds = Rect {
                    left: 0,
                    top: 0,
                    right: size.Width,
                    bottom: size.Height,
                };
                let read = source.read_crop(
                    bounds,
                    region,
                    &backend.device,
                    &backend.context,
                    &mut pixels,
                );
                let read_elapsed = sampled.elapsed();
                timing.read += read_elapsed;
                timing.slowest_read = timing.slowest_read.max(read_elapsed);
                source.texture = None;
                if read?
                    && pixels.bounds == region
                    && identity(hwnd) == initial_identity
                    && Geometry::get(hwnd).is_some_and(|g| {
                        g.dpi == dpi
                            && g.size() == size
                            && g.crop(local, initial.dpi) == Some(region)
                    })
                    && !unsafe { IsIconic(hwnd) }.as_bool()
                    && !closed.load(Ordering::Acquire)
                    && !stop.load(Ordering::Acquire)
                {
                    pixels.bounds = Rect {
                        left: 0,
                        top: 0,
                        right: region.right - region.left,
                        bottom: region.bottom - region.top,
                    };
                    previous_crop_bytes = pixels.pixels.len() as u64;
                    first = false;
                    timing.frames += 1;
                    // Pace readback starts. Charging the preceding readback to the
                    // next interval would steadily lower the effective capture rate.
                    deadline = (deadline + FRAME_INTERVAL).max(sampled + FRAME_INTERVAL / 2);
                    publish(WindowPinUpdate::Frame(pixels));
                }
            }
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(1), Duration::from_millis(12));
        thread::park_timeout(wait);
    }
    Ok(())
}

#[test]
fn window_pin_accepts_exact_client_edges_and_rejects_one_pixel_overflow() {
    let geometry = Geometry {
        frame: RECT {
            left: 2560,
            top: 0,
            right: 4480,
            bottom: 1080,
        },
        client: RECT {
            right: 1920,
            bottom: 1080,
            ..Default::default()
        },
        origin: POINT { x: 2560, y: 0 },
        dpi: 96,
    };
    let full = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    assert_eq!(geometry.crop(full, 96), Some(full));
    for outside in [
        Rect { left: -1, ..full },
        Rect { top: -1, ..full },
        Rect {
            right: 1921,
            ..full
        },
        Rect {
            bottom: 1081,
            ..full
        },
    ] {
        assert_eq!(geometry.crop(outside, 96), None);
    }
}

#[test]
fn window_pin_budget_includes_existing_crop_and_pool() {
    let rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    assert!(within_budget(
        SizeInt32 {
            Width: 1920,
            Height: 1080
        },
        rect,
        0
    ));
    assert!(!within_budget(
        SizeInt32 {
            Width: 7680,
            Height: 4320
        },
        rect,
        0
    ));
    assert!(!within_budget(
        SizeInt32 {
            Width: 1920,
            Height: 1080
        },
        rect,
        BUDGET
    ));
}
