//! WGC window capture. All graphics resources live on one cancellable MTA worker;
//! callers only exchange requests and immutable physical-pixel frames.
pub(crate) mod activation;
pub(crate) mod arrival;
pub(crate) mod capture;
mod compose;
mod crops;
#[cfg(test)]
pub(crate) mod diagnostics;
pub(crate) mod regions;
mod runtime;

use lexift_core::{
    Error, Result,
    ports::screen_capture::{ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest},
};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    request: Option<ScreenCaptureRequest>,
    generation: u64,
    frame: Option<Arc<ScreenCaptureFrame>>,
    error: Option<String>,
    stop: bool,
}
impl State {
    /// A request acknowledgement may change without changing its pixel content.
    fn publish(
        &mut self,
        revision: u64,
        regions: Vec<lexift_core::ports::screen_capture::CapturedRegion>,
        serial: &mut u64,
    ) {
        let pixels_changed = self.frame.as_ref().is_none_or(|old| {
            old.regions.len() != regions.len()
                || old
                    .regions
                    .iter()
                    .zip(&regions)
                    .any(|(a, b)| a.bounds != b.bounds || a.pixels != b.pixels)
        });
        if pixels_changed {
            *serial = serial.wrapping_add(1);
        }
        if pixels_changed
            || self
                .frame
                .as_ref()
                .is_some_and(|old| old.revision != revision)
        {
            self.frame = Some(Arc::new(ScreenCaptureFrame {
                revision,
                serial: *serial,
                regions,
            }));
        }
    }
    fn replace_request(&mut self, request: ScreenCaptureRequest) -> bool {
        if self.request.as_ref() == Some(&request) {
            return false;
        }
        // A completed ROI remains useful during continuous drawing. Changes to
        // exclusion/coverage policy invalidate it; clear always ends the epoch.
        if self.request.as_ref().is_none_or(|old| {
            old.paused_regions != request.paused_regions
                || old.excluded_windows != request.excluded_windows
        }) {
            self.generation = self.generation.wrapping_add(1);
        }
        self.request = Some(request);
        self.error = None;
        true
    }
}
type Shared = Arc<(Mutex<State>, Condvar)>;

pub(crate) struct WindowsScreenCapturePort {
    shared: Shared,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl WindowsScreenCapturePort {
    pub(crate) fn new() -> Option<Self> {
        if !activation::supported().ok()? {
            return None;
        }
        Some(Self {
            shared: Arc::new((Mutex::new(State::default()), Condvar::new())),
            worker: Mutex::new(None),
        })
    }
}

impl ScreenCapturePort for WindowsScreenCapturePort {
    fn is_available(&self) -> bool {
        true
    }
    fn request(&self, request: ScreenCaptureRequest) -> Result<()> {
        if request
            .regions
            .iter()
            .any(|r| r.right <= r.left || r.bottom <= r.top)
        {
            return Err(Error::new("Invalid screen capture region"));
        }
        let mut worker = self.worker.lock().unwrap();
        if worker.is_none() {
            let shared = self.shared.clone();
            *worker = Some(
                thread::Builder::new()
                    .name("annotation-capture".into())
                    .spawn(move || run(shared))
                    .map_err(|e| Error::new(e.to_string()))?,
            );
        }
        let mut state = self.shared.0.lock().unwrap();
        if state.replace_request(request) {
            self.shared.1.notify_one();
        }
        Ok(())
    }
    fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
        let state = self.shared.0.lock().unwrap();
        if state.frame.is_none()
            && let Some(error) = &state.error
        {
            return Err(Error::new(error.clone()));
        }
        Ok(state.frame.clone())
    }
    fn clear(&self) {
        let mut state = self.shared.0.lock().unwrap();
        state.request = None;
        state.frame = None;
        state.error = None;
        state.generation = state.generation.wrapping_add(1);
        self.shared.1.notify_one();
    }
}
impl Drop for WindowsScreenCapturePort {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().stop = true;
        self.shared.1.notify_one();
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

fn run(shared: Shared) {
    let mut backend = None;
    let mut serial = 0;
    loop {
        let (request, generation) = {
            let mut state = shared.0.lock().unwrap();
            while !state.stop && state.request.is_none() {
                // Release textures and WGC sessions even while the process stays alive.
                backend = None;
                state = shared.1.wait(state).unwrap();
            }
            if state.stop {
                break;
            }
            (state.request.clone().unwrap(), state.generation)
        };
        let started = Instant::now();
        let result = (|| {
            if backend.is_none() {
                backend = Some(capture::Backend::new()?);
            }
            backend.as_mut().unwrap().sample(&request)
        })();
        if result.is_err() {
            discard_unhealthy(&mut backend, capture::Backend::is_healthy);
        }
        let mut state = shared.0.lock().unwrap();
        if generation == state.generation && !state.stop {
            match result {
                Ok(Some(mut regions)) => {
                    if let Some(previous) = &state.frame {
                        compose::retain_covered(
                            &mut regions,
                            &previous.regions,
                            &request.paused_regions,
                        );
                    }
                    state.publish(request.revision, regions, &mut serial);
                    state.error = None;
                }
                Ok(None) => {}
                Err(error) => state.error = Some(error.to_string()),
            }
        }
        wait_for_next_sample(&shared, state, started);
    }
}

/// Drop a lost device on its owning worker, retaining the last published CPU
/// frame. The next rate-limited sample reconstructs the capture resources.
fn discard_unhealthy<B>(backend: &mut Option<B>, healthy: impl FnOnce(&B) -> bool) {
    if backend.as_ref().is_some_and(|backend| !healthy(backend)) {
        *backend = None;
    }
}

/// Replaced requests wake the worker to observe cancellation, but do not bypass
/// the frame interval. The next sample uses only the newest request.
fn wait_for_next_sample<'a>(
    shared: &'a Shared,
    mut state: std::sync::MutexGuard<'a, State>,
    started: Instant,
) {
    while !state.stop && state.request.is_some() {
        let delay = Duration::from_millis(34).saturating_sub(started.elapsed());
        if delay.is_zero() {
            break;
        }
        state = shared.1.wait_timeout(state, delay).unwrap().0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{domain::geometry::Rect, ports::magnifier::WindowToken};

    #[test]
    fn lost_capture_resources_drop_before_retry_while_last_frame_survives() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Resource(Arc<AtomicUsize>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let mut backend = Some(Resource(drops.clone()));
        let mut state = State::default();
        let mut serial = 0;
        state.publish(
            1,
            vec![lexift_core::ports::screen_capture::CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                pixels: vec![10, 20, 30, 255],
            }],
            &mut serial,
        );
        let last = state.frame.clone().unwrap();
        discard_unhealthy(&mut backend, |_| true);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        discard_unhealthy(&mut backend, |_| false);
        assert!(backend.is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&last, state.frame.as_ref().unwrap()));
        backend = Some(Resource(drops.clone()));
        state.publish(
            1,
            vec![lexift_core::ports::screen_capture::CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                pixels: vec![40, 50, 60, 255],
            }],
            &mut serial,
        );
        assert!(state.frame.as_ref().unwrap().serial > last.serial);
        drop(backend);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unchanged_pixels_acknowledge_request_without_new_content_version() {
        use lexift_core::ports::screen_capture::CapturedRegion;
        let mut state = State::default();
        let mut serial = 0;
        let mut regions = vec![CapturedRegion {
            bounds: Rect {
                left: -1,
                top: 0,
                right: 0,
                bottom: 1,
            },
            pixels: vec![20, 30, 40, 255],
        }];
        state.publish(1, regions.clone(), &mut serial);
        let first = state.frame.clone().unwrap();
        state.publish(1, regions.clone(), &mut serial);
        assert!(Arc::ptr_eq(&first, state.frame.as_ref().unwrap()));
        state.publish(2, regions.clone(), &mut serial);
        assert_eq!(state.frame.as_ref().unwrap().revision, 2);
        assert_eq!(state.frame.as_ref().unwrap().serial, first.serial);
        regions[0].pixels[0] += 1;
        state.publish(2, regions.clone(), &mut serial);
        assert_eq!(serial, first.serial + 1);
        regions[0].bounds.left -= 1;
        regions[0].bounds.right -= 1;
        state.publish(3, regions, &mut serial);
        assert_eq!(serial, first.serial + 2);
    }

    #[test]
    fn roi_updates_preserve_capture_epoch_but_policy_changes_invalidate_it() {
        let region = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let mut request = ScreenCaptureRequest {
            revision: 1,
            regions: vec![region],
            paused_regions: vec![],
            excluded_windows: vec![],
        };
        let mut state = State::default();
        assert!(state.replace_request(request.clone()));
        let epoch = state.generation;
        assert!(!state.replace_request(request.clone()));
        request.revision = 2;
        request.regions[0].right = 200;
        assert!(state.replace_request(request.clone()));
        assert_eq!(
            state.generation, epoch,
            "continuous geometry updates must allow an in-flight sample to complete"
        );
        request.revision = 3;
        request.paused_regions.push(region);
        state.replace_request(request.clone());
        assert_ne!(state.generation, epoch);
        let covered_epoch = state.generation;
        request.revision = 4;
        request.paused_regions.clear();
        state.replace_request(request);
        assert_ne!(
            state.generation, covered_epoch,
            "uncovering invalidates covered-policy samples"
        );
    }

    /// Opt-in comparison against a caller-owned static scene. No ordinary test
    /// run reads the desktop; the scene contains rounded, alpha and color-key windows.
    #[test]
    #[ignore = "requires MOSAIC_TEST_SCENE_HWND and MOSAIC_TEST_SCENE_PID"]
    fn live_transparency_and_rounding_match_visible_desktop() {
        use windows::Win32::{
            Foundation::HWND,
            Graphics::Gdi::{GetDC, GetPixel, ReleaseDC},
            UI::{
                HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
                WindowsAndMessaging::GetWindowThreadProcessId,
            },
        };
        let hwnd: isize = std::env::var("MOSAIC_TEST_SCENE_HWND")
            .unwrap()
            .parse()
            .unwrap();
        let expected_pid: u32 = std::env::var("MOSAIC_TEST_SCENE_PID")
            .unwrap()
            .parse()
            .unwrap();
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(HWND(hwnd as _), Some(&mut owner));
        }
        assert_eq!(
            owner, expected_pid,
            "test scene is no longer owned by the caller"
        );
        let bounds = Rect {
            left: 1200,
            top: 400,
            right: 1800,
            bottom: 800,
        };
        let port = WindowsScreenCapturePort::new().expect("WGC supported");
        port.request(ScreenCaptureRequest {
            revision: 81,
            regions: vec![bounds],
            paused_regions: Vec::new(),
            excluded_windows: Vec::new(),
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Ok(Some(frame)) = port.latest() {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "no composed frame: {:?}",
                port.latest().err()
            );
            thread::sleep(Duration::from_millis(34));
        };
        let region = &frame.regions[0];
        if let Ok(directory) = std::env::var("MOSAIC_TEST_SNAPSHOT_DIR") {
            std::fs::write(
                std::path::Path::new(&directory).join("transparent-captured.rgba"),
                &region.pixels,
            )
            .unwrap();
        }
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let dc = unsafe { GetDC(None) };
        let samples: Vec<_> = [
            (50, 50),
            (105, 85),
            (120, 170),
            (220, 170),
            (415, 275),
            (470, 150),
            (360, 60),
            (380, 100),
        ]
        .into_iter()
        .map(|(x, y)| {
            let color = unsafe { GetPixel(dc, bounds.left + x, bounds.top + y) }.0;
            let screen = [color as u8, (color >> 8) as u8, (color >> 16) as u8];
            let index = (y as usize * 600 + x as usize) * 4;
            (
                (x, y),
                screen,
                region.pixels[index..index + 4].to_vec(),
                color,
            )
        })
        .collect();
        unsafe {
            ReleaseDC(None, dc);
            SetThreadDpiAwarenessContext(previous);
        }
        for ((x, y), screen, captured, color) in samples {
            println!("({x},{y}) screen={screen:?} captured={captured:?}");
            assert_ne!(color, u32::MAX, "desktop pixel read failed");
            assert_eq!(captured[3], 255);
            assert!(
                screen
                    .iter()
                    .zip(&captured)
                    .all(|(a, b)| a.abs_diff(*b) <= 3),
                "composed transparency differs at ({x},{y}): screen={screen:?}, captured={captured:?}"
            );
        }
        let cropped = vec![
            Rect {
                left: 1210,
                top: 410,
                right: 1340,
                bottom: 610,
            },
            Rect {
                left: 1290,
                top: 500,
                right: 1450,
                bottom: 700,
            },
            Rect {
                left: 1670,
                top: 450,
                right: 1790,
                bottom: 790,
            },
        ];
        port.request(ScreenCaptureRequest {
            revision: 82,
            regions: cropped.clone(),
            paused_regions: Vec::new(),
            excluded_windows: Vec::new(),
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let split = loop {
            if let Some(frame) = port.latest().unwrap().filter(|f| f.revision == 82) {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "split ROI capture did not complete"
            );
            thread::sleep(Duration::from_millis(34));
        };
        assert_eq!(split.regions.len(), cropped.len());
        for (actual, expected) in split.regions.iter().zip(&cropped) {
            assert_eq!(actual.bounds, *expected);
            let width = (expected.right - expected.left) as usize;
            for y in expected.top..expected.bottom {
                let start =
                    ((y - bounds.top) as usize * 600 + (expected.left - bounds.left) as usize) * 4;
                let dst = (y - expected.top) as usize * width * 4;
                assert_eq!(
                    &actual.pixels[dst..dst + width * 4],
                    &region.pixels[start..start + width * 4],
                    "split translucent capture differs at row {y}"
                );
            }
        }
        println!("overlapping and separated ROI captures exactly match full reference");
        port.clear();
    }

    #[test]
    #[ignore = "requires the caller-owned mosaic coverage scene and command file"]
    fn live_covered_pixels_freeze_and_popup_close_restores_updates() {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::GetWindowThreadProcessId};
        let source: isize = std::env::var("MOSAIC_TEST_SCENE_HWND")
            .unwrap()
            .parse()
            .unwrap();
        let popup: isize = std::env::var("MOSAIC_TEST_POPUP_HWND")
            .unwrap()
            .parse()
            .unwrap();
        let pid: u32 = std::env::var("MOSAIC_TEST_SCENE_PID")
            .unwrap()
            .parse()
            .unwrap();
        for token in [source, popup] {
            let mut owner = 0;
            unsafe {
                GetWindowThreadProcessId(HWND(token as _), Some(&mut owner));
            }
            assert_eq!(owner, pid, "test window no longer belongs to the scene");
        }
        let command = std::env::var("MOSAIC_TEST_SCENE_COMMAND").unwrap();
        let bounds = Rect {
            left: 1200,
            top: 400,
            right: 1800,
            bottom: 800,
        };
        let covered = Rect {
            left: 1300,
            top: 500,
            right: 1500,
            bottom: 620,
        };
        let port = WindowsScreenCapturePort::new().unwrap();
        let request = |revision, paused_regions| ScreenCaptureRequest {
            revision,
            regions: vec![bounds],
            paused_regions,
            excluded_windows: vec![WindowToken(popup)],
        };
        let pixel = |frame: &ScreenCaptureFrame, x: usize, y: usize| {
            let i = (y * 600 + x) * 4;
            frame.regions[0].pixels[i..i + 4].to_vec()
        };
        let wait = |check: &dyn Fn(&ScreenCaptureFrame) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(Some(frame)) = port.latest()
                    && check(&frame)
                {
                    break frame;
                }
                assert!(
                    Instant::now() < deadline,
                    "scene did not update: {:?}",
                    port.latest().err()
                );
                thread::sleep(Duration::from_millis(34));
            }
        };
        let save = |name, frame: &ScreenCaptureFrame| {
            if let Ok(directory) = std::env::var("MOSAIC_TEST_SNAPSHOT_DIR") {
                std::fs::write(
                    std::path::Path::new(&directory).join(name),
                    &frame.regions[0].pixels,
                )
                .unwrap();
            }
        };
        port.request(request(91, Vec::new())).unwrap();
        wait(&|f| f.revision == 91 && pixel(f, 20, 20) == [20, 100, 180, 255]);
        port.request(request(92, vec![covered])).unwrap();
        wait(&|f| f.revision == 92);
        std::fs::write(&command, "red").unwrap();
        let frozen = wait(&|f| f.revision == 92 && pixel(f, 20, 20) == [190, 30, 60, 255]);
        assert_eq!(
            pixel(&frozen, 200, 160),
            [20, 100, 180, 255],
            "covered center must keep the previous sample"
        );
        save("covered-frozen.rgba", &frozen);
        // Allow the caller-owned scene to save its visible popup before closing it.
        thread::sleep(Duration::from_millis(150));
        std::fs::write(&command, "uncover").unwrap();
        port.request(request(93, Vec::new())).unwrap();
        let restored = wait(&|f| f.revision == 93 && pixel(f, 200, 160) == [190, 30, 60, 255]);
        save("covered-restored.rgba", &restored);
        std::fs::write(&command, "green").unwrap();
        let green = wait(&|f| pixel(f, 20, 20) == [30, 190, 90, 255]);
        assert_eq!(pixel(&green, 200, 160), [30, 190, 90, 255]);
        assert!(green.serial > restored.serial && restored.serial > frozen.serial);
        port.clear();
        assert!(port.latest().unwrap().is_none());
    }

    #[test]
    fn request_changes_do_not_bypass_capture_rate_limit_and_clear_wakes_immediately() {
        let shared = Arc::new((
            Mutex::new(State {
                request: Some(ScreenCaptureRequest {
                    revision: 1,
                    regions: Vec::new(),
                    paused_regions: Vec::new(),
                    excluded_windows: Vec::new(),
                }),
                ..State::default()
            }),
            Condvar::new(),
        ));
        let changing = shared.clone();
        let updates = thread::spawn(move || {
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(1));
                let mut state = changing.0.lock().unwrap();
                state.generation += 1;
                state.request.as_mut().unwrap().revision += 1;
                changing.1.notify_one();
            }
        });
        let started = Instant::now();
        wait_for_next_sample(&shared, shared.0.lock().unwrap(), started);
        assert!(started.elapsed() >= Duration::from_millis(34));
        updates.join().unwrap();
        assert_eq!(
            shared.0.lock().unwrap().request.as_ref().unwrap().revision,
            11
        );
        shared.0.lock().unwrap().request = None;
        let started = Instant::now();
        wait_for_next_sample(&shared, shared.0.lock().unwrap(), started);
        assert!(started.elapsed() < Duration::from_millis(34));
    }

    /// Runs against a caller-owned animated window. Kept opt-in so ordinary test
    /// runs never capture the user's desktop or create native capture sessions.
    #[test]
    #[ignore = "requires an animated desktop scene and MOSAIC_TEST_EXCLUDE HWND"]
    fn live_capture_changes_pixels_and_clear_discards_old_frames() {
        let excluded: isize = std::env::var("MOSAIC_TEST_EXCLUDE")
            .unwrap()
            .parse()
            .unwrap();
        let port = WindowsScreenCapturePort::new().expect("WGC supported");
        let bounds = Rect {
            left: 1220,
            top: 430,
            right: 1650,
            bottom: 720,
        };
        port.request(ScreenCaptureRequest {
            revision: 41,
            regions: vec![bounds],
            paused_regions: Vec::new(),
            excluded_windows: vec![WindowToken(excluded)],
        })
        .unwrap();
        let start = Instant::now();
        let mut first = None;
        let mut changes = 0;
        let mut last_serial = 0;
        while start.elapsed() < Duration::from_secs(5) {
            match port.latest() {
                Ok(Some(frame)) => {
                    assert_eq!(frame.revision, 41);
                    assert_eq!(frame.regions[0].bounds, bounds);
                    assert_eq!(frame.regions[0].pixels.len(), 430 * 290 * 4);
                    if first.is_none() {
                        first = Some(frame.clone());
                    }
                    if frame.serial != last_serial {
                        changes += 1;
                        last_serial = frame.serial;
                    }
                }
                Err(error) => eprintln!("capture waiting: {error}"),
                _ => {}
            }
            thread::sleep(Duration::from_millis(40));
        }
        assert!(first.is_some(), "no clean frame was produced");
        let first = first.unwrap();
        assert_eq!(
            &first.regions[0].pixels[first.regions[0].pixels.len() - 4..],
            &[30, 190, 90, 255],
            "excluded purple overlay leaked into the green source"
        );
        assert!(changes > 5, "animated base did not update: {changes}");
        println!(
            "composed changed frames={changes} elapsed={:.2}s",
            start.elapsed().as_secs_f32()
        );
        port.clear();
        assert!(port.latest().unwrap().is_none());
        thread::sleep(Duration::from_millis(100));
        assert!(
            port.latest().unwrap().is_none(),
            "late worker result repopulated a cleared session"
        );
        port.request(ScreenCaptureRequest {
            revision: 42,
            regions: vec![bounds],
            paused_regions: Vec::new(),
            excluded_windows: vec![WindowToken(excluded)],
        })
        .unwrap();
        let restarted = Instant::now();
        loop {
            if let Some(frame) = port.latest().unwrap() {
                assert_eq!(frame.revision, 42);
                assert!(frame.serial > last_serial);
                let pixels = &frame.regions[0].pixels;
                assert_eq!(&pixels[pixels.len() - 4..], &[30, 190, 90, 255]);
                break;
            }
            assert!(
                restarted.elapsed() < Duration::from_secs(5),
                "capture did not rebuild after clear"
            );
            thread::sleep(Duration::from_millis(40));
        }
        port.clear();
    }
}
