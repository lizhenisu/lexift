//! Opt-in throughput evidence for the production worker with an owned 60 Hz source.
//! Present timestamps measure submissions, not photon latency or GPU execution time.
use super::*;
#[path = "performance_commit.rs"]
mod commit_cpu;
#[path = "performance_interval.rs"]
mod interval_work;
#[path = "performance_lifecycle.rs"]
mod lifecycle;
#[path = "performance_motion.rs"]
mod motion;
#[path = "performance_coverage.rs"]
mod observation_coverage;
#[path = "performance_source_scope.rs"]
mod source_scope;
#[path = "performance_source_wait.rs"]
mod source_wait;
#[path = "performance_wake.rs"]
mod wake_response;
use lexift_core::domain::{
    annotation::{Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, Style},
    geometry::Rect,
};
use lexift_core::ports::magnifier::WindowToken;
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, RECT},
        Graphics::Gdi::*,
        UI::{
            HiDpi::{
                AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetThreadDpiAwarenessContext,
                SetThreadDpiAwarenessContext,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, PCWSTR, w},
};

#[derive(Clone, Copy, Default)]
pub(super) struct Stages {
    pub capture_details: [Option<(Instant, Instant)>; 17],
    pub capture_details_overflow: bool,
    pub cpu_spans: [Option<(Instant, Instant)>; 5],
    pub enumeration: Option<(Duration, usize)>,
    pub output_retry: bool,
    pub compose_permit: Option<bool>,
    pub commit_permit: Option<bool>,
    pub capture: Duration,
    pub effects: Duration,
    pub compose: Duration,
    pub compare: Duration,
    pub commit: Duration,
    pub submission_cpu: [Duration; 3],
    pub source_changed: bool,
    pub comparison_waiting: bool,
}
pub(super) struct Attempt {
    pub stamp: RenderStamp,
    pub at: Instant,
    pub work: Duration,
    pub stages: Stages,
    pub wait: Option<RenderWait>,
}

/// Poll outcomes at the actual gates, independent of the completion wait label.
/// Source waiting can coexist with a blocked commit permit; neither is a duration.
#[derive(Default, Debug, PartialEq, Eq)]
struct PermitPolls {
    compose: usize,
    compose_blocked: usize,
    commit: usize,
    commit_blocked: usize,
    commit_blocked_source_wait: usize,
}
impl PermitPolls {
    fn observe(&mut self, stages: Stages, wait: Option<&RenderWait>) {
        if let Some(ready) = stages.compose_permit {
            self.compose += 1;
            self.compose_blocked += usize::from(!ready);
        }
        if let Some(ready) = stages.commit_permit {
            self.commit += 1;
            self.commit_blocked += usize::from(!ready);
            self.commit_blocked_source_wait +=
                usize::from(!ready && matches!(wait, Some(RenderWait::Source)));
        }
    }
}

pub(super) struct Presentation {
    pub stamp: RenderStamp,
    pub native_commit: Option<super::display_timing::NativeCommit>,
    pub counts: Vec<super::display_timing::Count>,
    pub at: Instant,
    pub sources: Vec<super::super::screen_capture::diagnostics::Published>,
}

#[derive(Default)]
struct OwnedCommits {
    mixed: usize,
    unmatched: usize,
    repeated: usize,
    times: Vec<Instant>,
    delays: Vec<f64>,
}

// sync QPC, frequency, reported WGC 100ns time, converted sync delta,
// CPU dequeue-to-sync upper bound, independent QPC sampled at dequeue.
type CaptureClockExample = (i64, i64, i64, Option<f64>, f64, Option<i64>);

#[derive(Default)]
struct OwnedLatencies {
    capture_sync: Vec<f64>,
    capture_dequeue_lower: Vec<f64>,
    capture_dequeue_upper: Vec<f64>,
    capture_invalid: usize,
    capture_clock_examples: Vec<CaptureClockExample>,
    dequeue_lower: Vec<f64>,
    dequeue_upper: Vec<f64>,
    submit_lower: Vec<f64>,
    submit_upper: Vec<f64>,
    observation_lower: Vec<f64>,
    observation_upper: Vec<f64>,
    clock_spans: Vec<f64>,
    multi_surface: usize,
    invalid: usize,
    boundary: usize,
    mixed: usize,
    repeated: usize,
}

/// Measure one matching frozen version per native sync; never join nearest timestamps.
fn owned_latencies(
    observations: &[super::display_timing::Observation],
    presentations: &[Presentation],
    events: &[super::super::screen_capture::diagnostics::Event],
    token: isize,
    start: Instant,
) -> OwnedLatencies {
    let frames = owned_frames(events, token, start);
    let mut result = OwnedLatencies::default();
    let mut seen = HashSet::new();
    for (observation, commit) in super::display_timing::match_commits(observations, presentations) {
        // The current source list is frame-wide; it cannot identify another monitor's pixels.
        if commit.counts.len() != 1 {
            result.multi_surface += 1;
            continue;
        }
        let mut owned = commit.sources.iter().filter(|source| source.token == token);
        let Some(version) = owned.next() else {
            continue;
        };
        if owned.next().is_some() {
            result.mixed += 1;
            continue;
        }
        let key = (version.sequence, version.ticks);
        let Some(dequeued) = frames.get(&key) else {
            result.boundary += 1;
            continue;
        };
        if seen.contains(&key) {
            result.repeated += 1;
            continue;
        }
        let bounds = super::display_timing::latency_bounds;
        let Some(((dl, du), (sl, su), (before, after))) = bounds(observation, *dequeued)
            .zip(bounds(observation, commit.at))
            .zip(super::display_timing::presented_window(observation))
            .map(|((dequeue, submit), window)| (dequeue, submit, window))
        else {
            result.invalid += 1;
            continue;
        };
        if commit.at < *dequeued {
            result.invalid += 1;
            continue;
        }
        // Retain existing latency samples even if the supplemental WGC clock is
        // invalid. Report rejected capture-clock evidence instead of guessing it.
        if let Some(capture) = super::display_timing::capture_to_sync_ms(observation, version.ticks)
            .filter(|capture| *capture >= du)
        {
            result.capture_sync.push(capture);
            result.capture_dequeue_lower.push(capture - du);
            result.capture_dequeue_upper.push(capture - dl);
        } else {
            result.capture_invalid += 1;
            if result.capture_clock_examples.len() < 3 {
                result.capture_clock_examples.push((
                    observation.qpc,
                    observation.frequency,
                    version.ticks,
                    super::display_timing::capture_to_sync_ms(observation, version.ticks),
                    du,
                    events.iter().find_map(|event| {
                        if event.token == token
                            && event.sequence == version.sequence
                            && let super::super::screen_capture::diagnostics::Kind::Frame {
                                ticks,
                                dequeue_qpc,
                            } = event.kind
                            && ticks == version.ticks
                        {
                            return dequeue_qpc;
                        }
                        None
                    }),
                ));
            }
        }
        seen.insert(key);
        result.dequeue_lower.push(dl);
        result.dequeue_upper.push(du);
        result.submit_lower.push(sl);
        result.submit_upper.push(su);
        result
            .observation_lower
            .push(observation.at.duration_since(after).as_secs_f64() * 1000.);
        result
            .observation_upper
            .push(observation.at.duration_since(before).as_secs_f64() * 1000.);
        result.clock_spans.push(
            observation
                .clock
                .unwrap()
                .after
                .duration_since(observation.clock.unwrap().before)
                .as_secs_f64()
                * 1000.,
        );
    }
    result
}

fn owned_frames(
    events: &[super::super::screen_capture::diagnostics::Event],
    token: isize,
    start: Instant,
) -> HashMap<(u64, i64), Instant> {
    use super::super::screen_capture::diagnostics::Kind;
    events
        .iter()
        .filter_map(|event| {
            if event.token == token
                && event.at >= start
                && let Kind::Frame { ticks, .. } = event.kind
            {
                Some(((event.sequence, ticks), event.at))
            } else {
                None
            }
        })
        .collect()
}

/// Count the first successful commit of versions dequeued inside this interval.
/// Warmup, mixed dependencies and repeats cannot inflate source throughput.
fn owned_commits(
    presentations: &[Presentation],
    events: &[super::super::screen_capture::diagnostics::Event],
    token: isize,
    start: Instant,
) -> OwnedCommits {
    let frames = owned_frames(events, token, start);
    let mut result = OwnedCommits::default();
    let mut seen = HashSet::new();
    for presentation in presentations {
        let mut owned = presentation
            .sources
            .iter()
            .filter(|source| source.token == token);
        let Some(version) = owned.next() else {
            continue;
        };
        if owned.next().is_some() {
            result.mixed += 1;
            continue;
        }
        let key = (version.sequence, version.ticks);
        let Some(at) = frames.get(&key) else {
            result.unmatched += 1;
            continue;
        };
        if !seen.insert(key) {
            result.repeated += 1;
            continue;
        }
        result.times.push(presentation.at);
        result
            .delays
            .push(presentation.at.saturating_duration_since(*at).as_secs_f64() * 1000.);
    }
    result
}

/// Configured integer mode for this window's nearest output, not a measured period.
#[derive(Debug, PartialEq, Eq)]
struct MonitorMode {
    device: String,
    bounds: [i32; 4],
    pixels: [u32; 2],
    refresh_hz: u32,
}

/// Backend teardown restores the caller's DPI context; queries still use physical bounds.
struct PhysicalModeQuery(DPI_AWARENESS_CONTEXT);
impl Drop for PhysicalModeQuery {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

fn monitor_mode(hwnd: HWND) -> std::result::Result<MonitorMode, String> {
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if previous.0.is_null() {
            return Err(format!(
                "SetThreadDpiAwarenessContext: {}",
                windows::core::Error::from_thread()
            ));
        }
        let _restore = PhysicalModeQuery(previous);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
            return Err(format!(
                "GetMonitorInfoW: {}",
                windows::core::Error::from_thread()
            ));
        }
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if !EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
        .as_bool()
        {
            return Err(format!(
                "EnumDisplaySettingsW: {}",
                windows::core::Error::from_thread()
            ));
        }
        if mode.dmDisplayFrequency <= 1 {
            return Err("display driver did not report a configured refresh rate".into());
        }
        let bounds = info.monitorInfo.rcMonitor;
        Ok(MonitorMode {
            device: String::from_utf16_lossy(&info.szDevice)
                .trim_end_matches('\0')
                .into(),
            bounds: [bounds.left, bounds.top, bounds.right, bounds.bottom],
            pixels: [mode.dmPelsWidth, mode.dmPelsHeight],
            refresh_hz: mode.dmDisplayFrequency,
        })
    }
}

fn append_monitor_mode(
    report: &mut String,
    prefix: &str,
    mode: &std::result::Result<MonitorMode, String>,
) {
    match mode {
        Ok(mode) => report.push_str(&format!(
            "{prefix}_mode_status=available\n{prefix}_mode_device={}\n{prefix}_mode_bounds={:?}\n{prefix}_mode_pixels={:?}\n{prefix}_configured_refresh_integer_hz={}\n",
            mode.device, mode.bounds, mode.pixels, mode.refresh_hz,
        )),
        Err(error) => report.push_str(&format!("{prefix}_mode_status=unavailable\n{prefix}_mode_error={error}\n")),
    }
}

#[test]
#[ignore = "owned nonactivating window mode query; requires a desktop"]
fn gpu_owned_window_reports_current_monitor_mode() {
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 104,
        bottom: 104,
    };
    let owned = window(bounds, false);
    let before = unsafe { GetThreadDpiAwarenessContext() };
    let start = monitor_mode(owned.0).unwrap();
    assert!(
        unsafe { AreDpiAwarenessContextsEqual(before, GetThreadDpiAwarenessContext()) }.as_bool()
    );
    drop(bootstrap);
    let restored = unsafe { GetThreadDpiAwarenessContext() };
    let end = monitor_mode(owned.0).unwrap();
    assert!(
        unsafe { AreDpiAwarenessContextsEqual(restored, GetThreadDpiAwarenessContext()) }.as_bool()
    );
    assert_eq!(start, end, "mode changed between probe queries");
    assert!(start.pixels[0] > 0 && start.pixels[1] > 0);
    assert!(start.refresh_hz > 1);
    assert!(!start.device.is_empty());
    assert!(start.bounds[0] <= bounds.left && start.bounds[2] >= bounds.right);
    assert!(start.bounds[1] <= bounds.top && start.bounds[3] >= bounds.bottom);
    let mut report = String::from(
        "mode_scope=owned_window_nearest_output_integer_configuration_not_qpc_or_photon\n",
    );
    append_monitor_mode(&mut report, "owned_probe", &Ok(start));
    eprintln!("{report}");
    let token = owned.0;
    drop(owned);
    assert!(!unsafe { IsWindow(Some(token)) }.as_bool());
}

struct OwnedWindow(HWND);
impl Drop for OwnedWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// A different input thread catches the same hit-test mistake as another app.
/// Joining on drop also cleans up the fixture when a native assertion fails.
struct OtherThreadWindow {
    hwnd: HWND,
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}
impl OtherThreadWindow {
    fn new(bounds: Rect) -> Self {
        let (ready, receive) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            let owned = window(bounds, false);
            // STATIC without SS_NOTIFY is itself input-transparent.
            unsafe {
                let style = GetWindowLongPtrW(owned.0, GWL_STYLE);
                SetWindowLongPtrW(owned.0, GWL_STYLE, style | 0x100);
            }
            ready.send(owned.0.0 as isize).unwrap();
            while matches!(
                stopped.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                surface::pump_messages();
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            hwnd: HWND(receive.recv_timeout(Duration::from_secs(3)).unwrap() as _),
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for OtherThreadWindow {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn window(bounds: Rect, input: bool) -> OwnedWindow {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | if input {
                    WS_EX_LAYERED | WS_EX_TRANSPARENT
                } else {
                    WINDOW_EX_STYLE(0)
                },
            w!("STATIC"),
            w!("Lexift owned GPU performance fixture"),
            WS_POPUP,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        if input {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA).unwrap();
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        OwnedWindow(hwnd)
    }
}

/// Render all bands offscreen before one publication. Capturing eight separate
/// window writes counts intermediate paints as extra source frames.
#[derive(Clone, Copy)]
struct PaintPublication {
    before: Instant,
    after: Instant,
}
/// A verified paint's BitBlt/flush interval supplies latency bounds. Neither a
/// capture timestamp nor the nearest paint is used to identify the source.
fn paint_to_sync_bounds(
    observation: &super::display_timing::Observation,
    paint: PaintPublication,
) -> Option<(f64, f64)> {
    if paint.after < paint.before {
        return None;
    }
    let (before, after) = super::display_timing::presented_window(observation)?;
    Some((
        before.checked_duration_since(paint.after)?.as_secs_f64() * 1000.,
        after.checked_duration_since(paint.before)?.as_secs_f64() * 1000.,
    ))
}

struct PaintStages {
    paint_dequeue: (f64, f64),
    dequeue_commit: f64,
    commit_sync: (f64, f64),
    paint_sync: (f64, f64),
}
/// All stages belong to one pixel-identified publication and native commit.
/// Separate percentile summaries must never be added as a total percentile.
fn paint_stages(
    observation: &super::display_timing::Observation,
    paint: PaintPublication,
    dequeue: Instant,
    commit: Instant,
) -> Option<PaintStages> {
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.;
    Some(PaintStages {
        paint_dequeue: (
            ms(dequeue.checked_duration_since(paint.after)?),
            ms(dequeue.checked_duration_since(paint.before)?),
        ),
        dequeue_commit: ms(commit.checked_duration_since(dequeue)?),
        commit_sync: super::display_timing::latency_bounds(observation, commit)?,
        paint_sync: paint_to_sync_bounds(observation, paint)?,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct PaintFreshness {
    dequeue_newer: usize,
    commit_newer: usize,
    sync_newer: (usize, usize),
}

/// Counts fully published newer paints at matching-version phase boundaries.
/// Publications overlapping a sync interval contribute only to its upper bound.
/// This is source-content age, not notification latency or physical visibility.
fn paint_freshness(
    publications: &HashMap<u32, PaintPublication>,
    paint: u32,
    dequeue: Instant,
    commit: Instant,
    sync: (Instant, Instant),
) -> Option<PaintFreshness> {
    let own = publications.get(&paint)?;
    if own.after > dequeue
        || dequeue > commit
        || commit > sync.0
        || sync.0 > sync.1
        || publications.values().any(|p| p.before > p.after)
    {
        return None;
    }
    let newer = |boundary| {
        publications
            .iter()
            .filter(|(id, publication)| **id > paint && publication.after <= boundary)
            .count()
    };
    Some(PaintFreshness {
        dequeue_newer: newer(dequeue),
        commit_newer: newer(commit),
        sync_newer: (newer(sync.0), newer(sync.1)),
    })
}

type CaptureVersion = super::super::screen_capture::diagnostics::Published;

#[derive(Default)]
struct Confirmations {
    versions: HashMap<CaptureVersion, Option<(Instant, Instant)>>,
    invalid: usize,
}
impl Confirmations {
    /// Exact candidate identity wins over a later frame's current source sequence.
    /// Duplicate confirmations are ambiguous and must not choose a convenient one.
    fn from_events(events: &[super::super::screen_capture::diagnostics::Event]) -> Self {
        use super::super::screen_capture::diagnostics::Kind;
        let mut index = Self::default();
        for event in events {
            let Kind::Confirm {
                candidate, started, ..
            } = event.kind
            else {
                continue;
            };
            let Some(candidate) =
                candidate.filter(|candidate| candidate.token == event.token && started <= event.at)
            else {
                index.invalid += 1;
                continue;
            };
            index
                .versions
                .entry(candidate)
                .and_modify(|span| *span = None)
                .or_insert(Some((started, event.at)));
        }
        index
    }

    fn phases(
        &self,
        version: CaptureVersion,
        dequeue: Instant,
        commit: Instant,
    ) -> Option<[f64; 3]> {
        let (started, confirmed) = self.versions.get(&version).copied().flatten()?;
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.;
        Some([
            ms(started.checked_duration_since(dequeue)?),
            ms(confirmed.checked_duration_since(started)?),
            ms(commit.checked_duration_since(confirmed)?),
        ])
    }
}

#[derive(Default)]
struct SourceGpuComparisons {
    versions: HashMap<CaptureVersion, Option<[f64; 3]>>,
    invalid: usize,
    skipped: usize,
}
impl SourceGpuComparisons {
    fn from_events(events: &[super::super::screen_capture::diagnostics::Event]) -> Self {
        use super::super::screen_capture::diagnostics::Kind;
        let mut index = Self::default();
        for event in events {
            match event.kind {
                Kind::GpuComparison {
                    candidate,
                    copy_ms,
                    compare_ms,
                    observed_ms,
                } => {
                    let spans = [copy_ms, compare_ms, observed_ms];
                    if candidate.token != event.token {
                        index.invalid += 1;
                        continue;
                    }
                    if spans.iter().any(|ms| !ms.is_finite() || *ms < 0.) {
                        index.invalid += 1;
                        index.versions.insert(candidate, None);
                        continue;
                    }
                    index
                        .versions
                        .entry(candidate)
                        .and_modify(|sample| *sample = None)
                        .or_insert(Some(spans));
                }
                Kind::GpuTimingInvalid { candidate } => {
                    index.invalid += 1;
                    if candidate.token == event.token {
                        index.versions.insert(candidate, None);
                    }
                }
                Kind::GpuTimingSkipped { candidate } => {
                    if candidate.token == event.token {
                        index.skipped += 1;
                    } else {
                        index.invalid += 1;
                    }
                }
                _ => {}
            }
        }
        index
    }
}

#[path = "performance_source.rs"]
mod publication;
use publication::{Publisher, SourceKind};

struct SourceFrame {
    hwnd: HWND,
    target: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: Option<HGDIOBJ>,
}
impl SourceFrame {
    fn new(hwnd: HWND) -> Self {
        unsafe {
            let target = GetDC(Some(hwnd));
            assert!(!target.is_invalid(), "owned source DC unavailable");
            let mut frame = Self {
                hwnd,
                target,
                memory: HDC::default(),
                bitmap: HBITMAP::default(),
                previous: None,
            };
            frame.memory = CreateCompatibleDC(Some(target));
            assert!(
                !frame.memory.is_invalid(),
                "owned source buffer DC unavailable"
            );
            frame.bitmap = CreateCompatibleBitmap(target, 1280, 720);
            assert!(
                !frame.bitmap.is_invalid(),
                "owned source bitmap unavailable"
            );
            let previous = SelectObject(frame.memory, frame.bitmap.into());
            assert!(
                !previous.is_invalid(),
                "owned source bitmap selection failed"
            );
            frame.previous = Some(previous);
            frame
        }
    }
    fn paint(&self, tick: u32) -> PaintPublication {
        unsafe {
            for band in 0..8 {
                let brush = CreateSolidBrush(band_color(tick, band));
                assert!(!brush.is_invalid(), "owned source brush unavailable");
                let filled = FillRect(
                    self.memory,
                    &RECT {
                        left: (band * 160) as i32,
                        top: 0,
                        right: ((band + 1) * 160) as i32,
                        bottom: 720,
                    },
                    brush,
                );
                let _ = DeleteObject(brush.into());
                assert_ne!(filled, 0, "owned source buffer drawing failed");
            }
            let before = Instant::now();
            BitBlt(
                self.target,
                0,
                0,
                1280,
                720,
                Some(self.memory),
                0,
                0,
                SRCCOPY,
            )
            .expect("owned source frame publication failed");
            assert!(GdiFlush().as_bool(), "owned source drawing failed");
            PaintPublication {
                before,
                after: Instant::now(),
            }
        }
    }
    /// Check the owned window before capture starts; no output sampling is added
    /// to the timed worker or production rendering path.
    fn verify(&self, tick: u32) {
        unsafe {
            for band in 0..8 {
                for y in [0, 719] {
                    assert_eq!(
                        GetPixel(self.target, (band * 160 + 80) as i32, y),
                        band_color(tick, band),
                        "source publication mixed frame bands"
                    );
                }
            }
        }
    }
}
impl Drop for SourceFrame {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.take() {
                SelectObject(self.memory, previous);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.memory.is_invalid() {
                let _ = DeleteDC(self.memory);
            }
            ReleaseDC(Some(self.hwnd), self.target);
        }
    }
}
fn band_color(tick: u32, band: u32) -> COLORREF {
    assert!(tick < 1 << 24, "owned source identity exhausted");
    COLORREF(tick ^ super::paint_probe::band_mask(band))
}

fn scene(bounds: Rect, input: HWND, mode: usize) -> AnnotationScene {
    let points = [
        (80., 80.),
        (1200., 80.),
        (1200., 360.),
        (80., 360.),
        (80., 640.),
        (1200., 640.),
    ];
    let masks = if mode == 1 {
        let mut pixels = vec![0; 1280 * 720 * 4];
        for y in 0..720 {
            for x in 0..1280 {
                let covered = points.windows(2).any(|segment| {
                    let (a, b) = (segment[0], segment[1]);
                    let (dx, dy): (f32, f32) = (b.0 - a.0, b.1 - a.1);
                    let t = (((x as f32 - a.0) * dx + (y as f32 - a.1) * dy) / (dx * dx + dy * dy))
                        .clamp(0., 1.);
                    (x as f32 - a.0 - t * dx).powi(2) + (y as f32 - a.1 - t * dy).powi(2)
                        <= 40. * 40.
                });
                if covered {
                    pixels[(y * 1280 + x) * 4..(y * 1280 + x + 1) * 4].fill(255);
                }
            }
        }
        pixels
    } else {
        vec![255; 1280 * 720 * 4]
    };
    let layers = (0..if mode == 2 { 4 } else { 1 })
        .map(|index| {
            RenderLayer::Mosaic(MosaicLayer {
                object: Arc::new(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: if mode == 1 {
                        Kind::MosaicBrush
                    } else {
                        Kind::MosaicArea
                    },
                    bounds: Bounds::from_corners(
                        (bounds.left as f32, bounds.top as f32),
                        (bounds.right as f32, bounds.bottom as f32),
                    ),
                    points: if mode == 1 {
                        points
                            .iter()
                            .map(|&(x, y)| (x + bounds.left as f32, y + bounds.top as f32))
                            .collect()
                    } else {
                        Vec::new()
                    },
                    style: Style::default(),
                    extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 100, 80., 1.)),
                }),
                masks: vec![RasterLayer {
                    key: RasterKey {
                        asset: index + 1,
                        revision: 1,
                    },
                    bounds,
                    pixels: masks.clone().into(),
                }]
                .into(),
            })
        })
        .collect::<Vec<_>>();
    AnnotationScene {
        stamp: RenderStamp {
            session: 900 + mode as u64,
            revision: 1,
        },
        surfaces: vec![RenderSurface {
            input_window: WindowToken(input.0 as isize),
            bounds,
            scale: 1.,
        }]
        .into(),
        layers: layers.into(),
        interaction: Arc::from([]),
        excluded_windows: Arc::from([]),
        paused_regions: Arc::from([]),
    }
}

#[derive(Clone, Copy, Debug)]
enum CompletionScope {
    Disabled,
    Owned,
    All,
    CaptureAndOutput,
}

fn run_case(mode: usize, label: &str) {
    run_case_with_probe(mode, label, false, false, CompletionScope::All);
}
fn run_case_with_probe(
    mode: usize,
    label: &str,
    probe_enabled: bool,
    profile_capture: bool,
    completion_scope: CompletionScope,
) {
    run_case_with_source(
        mode,
        label,
        probe_enabled,
        profile_capture,
        completion_scope,
        SourceKind::Gdi,
    );
}

#[derive(Clone, Copy)]
enum ClockProbes {
    WaitsAndEnumeration,
    WaitsOnly,
    CoveredAllSources,
    CoveredOwnedSource,
    CompositionAndEnumeration,
    CompositionAndFallback,
    None,
    Composition,
    CompositionAndWaits,
}
fn run_case_with_source(
    mode: usize,
    label: &str,
    probe_enabled: bool,
    profile_capture: bool,
    completion_scope: CompletionScope,
    source_kind: SourceKind,
) {
    run_case_with_composition_probe(
        mode,
        label,
        probe_enabled,
        profile_capture,
        completion_scope,
        source_kind,
        ClockProbes::None,
    );
}
fn run_case_with_composition_probe(
    mode: usize,
    label: &str,
    probe_enabled: bool,
    profile_capture: bool,
    completion_scope: CompletionScope,
    source_kind: SourceKind,
    clocks: ClockProbes,
) {
    let composition_probe = !matches!(
        clocks,
        ClockProbes::None | ClockProbes::WaitsOnly | ClockProbes::WaitsAndEnumeration
    );
    let native_wait_probe = matches!(
        clocks,
        ClockProbes::WaitsAndEnumeration
            | ClockProbes::WaitsOnly
            | ClockProbes::CompositionAndWaits
            | ClockProbes::CoveredAllSources
            | ClockProbes::CoveredOwnedSource
    );
    let covered = matches!(
        clocks,
        ClockProbes::CoveredAllSources | ClockProbes::CoveredOwnedSource
    );
    let only_owned = matches!(clocks, ClockProbes::CoveredOwnedSource);
    let enumeration_probe = matches!(
        clocks,
        ClockProbes::CompositionAndEnumeration | ClockProbes::WaitsAndEnumeration
    );
    let force_signal_fallback = matches!(clocks, ClockProbes::CompositionAndFallback);
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    // Set physical coordinates before creating owned windows, as the app does.
    let bootstrap = Backend::new().unwrap();
    let adapter_name = {
        let device: windows::Win32::Graphics::Dxgi::IDXGIDevice =
            bootstrap.device().cast().unwrap();
        let desc = unsafe { device.GetAdapter().unwrap().GetDesc().unwrap() };
        String::from_utf16_lossy(&desc.Description)
            .trim_end_matches('\0')
            .to_owned()
    };
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    let source_mode_start = monitor_mode(source.0);
    let input_mode_start = monitor_mode(input.0);
    let source_frame = Publisher::new(source.0, &bootstrap, source_kind);
    source_frame.paint(0);
    source_frame.verify(0);
    drop(bootstrap);
    let port = WindowsAnnotationRenderPort::new().unwrap();
    port.shared.0.lock().unwrap().composition_probe = composition_probe;
    port.shared.0.lock().unwrap().record_native_waits = native_wait_probe;
    port.shared.0.lock().unwrap().enumeration_probe = enumeration_probe;
    port.shared.0.lock().unwrap().disable_precise_clock = force_signal_fallback;
    if probe_enabled {
        port.shared.0.lock().unwrap().paint_probe_token =
            Some((source.0.0 as isize, profile_capture));
        use super::super::screen_capture::capture::CompletionQueries;
        port.shared.0.lock().unwrap().completion_queries = match completion_scope {
            CompletionScope::Disabled => CompletionQueries::Disabled,
            CompletionScope::Owned => CompletionQueries::Source(source.0.0 as isize),
            CompletionScope::All | CompletionScope::CaptureAndOutput => CompletionQueries::All,
        };
        port.shared.0.lock().unwrap().output_completion_reference =
            !matches!(completion_scope, CompletionScope::CaptureAndOutput);
    }
    let document = if covered {
        assert_eq!(mode, 0);
        source_scope::document(bounds, input.0, source.0, only_owned)
    } else {
        scene(bounds, input.0, mode)
    };
    port.submit(Arc::new(document)).unwrap();
    let period = Duration::from_secs_f64(1. / 60.);
    let start = Instant::now();
    let warmup = Duration::from_secs(5);
    let duration = Duration::from_secs(30);
    let mut next = start;
    let mut tick = 1;
    let mut paint_publications = HashMap::new();
    let mut samples_start = None;
    let mut source_times = Vec::new();
    while start.elapsed() < warmup + duration {
        surface::pump_messages();
        let now = Instant::now();
        if now >= next {
            let publication = source_frame.paint(tick);
            if probe_enabled {
                assert!(paint_publications.insert(tick, publication).is_none());
            }
            tick += 1;
            next += period;
            if next <= now {
                next = now + period;
            }
            if start.elapsed() >= warmup {
                if samples_start.is_none() {
                    samples_start = Some(Instant::now());
                    let mut state = port.shared.0.lock().unwrap();
                    state.presentations.clear();
                    state.source_presentations.clear();
                    state.source_presentations_dropped = 0;
                    state.display_observations.clear();
                    state.display_observations_dropped = 0;
                    state.display_observation_errors = 0;
                    state.attempts.clear();
                    state.native_waits.clear();
                    state.native_waits_dropped = 0;
                    state.gpu_samples.clear();
                    state.capture_events.clear();
                    state.capture_events_dropped = 0;
                    state.paint_samples.clear();
                    state.paint_probe_summary = Default::default();
                    state.record_presentations = true;
                }
                source_times.push(Instant::now());
            }
        }
        port.latest().unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    let elapsed = samples_start.unwrap().elapsed().as_secs_f64();
    let fallback_waits = port.shared.0.lock().unwrap().fallback_waits;
    if force_signal_fallback {
        assert!(fallback_waits > 0, "benchmark did not enter event fallback");
    }
    let (
        times,
        source_presentations,
        source_presentations_dropped,
        display_observations,
        display_dropped,
        display_errors,
        captures,
        attempts,
        gpu_samples,
        dropped,
        invalid,
        pending,
        capture_events,
        capture_dropped,
        native_waits,
        native_waits_dropped,
    ) = {
        let mut state = port.shared.0.lock().unwrap();
        state.record_presentations = false;
        (
            std::mem::take(&mut state.presentations),
            std::mem::take(&mut state.source_presentations),
            state.source_presentations_dropped,
            std::mem::take(&mut state.display_observations),
            state.display_observations_dropped,
            state.display_observation_errors,
            state.captures,
            std::mem::take(&mut state.attempts),
            std::mem::take(&mut state.gpu_samples),
            state.gpu_query_dropped,
            state.gpu_query_invalid,
            state.gpu_query_pending,
            std::mem::take(&mut state.capture_events),
            state.capture_events_dropped,
            std::mem::take(&mut state.native_waits),
            state.native_waits_dropped,
        )
    };
    let percentile = |times: &[Instant]| {
        let mut intervals: Vec<_> = times
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.)
            .collect();
        intervals.sort_by(f64::total_cmp);
        intervals[((intervals.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)]
    };
    if only_owned {
        assert_eq!(
            captures, 1,
            "single-source fixture gained an unexpected capture"
        );
        assert!(
            capture_events
                .iter()
                .all(|event| event.token == source.0.0 as isize)
        );
    }
    let extra_arrivals = capture_events
        .iter()
        .filter(|event| {
            event.token != source.0.0 as isize
                && matches!(
                    event.kind,
                    super::super::screen_capture::diagnostics::Kind::Arrived { .. }
                )
        })
        .count();
    let extra_wakes = capture_events
        .iter()
        .filter(|event| {
            event.token != source.0.0 as isize
                && matches!(
                    event.kind,
                    super::super::screen_capture::diagnostics::Kind::Arrived { woke: true, .. }
                )
        })
        .count();
    assert!(
        times.len() > 2 && source_times.len() > 2,
        "fixture did not produce frames"
    );
    let mut report = format!(
        "case={label}\nprofile=release\nadapter={adapter_name}\nmeasurement_seconds={elapsed:.3}\nsource_frames={}\nsource_fps={:.3}\nsource_interval_p95_ms={:.3}\npresentations={}\npresent_fps={:.3}\npresent_interval_p95_ms={:.3}\ncaptured_windows={captures}\ninput_to_visible_ms=not_measured\nscope=production_native_worker_with_owned_source_not_full_app\nsource_publication={}\nsource_publication_scope=api_submission_call_bounds_not_gpu_completion_or_photon\nsource_to_candidate=frozen_texture_version_metadata_not_pixel_verified\neffective_display_fps=not_measured\n",
        source_times.len(),
        source_times.len() as f64 / elapsed,
        percentile(&source_times),
        times.len(),
        times.len() as f64 / elapsed,
        percentile(&times),
        source_kind.label()
    );
    let source_mode_end = monitor_mode(source.0);
    let input_mode_end = monitor_mode(input.0);
    report.push_str("configured_mode_scope=nearest_owned_window_output_integer_configuration_not_qpc_or_photon\n");
    append_monitor_mode(&mut report, "owned_source_start", &source_mode_start);
    append_monitor_mode(&mut report, "owned_source_end", &source_mode_end);
    append_monitor_mode(&mut report, "owned_input_start", &input_mode_start);
    append_monitor_mode(&mut report, "owned_input_end", &input_mode_end);
    report.push_str(&format!(
        "owned_source_mode_endpoints_match={}\nowned_input_mode_endpoints_match={}\nowned_windows_start_same_output={}\n",
        source_mode_start.is_ok() && source_mode_start == source_mode_end,
        input_mode_start.is_ok() && input_mode_start == input_mode_end,
        source_mode_start.is_ok() && source_mode_start == input_mode_start,
    ));
    let p95 = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values
            .get(((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.)
    };
    let cpu = |select: fn(&Attempt) -> Duration| {
        p95(attempts
            .iter()
            .map(|attempt| select(attempt).as_secs_f64() * 1000.)
            .collect())
    };
    let gpu: Vec<_> = gpu_samples
        .iter()
        .filter(|sample| sample.at >= samples_start.unwrap() && sample.full_frame)
        .collect();
    assert!(!gpu.is_empty(), "no completed GPU timestamp evidence");
    let stage = |index| p95(gpu.iter().map(|sample| sample.stages_ms[index]).collect());
    report.push_str(&format!("gpu_timing_scope=complete_processing_attempt_before_output_confirmation_including_feed_gaps_not_shader_active_time\ngpu_query_samples={}\ngpu_full_frame_samples={}\ngpu_query_dropped={dropped}\ngpu_query_invalid={invalid}\ngpu_query_pending_at_end={pending}\ngpu_execution_p95_ms={:.3}\ngpu_capture_p95_ms={:.3}\ngpu_effects_p95_ms={:.3}\ngpu_compose_p95_ms={:.3}\ngpu_output_compare_p95_ms={:.3}\ngpu_end_marker_p95_ms={:.3}\ngpu_result_observed_delay_p95_ms={:.3}\n",
        gpu_samples.len(), gpu.len(), p95(gpu.iter().map(|sample| sample.stages_ms.iter().sum()).collect()),
        stage(0), stage(1), stage(2), stage(3), stage(4), p95(gpu.iter().map(|sample| sample.observed_ms).collect())));
    let starts: Vec<_> = attempts.iter().map(|attempt| attempt.at).collect();
    report.push_str(&source_wait::report(
        &capture_events,
        &source_presentations,
        source.0.0 as isize,
        samples_start.unwrap(),
    ));
    if native_wait_probe {
        report.push_str(&source_wait::query_gap_work_report(
            &capture_events,
            source.0.0 as isize,
            samples_start.unwrap(),
            &attempts,
            &native_waits,
        ));
        report.push_str(&source_wait::query_gap_origin_report(
            &capture_events,
            source.0.0 as isize,
            samples_start.unwrap(),
            &attempts,
            &native_waits,
        ));
        if enumeration_probe {
            report.push_str(&source_wait::query_capture_detail_report(
                &capture_events,
                source.0.0 as isize,
                samples_start.unwrap(),
                &attempts,
            ));
        }
    }
    report.push_str(&format!(
        "source_scope_covered_fixture={covered}\nsource_scope_only_owned={only_owned}\nsource_scope_extra_arrivals={extra_arrivals}\nsource_scope_extra_wakes={extra_wakes}\n",
    ));
    if covered {
        report.push_str("source_scope_footprint=1280x720_viewport_896x336_blur_mask_with_complete_sample_inside_owned_source\n");
    }
    report.push_str(&format!("attempts={}\nattempt_fps={:.3}\nattempt_interval_p95_ms={:.3}\nsource_unchanged_attempts={}\nsource_waits={}\npresentation_waits={}\ncpu_work_p95_ms={:.3}\ncpu_capture_p95_ms={:.3}\ncpu_effects_p95_ms={:.3}\ncpu_compose_p95_ms={:.3}\ncpu_output_compare_p95_ms={:.3}\ncpu_commit_p95_ms={:.3}\n",
        attempts.len(), attempts.len() as f64/elapsed, percentile(&starts),
        attempts.iter().filter(|attempt| !attempt.stages.source_changed).count(),
        attempts.iter().filter(|attempt| matches!(attempt.wait, Some(RenderWait::Source))).count(),
        attempts.iter().filter(|attempt| matches!(attempt.wait, Some(RenderWait::Presentation))).count(),
        cpu(|attempt| attempt.work), cpu(|attempt| attempt.stages.capture), cpu(|attempt| attempt.stages.effects),
        cpu(|attempt| attempt.stages.compose), cpu(|attempt| attempt.stages.compare), cpu(|attempt| attempt.stages.commit)));
    report.push_str(&format!(
        "cpu_work_scope=sum_observed_attempt_elapsed_not_process_cpu_utilization\noutput_only_retries={}\ncpu_worker_work_ms_per_second={:.3}\ncpu_capture_work_ms_per_second={:.3}\n",
        attempts.iter().filter(|attempt| attempt.stages.output_retry).count(),
        attempts.iter().map(|attempt| attempt.work.as_secs_f64() * 1000.).sum::<f64>() / elapsed,
        attempts.iter().map(|attempt| attempt.stages.capture.as_secs_f64() * 1000.).sum::<f64>() / elapsed,
    ));
    let enumeration: Vec<_> = attempts
        .iter()
        .filter_map(|a| a.stages.enumeration)
        .collect();
    report.push_str(&format!(
        "event_fallback_forced={force_signal_fallback}\nevent_fallback_wait_calls_including_warmup={fallback_waits}\nevent_fallback_scope=forced_timer_unavailable_same_kernel_signal_millisecond_timeout_not_normal_timer_performance\n"
    ));
    report.push_str(&format!("enumeration_scope=owned_worker_gpu_capture_windows_call_cpu_elapsed_including_geometry_visibility_queries_not_wgc_or_os_sync\nenumeration_probe_enabled={}\nenumeration_samples={}\nenumeration_call_p95_ms={:.6}\nenumeration_visible_sources_max={}\nenumeration_work_ms_per_second={:.3}\n",
        enumeration_probe, enumeration.len(),
        p95(enumeration.iter().map(|(elapsed, _)| elapsed.as_secs_f64() * 1000.).collect()),
        enumeration.iter().map(|(_, count)| *count).max().unwrap_or(0),
        enumeration.iter().map(|(elapsed, _)| elapsed.as_secs_f64() * 1000.).sum::<f64>() / elapsed));
    let mut permits = PermitPolls::default();
    for attempt in &attempts {
        permits.observe(attempt.stages, attempt.wait.as_ref());
    }
    report.push_str(&format!(
        "permit_scope=actual_nonblocking_all_surface_gate_poll_outcomes_not_elapsed_or_photon\ncompose_permit_polls={}\ncompose_permit_blocked={}\ncommit_permit_polls={}\ncommit_permit_blocked={}\ncommit_permit_blocked_with_source_wait={}\n",
        permits.compose, permits.compose_blocked, permits.commit,
        permits.commit_blocked, permits.commit_blocked_source_wait,
    ));
    let submissions: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.stages.submission_cpu[0] != Duration::ZERO)
        .collect();
    let submission_p95 = |index: usize| {
        if submissions.is_empty() {
            "unavailable".to_owned()
        } else {
            format!(
                "{:.6}",
                p95(submissions
                    .iter()
                    .map(|attempt| attempt.stages.submission_cpu[index].as_secs_f64() * 1000.)
                    .collect())
            )
        }
    };
    report.push_str(&format!(
        "native_submission_cpu_scope=accepted_attempt_sum_across_surfaces_not_gpu_or_system_queue_latency\nnative_submission_cpu_samples={}\nnative_dxgi_present_cpu_p95_ms={}\nnative_composition_commit_cpu_p95_ms={}\nnative_window_placement_cpu_p95_ms={}\n",
        submissions.len(), submission_p95(0), submission_p95(1), submission_p95(2),
    ));
    commit_cpu::append(&mut report, &attempts, p95);
    report.push_str(&format!(
        "unchanged_with_pending_comparison={}\n",
        attempts
            .iter()
            .filter(|attempt| !attempt.stages.source_changed && attempt.stages.comparison_waiting)
            .count()
    ));
    // HWNDs identify sources only inside this owned fixture. Output stable ordinal
    // labels, retaining sequence ranges to distinguish polls from actual frames.
    wake_response::append(
        &mut report,
        source.0.0 as isize,
        &native_waits,
        &attempts,
        &capture_events,
    );
    use super::super::screen_capture::diagnostics::Kind;
    let mut sources: std::collections::BTreeMap<isize, Vec<_>> = Default::default();
    for event in capture_events
        .iter()
        .copied()
        .filter(|event| event.at >= samples_start.unwrap())
    {
        sources.entry(event.token).or_default().push(event);
    }
    report.push_str(&format!("capture_observation_scope=worker_dequeue_and_comparison_observation_not_arrival_or_display\ncapture_observation_dropped={capture_dropped}\n"));
    for (index, (token, events)) in sources.iter().enumerate() {
        let mut arrivals: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind, Kind::Arrived { .. }))
            .map(|event| event.at)
            .collect();
        // Concurrent callbacks can append in a different order from their entry timestamps.
        arrivals.sort_unstable();
        let wake_times: Vec<_> = events
            .iter()
            .filter_map(|event| match event.kind {
                Kind::Arrived {
                    notified,
                    woke: true,
                } => notified
                    .checked_duration_since(event.at)
                    .map(|duration| duration.as_secs_f64() * 1000.),
                _ => None,
            })
            .collect();
        report.push_str(&format!(
            "source_{index}_callback_scope=entry_to_notification_complete_before_diagnostic_lock_not_frame_or_worker_execution\nsource_{index}_callback_wakes={}\nsource_{index}_callback_to_notify_p95_ms={:.6}\n",
            wake_times.len(), p95(wake_times),
        ));
        let arrival_intervals = arrivals
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.)
            .collect();
        report.push_str(&format!("source_{index}_arrival_notifications={}\nsource_{index}_arrival_interval_p95_ms={:.3}\n", arrivals.len(), p95(arrival_intervals)));
        let frames: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind, Kind::Frame { .. }))
            .collect();
        let intervals = frames
            .windows(2)
            .map(|pair| pair[1].at.duration_since(pair[0].at).as_secs_f64() * 1000.)
            .collect();
        let timestamps: Vec<_> = frames
            .iter()
            .filter_map(|event| match event.kind {
                Kind::Frame { ticks, .. } => Some(ticks),
                _ => None,
            })
            .collect();
        let capture_intervals = timestamps
            .windows(2)
            .filter_map(|pair| {
                pair[1]
                    .checked_sub(pair[0])
                    .filter(|delta| *delta > 0)
                    .map(|delta| delta as f64 / 10_000.)
            })
            .collect();
        report.push_str(&format!(
            "source_{index}_fixture_owned={}\nsource_{index}_wgc_timestamp_interval_p95_ms={:.3}\n",
            *token == source.0.0 as isize,
            p95(capture_intervals)
        ));
        let confirmations: Vec<_> = events
            .iter()
            .filter_map(|event| match event.kind {
                Kind::Confirm { elapsed, .. } => Some(elapsed.as_secs_f64() * 1000.),
                _ => None,
            })
            .collect();
        report.push_str(&format!("source_{index}_dequeued={}\nsource_{index}_dequeue_interval_p95_ms={:.3}\nsource_{index}_comparison_confirmations={}\nsource_{index}_comparison_observed_p95_ms={:.3}\nsource_{index}_changed_confirmations={}\nsource_{index}_pending_polls={}\nsource_{index}_sequence_first={}\nsource_{index}_sequence_last={}\n",
            frames.len(), p95(intervals), confirmations.len(), p95(confirmations),
            events.iter().filter(|event| matches!(event.kind, Kind::Confirm { changed: true, .. })).count(),
            events.iter().filter(|event| matches!(event.kind, Kind::Pending)).count(),
            frames.first().map_or(0, |event| event.sequence), frames.last().map_or(0, |event| event.sequence)));
    }
    // Count the owned source independently from unrelated desktop updates.
    // Several versions in one candidate describe a mixed/retained dependency;
    // do not silently pick the newest and claim a complete source frame.
    let OwnedCommits {
        mixed,
        unmatched: unseen,
        repeated,
        times: owned_times,
        delays: mut matched,
    } = owned_commits(
        &source_presentations,
        &capture_events,
        source.0.0 as isize,
        samples_start.unwrap(),
    );
    matched.sort_by(f64::total_cmp);
    let delay = matched
        .get(((matched.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN);
    report.push_str(&format!("source_presentation_records={}\nsource_presentation_records_dropped={source_presentations_dropped}\nowned_source_distinct_committed_versions={}\nowned_source_commit_fps={:.3}\nowned_source_commit_interval_p95_ms={:.3}\nowned_source_mixed_version_commits={mixed}\nowned_source_unmatched_commits={unseen}\nowned_source_repeat_version_commits={repeated}\nowned_source_sample_policy=first_commit_for_version_dequeued_in_measurement_interval\nowned_source_dequeue_to_commit_p95_ms={delay:.3}\nowned_source_commit_scope=accepted_native_commit_not_physical_display_or_paint_id\n",
        source_presentations.len(), owned_times.len(), owned_times.len() as f64 / elapsed,
        if owned_times.len() > 1 { percentile(&owned_times) } else { f64::NAN }));
    let owned_token = source.0.0 as isize;
    let shown = super::display_timing::match_sources(&display_observations, &source_presentations);
    let mut shown_versions = HashSet::new();
    let mut shown_times = Vec::new();
    let mut mismatched_refresh = 0;
    let mut display_boundary = 0;
    let measured_versions: HashSet<_> = capture_events
        .iter()
        .filter_map(|event| {
            if event.token == owned_token
                && event.at >= samples_start.unwrap()
                && let Kind::Frame { ticks, .. } = event.kind
            {
                Some((event.sequence, ticks))
            } else {
                None
            }
        })
        .collect();
    let mut displayed = Vec::new();
    for (observation, sources) in &shown {
        if observation.present_refresh != observation.sync_refresh {
            mismatched_refresh += 1;
        }
        let mut owned = sources.iter().filter(|source| source.token == owned_token);
        if let Some(version) = owned.next()
            && owned.next().is_none()
        {
            if !measured_versions.contains(&(version.sequence, version.ticks)) {
                display_boundary += 1;
                continue;
            }
            if shown_versions.insert((version.sequence, version.ticks)) {
                shown_times.push(observation.at);
                displayed.push(*observation);
            }
        }
    }
    report.push_str(&format!("display_observation_scope=os_reported_counter_not_photon_or_paint_id\ndisplay_observation_records={}\ndisplay_observation_dropped={display_dropped}\ndisplay_observation_errors={display_errors}\ndisplay_matched_native_commits={}\ndisplay_refresh_timestamp_mismatch={mismatched_refresh}\nowned_source_os_displayed_versions={}\nowned_source_os_displayed_fps={:.3}\nowned_source_display_observed_interval_p95_ms={:.3}\n",
        display_observations.len(), shown.len(), shown_versions.len(), shown_versions.len() as f64 / elapsed,
        if shown_times.len() > 1 { percentile(&shown_times) } else { f64::NAN }));
    let mut qpc_intervals: Vec<_> = displayed
        .windows(2)
        .filter_map(|pair| super::display_timing::interval_ms(pair[0], pair[1]))
        .collect();
    let invalid_intervals = displayed
        .len()
        .saturating_sub(1)
        .saturating_sub(qpc_intervals.len());
    qpc_intervals.sort_by(f64::total_cmp);
    let qpc_p95 = qpc_intervals
        .get(((qpc_intervals.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN);
    let frequencies: std::collections::BTreeSet<_> = displayed
        .iter()
        .map(|observation| observation.frequency)
        .collect();
    report.push_str(&format!("display_qpc_frequencies_hz={frequencies:?}\ndisplay_boundary_source_commits_excluded={display_boundary}\nowned_source_display_qpc_intervals={}\nowned_source_display_qpc_invalid_intervals={invalid_intervals}\nowned_source_display_qpc_interval_p95_ms={qpc_p95:.3}\ndisplay_qpc_scope=os_sync_timestamp_with_matching_refresh_counters_not_photon\n", qpc_intervals.len()));
    let coverage = observation_coverage::Coverage::from_observations(&displayed);
    let optional_p95 = |values: &[f64]| {
        if values.is_empty() {
            "unavailable".to_owned()
        } else {
            format!("{:.6}", p95(values.to_vec()))
        }
    };
    let consecutive_over = coverage
        .consecutive_cycles
        .iter()
        .filter(|cycles| **cycles > 3)
        .count();
    report.push_str(&format!(
        "display_coverage_scope=adjacent_observed_unique_source_versions_consecutive_native_present_counts_not_all_physical_frames\ndisplay_consecutive_count_intervals={}\ndisplay_consecutive_count_qpc_p95_ms={}\ndisplay_consecutive_count_valid_refresh_intervals={}\ndisplay_consecutive_count_over_3_refreshes={}\ndisplay_counter_gap_intervals={}\ndisplay_counter_gap_qpc_p95_ms={}\ndisplay_counter_gap_unobserved_present_counts={}\ndisplay_counter_gap_meaning=unknown_history_may_include_repeated_source_or_unobserved_display_or_discarded_present\ndisplay_coverage_invalid={}\n",
        coverage.consecutive_ms.len(), optional_p95(&coverage.consecutive_ms),
        coverage.consecutive_cycles.len(), consecutive_over,
        coverage.gap_ms.len(), optional_p95(&coverage.gap_ms), coverage.skipped_counts, coverage.invalid,
    ));
    let mut cycle_counts = std::collections::BTreeMap::<u32, usize>::new();
    let mut refresh_periods = Vec::new();
    for pair in displayed.windows(2) {
        if let Some((cycles, period)) = super::display_timing::refresh_interval(pair[0], pair[1]) {
            *cycle_counts.entry(cycles).or_default() += 1;
            refresh_periods.push(period);
        }
    }
    refresh_periods.sort_by(f64::total_cmp);
    let period_median = refresh_periods
        .get(refresh_periods.len() / 2)
        .copied()
        .unwrap_or(f64::NAN);
    let invalid_cycles = displayed
        .len()
        .saturating_sub(1)
        .saturating_sub(refresh_periods.len());
    if probe_enabled {
        let (samples, (dropped, invalid, pending, cpu)) = {
            let mut state = port.shared.0.lock().unwrap();
            (
                std::mem::take(&mut state.paint_samples),
                std::mem::take(&mut state.paint_probe_summary),
            )
        };
        let mut decoded = HashMap::new();
        for sample in &samples {
            let key = (
                sample.source.token,
                sample.source.sequence,
                sample.source.ticks,
            );
            decoded
                .entry(key)
                .and_modify(|value| {
                    if *value != Some(sample.paint) {
                        *value = None;
                    }
                })
                .or_insert(Some(sample.paint));
        }
        let frames = owned_frames(&capture_events, source.0.0 as isize, samples_start.unwrap());
        let confirmations = Confirmations::from_events(&capture_events);
        let source_gpu = SourceGpuComparisons::from_events(&capture_events);
        let mut source_gpu_samples = Vec::new();
        let mut source_gpu_missing = 0;
        let mut capture_phases = Vec::new();
        let mut cadence_frames = Vec::new();
        let mut native_commits = HashMap::new();
        let mut composition_samples = HashMap::new();
        let mut composition_pre_samples = HashMap::new();
        let mut composition_advances = HashMap::new();
        let mut composition_missing = 0;
        let mut composition_invalid = 0;
        let mut composition_invalid_examples = Vec::new();
        let mut native_commit_missing = 0;
        let mut native_commit_invalid = 0;
        let mut capture_phase_missing = 0;
        let mut stages = Vec::new();
        let mut freshness = Vec::new();
        let mut freshness_invalid = 0;
        let mut seen = HashSet::new();
        let mut lower = Vec::new();
        let mut upper = Vec::new();
        let mut missing = 0;
        let mut ambiguous = 0;
        for (observation, commit) in
            super::display_timing::match_commits(&display_observations, &source_presentations)
        {
            if commit.counts.len() != 1 {
                ambiguous += 1;
                continue;
            }
            let owned: Vec<_> = commit
                .sources
                .iter()
                .filter(|v| v.token == source.0.0 as isize)
                .collect();
            if owned.len() != 1 {
                ambiguous += 1;
                continue;
            }
            let version = owned[0];
            let Some(Some(paint)) = decoded.get(&(version.token, version.sequence, version.ticks))
            else {
                missing += 1;
                continue;
            };
            if seen.contains(paint) {
                continue;
            }
            let Some(publication) = paint_publications
                .get(paint)
                .filter(|p| p.before >= samples_start.unwrap())
            else {
                missing += 1;
                continue;
            };
            let Some(dequeued) = frames.get(&(version.sequence, version.ticks)) else {
                missing += 1;
                continue;
            };
            if let Some(sample) = paint_stages(observation, *publication, *dequeued, commit.at) {
                if composition_probe {
                    if let Some(native) = commit.native_commit
                        && let Some(clock) = native.composition
                    {
                        if let Some(sample) = clock.sample(native, observation) {
                            composition_samples.insert(*paint, sample);
                            if let Some(pre) = native.composition_before_present
                                && let Some(pre_sample) = pre.sample(native, observation)
                                && let Some(advance) = pre.estimate_advance_ms(clock)
                            {
                                composition_pre_samples.insert(*paint, pre_sample);
                                composition_advances.insert(*paint, advance);
                            }
                        } else {
                            composition_invalid += 1;
                            if composition_invalid_examples.len() < 3 {
                                composition_invalid_examples.push(format!(
                                    "stats={:?},anchor_qpc={},display_frequency={},display_qpc={},query_ms={:?},order={}",
                                    clock.stats, clock.anchor.qpc, observation.frequency, observation.qpc,
                                    clock.after.checked_duration_since(clock.before).map(|d|d.as_secs_f64()*1000.),
                                    clock.before <= clock.anchor.before && clock.anchor.before <= clock.anchor.after
                                        && clock.anchor.after <= clock.after && clock.after <= native.before && native.before <= native.after,
                                ));
                            }
                        }
                    } else {
                        composition_missing += 1;
                    }
                }
                if let Some(native) = commit.native_commit {
                    if let Some(sample) =
                        super::display_timing::native_commit_sync(observation, native, commit.at)
                    {
                        native_commits.insert(*paint, sample);
                    } else {
                        native_commit_invalid += 1;
                    }
                } else {
                    native_commit_missing += 1;
                }
                seen.insert(*paint);
                lower.push(sample.paint_sync.0);
                upper.push(sample.paint_sync.1);
                stages.push(sample);
                if let Some((_, sync)) = super::display_timing::presented_window(observation) {
                    cadence_frames.push(super::display_timing::CadenceFrame {
                        count: observation.count,
                        sync_refresh: observation.sync_refresh,
                        paint: *paint,
                        endpoints: [publication.after, *dequeued, commit.at, sync],
                    });
                }
                if let Some(spans) = source_gpu.versions.get(version).copied().flatten() {
                    source_gpu_samples.push(spans);
                } else {
                    source_gpu_missing += 1;
                }
                if let Some(phases) = confirmations.phases(*version, *dequeued, commit.at) {
                    capture_phases.push(phases);
                } else {
                    capture_phase_missing += 1;
                }
                let current =
                    super::display_timing::presented_window(observation).and_then(|sync| {
                        paint_freshness(&paint_publications, *paint, *dequeued, commit.at, sync)
                    });
                if let Some(current) = current {
                    freshness.push(current);
                } else {
                    freshness_invalid += 1;
                }
            } else {
                ambiguous += 1;
            }
        }
        let metric = |values: Vec<f64>| {
            if values.is_empty() {
                "unavailable".to_owned()
            } else {
                format!("{:.6}", p95(values))
            }
        };
        let cadence: Vec<_> = cadence_frames
            .windows(2)
            .filter_map(|pair| super::display_timing::cadence_pair(pair[0], pair[1]))
            .collect();
        observation_coverage::report_cadence(
            &mut report,
            &cadence_frames,
            period_median * 3.,
            &metric,
        );
        let long: Vec<_> = cadence
            .iter()
            .filter(|pair| pair.refresh_cycles > 3)
            .collect();
        report_cadence_sources(&mut report, &cadence, period_median * 3., &metric);
        report.push_str(&format!(
            "native_commit_scope=actual_dcomposition_call_window_to_same_pixel_version_os_sync_not_batch_pickup_deadline_or_photon\nnative_commit_missing={native_commit_missing}\nnative_commit_invalid={native_commit_invalid}\n",
        ));
        report.push_str(&format!("composition_probe_enabled={composition_probe}\ncomposition_clock_missing={composition_missing}\ncomposition_clock_invalid={composition_invalid}\n"));
        report.push_str(&format!(
            "composition_clock_invalid_examples={composition_invalid_examples:?}\n"
        ));
        let clocks: Vec<_> = composition_samples.values().copied().collect();
        report_composition_samples(&mut report, "all", &clocks, &metric);
        let long_clocks: Vec<_> = cadence
            .iter()
            .filter(|pair| {
                pair.refresh_cycles > 3
                    && pair.source_group(period_median * 3.)
                        == Some(super::display_timing::CadenceSource::ContinuousWithinBudget)
            })
            .filter_map(|pair| composition_samples.get(&pair.paint).copied())
            .collect();
        report_composition_samples(
            &mut report,
            "continuous_publication_within_budget_long",
            &long_clocks,
            &metric,
        );
        for (name, paints) in
            [
                (
                    "all",
                    composition_samples.keys().copied().collect::<Vec<_>>(),
                ),
                (
                    "continuous_publication_within_budget_long",
                    cadence
                        .iter()
                        .filter(|pair| {
                            pair.refresh_cycles > 3 && pair.source_group(period_median * 3.)
                    == Some(super::display_timing::CadenceSource::ContinuousWithinBudget)
                        })
                        .map(|pair| pair.paint)
                        .collect::<Vec<_>>(),
                ),
            ]
        {
            let pre: Vec<_> = paints
                .iter()
                .filter_map(|p| composition_pre_samples.get(p).copied())
                .collect();
            report_composition_samples(&mut report, &format!("pre_present_{name}"), &pre, &metric);
            let mut advances = std::collections::BTreeMap::new();
            for paint in &paints {
                if let Some(delta) = composition_advances.get(paint)
                    && let Some(post) = composition_samples.get(paint)
                {
                    *advances
                        .entry((delta / post.period_ms).round() as i64)
                        .or_insert(0usize) += 1;
                }
            }
            report.push_str(&format!("composition_{name}_pre_post_paired_samples={}\ncomposition_{name}_pre_post_unpaired_samples={}\ncomposition_{name}_prediction_advance_rounded_cycles_histogram={advances:?}\n",
                pre.len(), paints.len() - pre.len()));
        }
        for (position, samples) in [
            ("post_present", &composition_samples),
            ("pre_present", &composition_pre_samples),
        ] {
            report_prediction_intervals(
                &mut report,
                position,
                &cadence_frames,
                samples,
                period_median * 3.,
                &metric,
            );
        }
        report.push_str(&format!(
            "native_wait_probe_enabled={native_wait_probe}\nnative_wait_samples={}\nnative_wait_samples_dropped={native_waits_dropped}\n",
            native_waits.len()
        ));
        interval_work::report(
            &mut report,
            &cadence_frames,
            &composition_pre_samples,
            &attempts,
            &native_waits,
            period_median * 3.,
            &metric,
        );
        let all: Vec<_> = native_commits.values().copied().collect();
        report_native_commit_samples(&mut report, "all", &all, period_median, &metric);
        let tail: Vec<_> = cadence
            .iter()
            .filter(|pair| {
                pair.refresh_cycles > 3
                    && pair.source_group(period_median * 3.)
                        == Some(super::display_timing::CadenceSource::ContinuousWithinBudget)
            })
            .filter_map(|pair| native_commits.get(&pair.paint).copied())
            .collect();
        report_native_commit_samples(
            &mut report,
            "continuous_publication_within_budget_long",
            &tail,
            period_median,
            &metric,
        );
        let mut widened = [0usize; 3];
        for pair in &long {
            if let Some(index) = pair.largest_growth() {
                widened[index] += 1;
            }
        }
        report.push_str(&format!(
            "cadence_pair_scope=same_pixel_identified_adjacent_display_versions_publication_complete_dequeue_commit_os_sync_upper_not_arrival_or_photon\ncadence_pairs={}\ncadence_invalid_pairs={}\ncadence_long_over_3_refreshes_pairs={}\ncadence_long_skipped_paint_pairs={}\ncadence_long_largest_growth_publish_to_dequeue={}\ncadence_long_largest_growth_dequeue_to_commit={}\ncadence_long_largest_growth_commit_to_sync={}\n",
            cadence.len(), cadence_frames.len().saturating_sub(1).saturating_sub(cadence.len()),
            long.len(), long.iter().filter(|pair| pair.skipped_paints != 0).count(),
            widened[0], widened[1], widened[2],
        ));
        for (index, stage) in ["publication", "dequeue", "commit", "sync"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!(
                "cadence_long_{stage}_interval_p95_ms={}\n",
                metric(long.iter().map(|pair| pair.intervals_ms[index]).collect())
            ));
        }
        for (index, stage) in ["publish_to_dequeue", "dequeue_to_commit", "commit_to_sync"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!(
                "cadence_long_{stage}_interval_growth_p95_ms={}\n",
                metric(long.iter().map(|pair| pair.growth_ms[index]).collect())
            ));
        }
        report.push_str(&format!(
            "capture_completion_scope={completion_scope:?}\nsource_gpu_timing_scope=actual_owned_wgc_frozen_candidate_timestamp_spans_copy_and_compare_readback_not_shader_active_queue_absolute_time_or_photon\nsource_gpu_timing_requested={profile_capture}\nsource_gpu_timing_samples={}\nsource_gpu_timing_missing_or_ambiguous={}\nsource_gpu_timing_invalid={}\nsource_gpu_timing_skipped={}\nsource_gpu_copy_span_p95_ms={}\nsource_gpu_compare_readback_span_p95_ms={}\nsource_gpu_query_observed_p95_ms={}\n",
            source_gpu_samples.len(), source_gpu_missing, source_gpu.invalid, source_gpu.skipped,
            metric(source_gpu_samples.iter().map(|s| s[0]).collect()),
            metric(source_gpu_samples.iter().map(|s| s[1]).collect()),
            metric(source_gpu_samples.iter().map(|s| s[2]).collect()),
        ));
        report.push_str(&format!(
            "capture_phase_scope=exact_frozen_candidate_dequeue_comparison_start_confirmation_and_commit_elapsed_not_gpu_active_or_photon\ncapture_phase_samples={}\ncapture_phase_missing_or_ambiguous={}\ncapture_confirmation_invalid={}\ncapture_dequeue_to_comparison_start_p95_ms={}\ncapture_comparison_start_to_confirm_p95_ms={}\ncapture_confirm_to_commit_p95_ms={}\n",
            capture_phases.len(), capture_phase_missing, confirmations.invalid,
            metric(capture_phases.iter().map(|s| s[0]).collect()),
            metric(capture_phases.iter().map(|s| s[1]).collect()),
            metric(capture_phases.iter().map(|s| s[2]).collect()),
        ));
        let denominator = freshness.len().max(1) as f64;
        report.push_str(&format!(
            "paint_freshness_scope=completed_publication_calls_for_newer_owned_paints_at_exact_pixel_version_phase_boundaries_not_gpu_completion_wgc_arrival_or_photon\npaint_freshness_samples={}\npaint_freshness_invalid={}\npaint_freshness_dequeue_newer_p95={}\npaint_freshness_commit_newer_p95={}\npaint_freshness_sync_newer_lower_p95={}\npaint_freshness_sync_newer_upper_p95={}\npaint_freshness_dequeue_current_fraction={:.6}\npaint_freshness_commit_current_fraction={:.6}\npaint_freshness_sync_current_lower_fraction={:.6}\npaint_freshness_sync_current_upper_fraction={:.6}\n",
            freshness.len(), freshness_invalid,
            metric(freshness.iter().map(|s| s.dequeue_newer as f64).collect()),
            metric(freshness.iter().map(|s| s.commit_newer as f64).collect()),
            metric(freshness.iter().map(|s| s.sync_newer.0 as f64).collect()),
            metric(freshness.iter().map(|s| s.sync_newer.1 as f64).collect()),
            freshness.iter().filter(|s| s.dequeue_newer == 0).count() as f64 / denominator,
            freshness.iter().filter(|s| s.commit_newer == 0).count() as f64 / denominator,
            freshness.iter().filter(|s| s.sync_newer.1 == 0).count() as f64 / denominator,
            freshness.iter().filter(|s| s.sync_newer.0 == 0).count() as f64 / denominator,
        ));
        report.push_str(&format!("paint_stage_scope=same_identified_publication_dequeue_commit_and_os_sync_percentiles_not_additive\npaint_stage_samples={}\npaint_to_dequeue_lower_p95_ms={}\npaint_to_dequeue_upper_p95_ms={}\npaint_stage_dequeue_to_commit_p95_ms={}\npaint_stage_commit_to_sync_lower_p95_ms={}\npaint_stage_commit_to_sync_upper_p95_ms={}\n",
            stages.len(), metric(stages.iter().map(|s| s.paint_dequeue.0).collect()), metric(stages.iter().map(|s| s.paint_dequeue.1).collect()),
            metric(stages.iter().map(|s| s.dequeue_commit).collect()), metric(stages.iter().map(|s| s.commit_sync.0).collect()), metric(stages.iter().map(|s| s.commit_sync.1).collect())));
        report.push_str(&format!("paint_probe_scope=two_identity_pixels_nonblocking_instrumented_worker_not_full_frame_input_or_photon\npaint_probe_decoded={}\npaint_probe_dropped={dropped}\npaint_probe_invalid={invalid}\npaint_probe_pending={pending}\npaint_sync_samples={}\npaint_sync_missing={missing}\npaint_sync_ambiguous={ambiguous}\npaint_publication_to_sync_lower_p95_ms={}\npaint_publication_to_sync_upper_p95_ms={}\npaint_probe_poll_cpu_p95_ms={}\n",
            samples.len(), lower.len(), metric(lower), metric(upper), metric(cpu.into_iter().map(|t| t.as_secs_f64() * 1000.).collect())));
    }
    let latency = owned_latencies(
        &display_observations,
        &source_presentations,
        &capture_events,
        source.0.0 as isize,
        samples_start.unwrap(),
    );
    let latency_p95 = |values: Vec<f64>| {
        if values.is_empty() {
            "unavailable".to_owned()
        } else {
            format!("{:.6}", p95(values))
        }
    };
    report.push_str(&format!("capture_clock_example_fields=sync_qpc,frequency,wgc_100ns,converted_sync_ms,dequeue_upper_ms,dequeue_qpc\ncapture_clock_invalid_examples={:?}\n", latency.capture_clock_examples));
    report.push_str(&format!(
        "capture_clock_scope=reported_wgc_compositor_timestamp_to_matching_os_sync_not_paint_input_or_photon\ncapture_clock_samples={}\ncapture_clock_invalid={}\nowned_source_reported_capture_to_sync_p95_ms={}\nowned_source_reported_capture_to_dequeue_lower_p95_ms={}\nowned_source_reported_capture_to_dequeue_upper_p95_ms={}\n",
        latency.capture_sync.len(), latency.capture_invalid,
        latency_p95(latency.capture_sync), latency_p95(latency.capture_dequeue_lower), latency_p95(latency.capture_dequeue_upper),
    ));
    report.push_str(&format!(
        "source_sync_latency_scope=first_valid_single_surface_frozen_version_sync_bounds_not_input_or_photon\nsource_sync_latency_status={}\nsource_sync_latency_samples={}\nsource_sync_latency_multi_surface={}\nsource_sync_latency_invalid={}\nsource_sync_latency_boundary={}\nsource_sync_latency_mixed={}\nsource_sync_latency_repeated={}\nowned_source_dequeue_to_sync_lower_p95_ms={}\nowned_source_dequeue_to_sync_upper_p95_ms={}\nowned_source_submit_to_sync_lower_p95_ms={}\nowned_source_submit_to_sync_upper_p95_ms={}\nowned_source_sync_observation_lag_lower_p95_ms={}\nowned_source_sync_observation_lag_upper_p95_ms={}\nsource_sync_clock_call_span_p95_ms={}\n",
        if latency.dequeue_lower.is_empty() { "unavailable" } else { "available" }, latency.dequeue_lower.len(), latency.multi_surface, latency.invalid, latency.boundary, latency.mixed, latency.repeated,
        latency_p95(latency.dequeue_lower), latency_p95(latency.dequeue_upper), latency_p95(latency.submit_lower), latency_p95(latency.submit_upper),
        latency_p95(latency.observation_lower), latency_p95(latency.observation_upper), latency_p95(latency.clock_spans),
    ));
    report.push_str(&format!("owned_source_display_refresh_cycle_histogram={cycle_counts:?}\ndisplay_refresh_period_inferred_median_ms={period_median:.3}\ndisplay_refresh_cycle_invalid_intervals={invalid_cycles}\ndisplay_refresh_period_scope=inferred_from_os_qpc_and_counters_not_configured_monitor_mode\n"));
    let valid_cycles: usize = cycle_counts.values().sum();
    let over_three: usize = cycle_counts.range(4..).map(|(_, count)| count).sum();
    let cycle_p95 = if valid_cycles == 0 {
        "unavailable".to_owned()
    } else {
        let rank = (valid_cycles as f64 * 0.95).ceil() as usize;
        let mut cumulative = 0;
        cycle_counts
            .iter()
            .find_map(|(cycles, count)| {
                cumulative += count;
                (cumulative >= rank).then(|| cycles.to_string())
            })
            .unwrap()
    };
    let fraction = if valid_cycles == 0 {
        "unavailable".to_owned()
    } else {
        format!("{:.6}", over_three as f64 / valid_cycles as f64)
    };
    report.push_str(&format!(
        "refresh_cadence_scope=validated_os_refresh_counter_deltas_not_rounded_milliseconds_or_photon\nrefresh_cadence_preference=maximize_intervals_within_3_refreshes_not_universal_hardware_guarantee\nrefresh_cadence_valid_intervals={valid_cycles}\nrefresh_cadence_over_3_refreshes={over_three}\nrefresh_cadence_over_3_fraction={fraction}\nrefresh_cadence_p95_cycles={cycle_p95}\nrefresh_cadence_3_refreshes_inferred_ms={:.6}\n",
        period_median * 3.,
    ));
    if let Some(reference) = super::display_timing::ideal_periodic_intervals(
        1000. * elapsed / source_times.len() as f64,
        period_median,
    ) {
        let reference_p95 = p95(reference.clone());
        let over_target = reference.iter().filter(|ms| **ms > 20.).count();
        let observed_over = qpc_intervals.iter().filter(|ms| **ms > 20.).count();
        let beyond_reference = qpc_intervals
            .iter()
            .filter(|ms| **ms > reference_p95 + 0.1)
            .count();
        report.push_str(&format!(
            "refresh_reference_scope=periodic_source_first_eligible_refresh_no_processing_delay_not_hardware_lower_bound_or_gate\nrefresh_reference_p95_ms={reference_p95:.3}\nrefresh_reference_over_20ms_fraction={:.6}\nobserved_over_20ms_fraction={:.6}\nobserved_beyond_reference_p95_fraction={:.6}\nrefresh_reference_20ms_role=historical_reference_not_acceptance_gate\n",
            over_target as f64 / reference.len() as f64,
            observed_over as f64 / qpc_intervals.len().max(1) as f64,
            beyond_reference as f64 / qpc_intervals.len().max(1) as f64,
        ));
    }
    eprintln!("{report}");
    std::fs::write(
        format!("../../target/annotation-performance-{label}.txt"),
        report,
    )
    .unwrap();
    port.clear();
    drop(port);
}

/// Probe one point well inside each uniform band at three heights. This proves
/// these source/output samples, not every pixel or physical display latency.
fn band_samples(pixels: &[u8], width: u32, height: u32) -> Vec<[u8; 4]> {
    assert_eq!((width, height), (1280, 720));
    assert_eq!(pixels.len(), width as usize * height as usize * 4);
    [80, 360, 640]
        .into_iter()
        .flat_map(|y| {
            (0..8).map(move |band| {
                let offset = (y * width as usize + band * 160 + 80) * 4;
                pixels[offset..offset + 4].try_into().unwrap()
            })
        })
        .collect()
}

fn expected_band_samples(tick: u32) -> Vec<[u8; 4]> {
    (0..24)
        .map(|sample| {
            let rgb = band_color(tick, sample % 8).0;
            // Both WGC and the composition swap chain store BGRA bytes.
            [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
        })
        .collect()
}

fn sample_error(actual: &[[u8; 4]], expected: &[[u8; 4]]) -> u8 {
    assert_eq!(actual.len(), expected.len());
    actual
        .iter()
        .zip(expected)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)))
        .max()
        .unwrap()
}

#[test]
#[ignore = "owned WGC source/output pixel verification; blocking readback, not a benchmark"]
fn gpu_owned_source_pixels_match_frozen_commit_versions() {
    if cfg!(debug_assertions) {
        panic!("run pixel verification with --release");
    }
    let mut renderer = Renderer::new().unwrap();
    let verification = Graphics::new(
        renderer.graphics.device.clone(),
        renderer.graphics.context.clone(),
    )
    .unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    // STATIC's initial WM_PAINT must finish before our first GDI publication.
    surface::pump_messages();
    let painter = SourceFrame::new(source.0);
    let mut document = scene(bounds, input.0, 0);
    let token = source.0.0 as isize;
    let request = ScreenCaptureRequest {
        revision: document.stamp.revision,
        regions: vec![bounds],
        paused_regions: Vec::new(),
        excluded_windows: vec![WindowToken(input.0.0 as isize)],
    };
    let mut decoded = HashMap::new();
    let mut captured_ticks = HashSet::new();
    let mut committed_ticks = HashSet::new();
    let mut maximum_error = 0;
    let mut rejected_candidates = 0;
    let mut paint_commits = HashMap::new();
    let mut paint_observations = Vec::new();
    for effect in [MosaicEffect::Blur, MosaicEffect::Pixelate] {
        let mut layers = document.layers.to_vec();
        let RenderLayer::Mosaic(layer) = &mut layers[0] else {
            unreachable!()
        };
        let mut object = (*layer.object).clone();
        let Extra::Mosaic(value) = &mut object.extra else {
            unreachable!()
        };
        value.effect = effect;
        value.strength = if effect == MosaicEffect::Blur {
            100
        } else {
            50
        };
        layer.object = Arc::new(object);
        document.layers = layers.into();
        document.stamp.revision += 1;
        let mut paints = HashMap::new();
        let mut rejected = false;
        let mut saw_newer_after_rejection = false;
        for tick in [1, 37, 109, 211] {
            // Reuse the single 109 publication injected during rejection. A
            // repaint of identical pixels would make its paint identity ambiguous.
            if !(tick == 109 && rejected) {
                assert!(paints.insert(tick, painter.paint(tick)).is_none());
                painter.verify(tick);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                surface::pump_messages();
                if let Some(surface) = renderer.surfaces.first()
                    && let Ok(observation) = surface.display_observation()
                {
                    paint_observations.push(observation);
                }
                if let Some(frame) = renderer.capture.sample_gpu(&request).unwrap() {
                    let owned = frame
                        .layers
                        .iter()
                        .find(|layer| layer.source.is_some_and(|version| version.token == token))
                        .expect("owned source missing from complete WGC capture");
                    let version = owned.source.unwrap();
                    let texture = graphics::Texture::wrap(
                        &renderer.graphics.device,
                        owned.texture.clone(),
                        false,
                        false,
                    )
                    .unwrap();
                    let samples = band_samples(
                        &graphics::tests::readback(&renderer.graphics, &texture),
                        texture.width,
                        texture.height,
                    );
                    let painted = [1, 37, 109, 211]
                        .into_iter()
                        .find(|&painted| {
                            sample_error(&samples, &expected_band_samples(painted)) <= 1
                        })
                        .unwrap_or_else(|| panic!(
                            "source bands mixed versions or differ from owned paint: {samples:?}; bounds={:?}; version={version:?}", owned.bounds,
                        ));
                    captured_ticks.insert(painted);
                    if rejected && painted == 109 {
                        saw_newer_after_rejection = true;
                    }
                    let key = (version.token, version.sequence, version.ticks);
                    if let Some(previous) = decoded.insert(key, painted) {
                        assert_eq!(previous, painted, "same WGC version changed pixels");
                    }
                    renderer.sampled = Some(frame);
                    let mut output = None;
                    let result = renderer
                        .render(&document, |surfaces| {
                            output = Some(band_samples(
                                &graphics::tests::readback(&verification, &surfaces[0].target),
                                surfaces[0].target.width,
                                surfaces[0].target.height,
                            ));
                            if tick == 37
                                && !rejected
                                && sample_error(
                                    output.as_ref().unwrap(),
                                    &expected_band_samples(37),
                                ) <= 1
                            {
                                // Keep the frozen 37 candidate while the real WGC source
                                // advances. Its eventual metadata must still describe 37.
                                rejected = true;
                                rejected_candidates += 1;
                                assert!(paints.insert(109, painter.paint(109)).is_none());
                                painter.verify(109);
                                return Ok(false);
                            }
                            if tick == 37 && rejected && !saw_newer_after_rejection {
                                return Ok(false);
                            }
                            present_surfaces(surfaces)
                        })
                        .unwrap();
                    if let Some(versions) = &renderer.last_commit_sources {
                        assert!(result.presented);
                        let owned: Vec<_> = versions.iter().filter(|v| v.token == token).collect();
                        assert_eq!(owned.len(), 1, "candidate mixed owned source versions");
                        let version = owned[0];
                        let painted = decoded[&(version.token, version.sequence, version.ticks)];
                        let error = sample_error(&output.unwrap(), &expected_band_samples(painted));
                        maximum_error = maximum_error.max(error);
                        assert!(
                            error <= 1,
                            "commit pixels disagree with frozen source version: {error}"
                        );
                        committed_ticks.insert(painted);
                        if painted == tick {
                            assert_eq!(renderer.last_commit_counts.len(), 1);
                            let count = renderer.last_commit_counts[0];
                            assert!(
                                paint_commits
                                    .insert(count, (effect, painted, paints[&painted]))
                                    .is_none()
                            );
                            break;
                        }
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "paint {tick} was not committed for {effect:?}"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(saw_newer_after_rejection);
    }
    // A sparse first presentation can report zero/disjoint statistics. Collect
    // later acknowledgements without stalling source progress or inventing one.
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        surface::pump_messages();
        if let Ok(observation) = renderer.surfaces[0].display_observation() {
            paint_observations.push(observation);
        }
        thread::sleep(Duration::from_millis(1));
    }
    let mut seen = HashSet::new();
    let mut sync_samples = Vec::new();
    for observation in &paint_observations {
        if let Some((effect, painted, publication)) = paint_commits.get(&observation.count)
            && !seen.contains(&observation.count)
            && let Some(bounds) = paint_to_sync_bounds(observation, *publication)
        {
            seen.insert(observation.count);
            sync_samples.push((*effect, *painted, bounds));
        }
    }
    eprintln!(
        "paint_sync_scope=verified_owned_paint_bitblt_flush_to_matching_os_sync_instrumented_blocking_readback_not_worker_throughput_input_or_photon status={} commits={} matched={} samples={sync_samples:?}",
        if sync_samples.is_empty() {
            "unavailable"
        } else {
            "available"
        },
        paint_commits.len(),
        sync_samples.len()
    );
    assert_eq!(rejected_candidates, 2);
    assert_eq!(captured_ticks, HashSet::from([1, 37, 109, 211]));
    assert_eq!(committed_ticks, captured_ticks);
    println!(
        "owned_paint_versions_verified=4 effects=2 rejected_candidates={rejected_candidates} band_samples_per_frame=24 max_channel_error={maximum_error}"
    );
    println!(
        "pixel_verification_scope=owned_WGC_texture_and_submitted_candidate_samples_not_full_frame_or_photon_time"
    );
    // Source/input windows were created after the renderer. Retire WGC and
    // presentation while their HWND owners still exist, rather than relying on
    // reverse local-drop order across capture's asynchronous producers.
    renderer.capture.stop_capture();
    drop(verification);
    drop(renderer);
}

#[test]
#[ignore = "owned static source scene wake measurement; requires Release and WGC"]
fn gpu_static_source_scene_request_to_attempt() {
    if cfg!(debug_assertions) {
        panic!("run with --release");
    }
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    let source_frame = SourceFrame::new(source.0);
    source_frame.paint(37);
    source_frame.verify(37);
    drop(bootstrap);
    let port = WindowsAnnotationRenderPort::new().unwrap();
    let initial = scene(bounds, input.0, 0);
    port.submit(Arc::new(initial.clone())).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        surface::pump_messages();
        if port.latest().unwrap().is_some_and(|frame| frame.presented) {
            break;
        }
        assert!(Instant::now() < deadline, "static source did not present");
        thread::sleep(Duration::from_millis(1));
    }
    port.shared.0.lock().unwrap().record_presentations = true;
    let mut delays = Vec::new();
    for revision in 2..=201 {
        // Vary the phase without producing fresh source pixels. This measures
        // scene scheduling, not capture freshness, real input or photon time.
        thread::sleep(Duration::from_millis(1 + revision % 15));
        let mut updated = initial.clone();
        updated.stamp.revision = revision;
        let stamp = updated.stamp;
        let requested = Instant::now();
        port.submit(Arc::new(updated)).unwrap();
        let deadline = requested + Duration::from_secs(2);
        loop {
            surface::pump_messages();
            port.latest().unwrap();
            let attempt = port
                .shared
                .0
                .lock()
                .unwrap()
                .attempts
                .iter()
                .find(|attempt| attempt.stamp == stamp)
                .map(|attempt| attempt.at);
            if let Some(at) = attempt {
                delays.push(at.duration_since(requested).as_secs_f64() * 1000.);
                break;
            }
            assert!(Instant::now() < deadline, "scene attempt timed out");
            thread::sleep(Duration::from_millis(1));
        }
    }
    delays.sort_by(f64::total_cmp);
    println!(
        "scene_request_attempt_samples={} p50_ms={:.4} p95_ms={:.4} max_ms={:.4}",
        delays.len(),
        delays[delays.len() / 2],
        delays[delays.len() * 95 / 100],
        delays.last().unwrap()
    );
    println!(
        "scope=owned_static_source_request_to_first_worker_attempt_not_input_commit_sync_or_photon"
    );
    assert_eq!(delays.len(), 200);
}

#[test]
#[ignore = "35-second instrumented owned source latency; requires Release and WGC"]
fn gpu_live_capture_paint_latency_large_blur() {
    run_case_with_probe(
        0,
        "paint-latency-large-blur",
        true,
        true,
        CompletionScope::All,
    );
}

#[test]
#[ignore = "35-second owned desktop fixture; requires Release and interactive D3D11/WGC"]
fn gpu_live_capture_performance_large_blur() {
    run_case(0, "large-blur");
}
#[test]
#[ignore = "35-second owned desktop fixture; requires Release and interactive D3D11/WGC"]
fn gpu_live_capture_performance_long_brush() {
    run_case(1, "long-brush");
}
#[test]
#[ignore = "35-second owned desktop fixture; requires Release and interactive D3D11/WGC"]
fn gpu_live_capture_performance_four_overlaps() {
    run_case(2, "four-overlaps");
}

#[test]
#[ignore = "owned desktop composition pixel probe; requires an unobstructed native desktop"]
fn gpu_surface_updates_owned_desktop_pixels_on_successive_presents() {
    let capture = Backend::new().unwrap();
    let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 168,
        bottom: 168,
    };
    let underlying = OtherThreadWindow::new(bounds);
    assert_eq!(
        unsafe { WindowFromPoint(windows::Win32::Foundation::POINT { x: 104, y: 104 }) },
        underlying.hwnd,
        "underlying fixture must accept input before composition starts"
    );
    let input = window(bounds, true);
    let surface = surface::Surface::new(
        &graphics,
        RenderSurface {
            bounds,
            scale: 1.,
            input_window: WindowToken(input.0.0 as isize),
        },
    )
    .unwrap();
    surface.verify_device_and_queue(&graphics).unwrap();
    assert_eq!(capture.device().as_raw(), graphics.device.as_raw());
    assert_eq!(capture.context().as_raw(), graphics.context.as_raw());
    eprintln!(
        "presentation_topology=same_device_capture_context_backbuffer_swapchain\nmaximum_frame_latency=1\nswap_buffers=2\nsurface_message_thread=current_owned_fixture_thread\nqueue_scope=native_configuration_not_dwm_queue_depth_or_photon_latency"
    );
    for red in [7u8, 63, 127, 211] {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !surface.ready().unwrap() {
            assert!(Instant::now() < deadline, "surface readiness timed out");
            surface::pump_messages();
            thread::sleep(Duration::from_millis(1));
        }
        let texture = Texture::create(
            &graphics.device,
            128,
            128,
            Some(&[red, 0, 128, 255].repeat(128 * 128)),
        )
        .unwrap();
        graphics.blit(
            &texture,
            &surface.target,
            bounds,
            graphics::Blit {
                source_rect: Rect {
                    left: 0,
                    top: 0,
                    right: 128,
                    bottom: 128,
                },
                destination: bounds,
                opacity: 1.,
                blend: false,
            },
        );
        assert!(surface.present().unwrap());
        // Read the owned surface on the composed desktop, not its GPU back buffer.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            surface::pump_messages();
            let actual = unsafe {
                let dc = GetDC(None);
                assert!(!dc.is_invalid());
                let color = GetPixel(dc, 104, 104);
                ReleaseDC(None, dc);
                [color.0 as u8, (color.0 >> 8) as u8, (color.0 >> 16) as u8]
            };
            if actual == [red, 0, 128] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "desktop sample {actual:?} did not reach expected version {:?}",
                [red, 0, 128]
            );
            thread::sleep(Duration::from_millis(1));
        }
        // Opaque composition pixels must remain transparent to input after
        // every repaint, without hiding the visual.
        for (x, y) in [(104, 104), (80, 80)] {
            assert_eq!(
                unsafe { WindowFromPoint(windows::Win32::Foundation::POINT { x, y }) },
                underlying.hwnd,
                "GPU output intercepted input intended for another thread at {x},{y}"
            );
        }
    }
    eprintln!(
        "scope=owned_composed_desktop_sample_not_photon_or_latency versions=4 pixels_per_version=1 exact_match=true"
    );
}

#[test]
#[ignore = "owned native composition swap-chain statistics probe"]
fn gpu_composition_display_statistics_probe() {
    let capture = Backend::new().unwrap();
    let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 168,
        bottom: 168,
    };
    let input = window(bounds, true);
    let surface = surface::Surface::new(
        &graphics,
        RenderSurface {
            bounds,
            scale: 1.,
            input_window: WindowToken(input.0.0 as isize),
        },
    )
    .unwrap();
    let mut report =
        String::from("scope=owned_composition_statistics_capability_not_photon_latency\n");
    for tick in 0..30 {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !surface.ready().unwrap() {
            assert!(Instant::now() < deadline, "surface readiness timed out");
            surface::pump_messages();
            thread::sleep(Duration::from_millis(1));
        }
        let texture = Texture::create(
            &graphics.device,
            128,
            128,
            Some(&[tick * 7, 0, 128, 255].repeat(128 * 128)),
        )
        .unwrap();
        graphics.blit(
            &texture,
            &surface.target,
            bounds,
            graphics::Blit {
                source_rect: Rect {
                    left: 0,
                    top: 0,
                    right: 128,
                    bottom: 128,
                },
                destination: bounds,
                opacity: 1.,
                blend: false,
            },
        );
        assert!(surface.present().unwrap());
        thread::sleep(Duration::from_millis(20));
        surface::pump_messages();
        report.push_str(&format!("{tick}:{}\n", surface.statistics_probe()));
    }
    eprintln!("{report}");
    std::fs::write(
        "../../target/annotation-display-statistics-probe.txt",
        report,
    )
    .unwrap();
}

#[test]
fn owned_source_statistics_exclude_warmup_repeats_and_mixed_versions() {
    use super::super::screen_capture::diagnostics::{Event, Kind, Published};
    let start = Instant::now();
    let version = |sequence| Published {
        token: 7,
        sequence,
        ticks: sequence as i64 * 100,
    };
    let events = [
        Event {
            token: 7,
            sequence: 1,
            at: start - Duration::from_millis(1),
            kind: Kind::Frame {
                ticks: 100,
                dequeue_qpc: None,
            },
        },
        Event {
            token: 7,
            sequence: 2,
            at: start + Duration::from_millis(1),
            kind: Kind::Frame {
                ticks: 200,
                dequeue_qpc: None,
            },
        },
    ];
    let presentations = [
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            counts: Vec::new(),
            at: start + Duration::from_millis(2),
            sources: vec![version(1)],
        },
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            counts: Vec::new(),
            at: start + Duration::from_millis(5),
            sources: vec![version(2)],
        },
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            counts: Vec::new(),
            at: start + Duration::from_millis(9),
            sources: vec![version(2)],
        },
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            counts: Vec::new(),
            at: start + Duration::from_millis(11),
            sources: vec![version(2), version(3)],
        },
    ];
    let result = owned_commits(&presentations, &events, 7, start);
    assert_eq!((result.unmatched, result.repeated, result.mixed), (1, 1, 1));
    assert_eq!(result.times, vec![presentations[1].at]);
    assert_eq!(result.delays, vec![4.]);
}

#[test]
fn source_sync_latency_joins_exact_versions_and_rejects_other_surfaces() {
    use super::super::screen_capture::diagnostics::{Event, Kind, Published};
    use super::display_timing::{ClockAnchor, Count, Observation};
    let at = Instant::now();
    let count = Count {
        chain: 1,
        present: 1,
    };
    let frame = Event {
        token: 7,
        sequence: 1,
        at: at + Duration::from_millis(3),
        kind: Kind::Frame {
            ticks: 9_810_000,
            dequeue_qpc: None,
        },
    };
    let mut commit = Presentation {
        stamp: lexift_core::ports::annotation_render::RenderStamp {
            session: 1,
            revision: 1,
        },
        native_commit: None,
        at: at + Duration::from_millis(7),
        counts: vec![count],
        sources: vec![Published {
            token: 7,
            sequence: 1,
            ticks: 9_810_000,
        }],
    };
    let observation = Observation {
        frequency: 1000,
        count,
        at: at + Duration::from_millis(25),
        qpc: 995,
        present_refresh: 10,
        sync_refresh: 10,
        clock: Some(ClockAnchor {
            before: at + Duration::from_millis(20),
            after: at + Duration::from_millis(21),
            qpc: 1000,
        }),
    };
    let measure = |commit: &Presentation| {
        owned_latencies(
            std::slice::from_ref(&observation),
            std::slice::from_ref(commit),
            &[frame],
            7,
            at,
        )
    };
    let result = measure(&commit);
    assert_eq!(result.capture_sync, vec![14.]);
    assert_eq!(result.capture_dequeue_lower, vec![1.]);
    assert_eq!(result.capture_dequeue_upper, vec![2.]);
    assert_eq!(result.capture_invalid, 0);
    assert_eq!(result.dequeue_lower, vec![12.]);
    assert_eq!(result.dequeue_upper, vec![13.]);
    assert_eq!(result.submit_lower, vec![8.]);
    assert_eq!(result.submit_upper, vec![9.]);
    assert_eq!(result.observation_lower, vec![9.]);
    assert_eq!(result.observation_upper, vec![10.]);
    assert_eq!(result.clock_spans, vec![1.]);
    let mut future_frame = frame;
    future_frame.kind = Kind::Frame {
        ticks: 9_930_000,
        dequeue_qpc: None,
    };
    let future_commit = Presentation {
        stamp: lexift_core::ports::annotation_render::RenderStamp {
            session: 1,
            revision: 1,
        },
        native_commit: None,
        at: commit.at,
        counts: commit.counts.clone(),
        sources: vec![Published {
            token: 7,
            sequence: 1,
            ticks: 9_930_000,
        }],
    };
    let rejected = owned_latencies(
        std::slice::from_ref(&observation),
        std::slice::from_ref(&future_commit),
        &[future_frame],
        7,
        at,
    );
    assert_eq!(rejected.capture_invalid, 1);
    assert!(rejected.capture_sync.is_empty());
    assert_eq!(
        rejected.dequeue_lower,
        vec![12.],
        "bad WGC clock does not discard valid native provenance"
    );
    commit.sources[0].ticks = 9_810_001;
    assert_eq!(measure(&commit).boundary, 1);
    commit.sources[0].ticks = 9_810_000;
    commit.sources.push(commit.sources[0]);
    assert_eq!(measure(&commit).mixed, 1);
    commit.sources.pop();
    commit.counts.push(Count {
        chain: 2,
        present: 1,
    });
    assert_eq!(measure(&commit).multi_surface, 1);
    commit.counts.pop();
    commit.at = at + Duration::from_millis(17);
    assert_eq!(measure(&commit).invalid, 1);
}

#[test]
fn verified_publication_bounds_require_a_valid_matching_sync_window() {
    use super::display_timing::{ClockAnchor, Count, Observation};
    let at = Instant::now();
    let mut observation = Observation {
        frequency: 1000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: at + Duration::from_millis(25),
        qpc: 995,
        present_refresh: 1,
        sync_refresh: 1,
        clock: Some(ClockAnchor {
            before: at + Duration::from_millis(20),
            after: at + Duration::from_millis(21),
            qpc: 1000,
        }),
    };
    let mut paint = PaintPublication {
        before: at + Duration::from_millis(1),
        after: at + Duration::from_millis(3),
    };
    assert_eq!(paint_to_sync_bounds(&observation, paint), Some((12., 15.)));
    let dequeued = at + Duration::from_millis(5);
    let committed = at + Duration::from_millis(9);
    let stages = paint_stages(&observation, paint, dequeued, committed).unwrap();
    assert_eq!(stages.paint_dequeue, (2., 4.));
    assert_eq!(stages.dequeue_commit, 4.);
    assert_eq!(stages.commit_sync, (6., 7.));
    assert_eq!(stages.paint_sync, (12., 15.));
    assert!(paint_stages(&observation, paint, committed, dequeued).is_none());
    assert!(paint_stages(&observation, paint, at, committed).is_none());
    paint.after = at + Duration::from_millis(17);
    assert!(paint_to_sync_bounds(&observation, paint).is_none());
    paint.after = at;
    assert!(paint_to_sync_bounds(&observation, paint).is_none());
    paint.after = at + Duration::from_millis(3);
    observation.sync_refresh = 2;
    assert!(paint_to_sync_bounds(&observation, paint).is_none());
}

#[test]
fn paint_freshness_uses_fully_published_newer_versions_and_sync_bounds() {
    let now = Instant::now();
    let at = |ms| now + Duration::from_millis(ms);
    let mut publications = HashMap::from([
        (
            7,
            PaintPublication {
                before: at(0),
                after: at(1),
            },
        ),
        (
            8,
            PaintPublication {
                before: at(4),
                after: at(5),
            },
        ),
        (
            9,
            PaintPublication {
                before: at(14),
                after: at(15),
            },
        ),
        (
            10,
            PaintPublication {
                before: at(21),
                after: at(25),
            },
        ),
        (
            6,
            PaintPublication {
                before: at(8),
                after: at(9),
            },
        ),
    ]);
    assert_eq!(
        paint_freshness(&publications, 7, at(10), at(20), (at(22), at(30))),
        Some(PaintFreshness {
            dequeue_newer: 1,
            commit_newer: 2,
            sync_newer: (2, 3),
        })
    );
    assert_eq!(
        paint_freshness(&publications, 9, at(15), at(20), (at(22), at(30))),
        Some(PaintFreshness {
            dequeue_newer: 0,
            commit_newer: 0,
            sync_newer: (0, 1),
        })
    );
    assert!(paint_freshness(&publications, 11, at(30), at(30), (at(30), at(30))).is_none());
    assert!(paint_freshness(&publications, 9, at(10), at(20), (at(22), at(30))).is_none());
    assert!(paint_freshness(&publications, 7, at(10), at(5), (at(22), at(30))).is_none());
    assert!(paint_freshness(&publications, 7, at(10), at(20), (at(19), at(30))).is_none());
    assert!(paint_freshness(&publications, 7, at(10), at(20), (at(30), at(22))).is_none());
    publications.insert(
        10,
        PaintPublication {
            before: at(25),
            after: at(21),
        },
    );
    assert!(paint_freshness(&publications, 7, at(10), at(20), (at(22), at(30))).is_none());
}

#[test]
fn candidate_confirmation_phases_reject_duplicate_missing_and_cross_source_events() {
    use super::super::screen_capture::diagnostics::{Event, Kind};
    let now = Instant::now();
    let at = |ms| now + Duration::from_millis(ms);
    let version = CaptureVersion {
        token: 7,
        sequence: 2,
        ticks: 100,
    };
    let event = Event {
        token: 7,
        sequence: 999,
        at: at(6),
        kind: Kind::Confirm {
            started: at(2),
            elapsed: Duration::from_millis(4),
            candidate: Some(version),
            changed: true,
        },
    };
    let index = Confirmations::from_events(&[event]);
    assert_eq!(index.phases(version, at(1), at(10)), Some([1., 4., 4.]));
    assert_eq!(index.invalid, 0);
    assert!(
        index
            .phases(
                CaptureVersion {
                    sequence: 999,
                    ..version
                },
                at(1),
                at(10)
            )
            .is_none()
    );
    assert!(
        index
            .phases(
                CaptureVersion {
                    token: 8,
                    ..version
                },
                at(1),
                at(10)
            )
            .is_none()
    );
    assert!(
        index
            .phases(
                CaptureVersion {
                    ticks: 101,
                    ..version
                },
                at(1),
                at(10)
            )
            .is_none()
    );
    assert!(index.phases(version, at(3), at(10)).is_none());
    assert!(index.phases(version, at(1), at(5)).is_none());
    let duplicate = Confirmations::from_events(&[event, event]);
    assert!(duplicate.phases(version, at(1), at(10)).is_none());
    let wrong_source = Confirmations::from_events(&[Event { token: 8, ..event }]);
    assert_eq!(wrong_source.invalid, 1);
    assert!(wrong_source.phases(version, at(1), at(10)).is_none());
    let reversed = Confirmations::from_events(&[Event { at: at(1), ..event }]);
    assert_eq!(reversed.invalid, 1);
}

#[test]
#[ignore = "isolated D3D11 comparison timestamp experiment; run in Release without other builds"]
fn gpu_comparison_thread_group_costs() {
    use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};
    fn p95(mut samples: Vec<f64>) -> f64 {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() * 95 / 100]
    }
    use super::super::d3d_compare::{Comparison, Kernel, Outcome};
    let backend = super::super::screen_capture::capture::Backend::new().unwrap();
    let device = backend.device();
    let context = backend.context();
    let width = 2560u32;
    let height = 1440u32;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let texture = |pixels: &[u8]| {
        let mut texture = None;
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            ..Default::default()
        };
        unsafe {
            device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
                .unwrap();
        }
        texture.unwrap()
    };
    let pixels = [40, 60, 80, 255].repeat((width * height) as usize);
    let baseline = texture(&pixels);
    let kernels = [
        Kernel::test_group(device, 8).unwrap(),
        Kernel::test_group(device, 16).unwrap(),
    ];
    for case in 0..3 {
        let mut next = pixels.clone();
        let expected = match case {
            0 => Outcome::Equal,
            1 => {
                *next.last_mut().unwrap() -= 1;
                Outcome::Changed(Rect {
                    left: width as i32 - 1,
                    top: height as i32 - 1,
                    right: width as i32,
                    bottom: height as i32,
                })
            }
            _ => {
                for pixel in next.as_chunks_mut::<4>().0 {
                    pixel[0] += 1;
                }
                Outcome::Changed(Rect {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                })
            }
        };
        let candidate = texture(&next);
        let mut comparisons = [
            Comparison::new(device, &baseline, &desc).unwrap(),
            Comparison::new(device, &baseline, &desc).unwrap(),
        ];
        for comparison in &comparisons {
            unsafe {
                context.CopyResource(&comparison.candidate, &candidate);
            }
        }
        let mut profiler = gpu_timing::Profiler::default();
        let mut gpu = [Vec::new(), Vec::new()];
        let mut observed = [Vec::new(), Vec::new()];
        for sample in 0..250 {
            let order = if sample % 2 == 0 { [0, 1] } else { [1, 0] };
            for index in order {
                let began = Instant::now();
                let mut trace = profiler.begin(device, context).unwrap().unwrap();
                kernels[index].enqueue(
                    context,
                    &mut comparisons[index],
                    &[Rect {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: height as i32,
                    }],
                );
                trace.mark(context, 1);
                profiler.finish(context, trace);
                unsafe {
                    context.Flush();
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut outcome = None;
                let mut timing = None;
                while outcome.is_none() || timing.is_none() {
                    if outcome.is_none() {
                        match comparisons[index].try_complete(context).unwrap() {
                            Outcome::Pending => {}
                            value => outcome = Some(value),
                        }
                    }
                    if timing.is_none() {
                        timing = profiler.poll(context).unwrap().into_iter().next();
                    }
                    assert!(
                        Instant::now() < deadline,
                        "comparison/timestamp did not complete"
                    );
                    if outcome.is_none() || timing.is_none() {
                        std::thread::sleep(Duration::from_micros(100));
                    }
                }
                assert_eq!(outcome.unwrap(), expected);
                if sample >= 10 {
                    gpu[index].push(timing.unwrap().stages_ms[0]);
                    observed[index].push(began.elapsed().as_secs_f64() * 1000.);
                }
            }
        }
        assert_eq!(profiler.invalid, 0);
        assert_eq!(profiler.dropped, 0);
        eprintln!(
            "comparison_batch_scope=dispatch_clear_and_16byte_readback_gpu_timestamp_not_shader_active_or_capture case={case} width={width} height={height} samples_per_group=240 gpu_8_p95_ms={:.6} gpu_16_p95_ms={:.6} observed_8_p95_ms={:.6} observed_16_p95_ms={:.6}",
            p95(gpu[0].clone()),
            p95(gpu[1].clone()),
            p95(observed[0].clone()),
            p95(observed[1].clone())
        );
    }
}

#[test]
fn source_gpu_timing_index_is_version_exact_and_rejects_bad_samples() {
    use super::super::screen_capture::diagnostics::{Event, Kind};
    let candidate = CaptureVersion {
        token: 7,
        sequence: 2,
        ticks: 100,
    };
    let event = Event {
        token: 7,
        sequence: 999,
        at: Instant::now(),
        kind: Kind::GpuComparison {
            candidate,
            copy_ms: 0.1,
            compare_ms: 0.2,
            observed_ms: 3.,
        },
    };
    let index = SourceGpuComparisons::from_events(&[event]);
    assert_eq!(index.versions.get(&candidate), Some(&Some([0.1, 0.2, 3.])));
    assert!(!index.versions.contains_key(&CaptureVersion {
        sequence: 999,
        ..candidate
    }));
    assert!(!index.versions.contains_key(&CaptureVersion {
        ticks: 101,
        ..candidate
    }));
    assert_eq!(
        SourceGpuComparisons::from_events(&[event, event])
            .versions
            .get(&candidate),
        Some(&None)
    );
    assert_eq!(
        SourceGpuComparisons::from_events(&[Event { token: 8, ..event }]).invalid,
        1
    );
    let invalid = Event {
        kind: Kind::GpuTimingInvalid { candidate },
        ..event
    };
    assert_eq!(
        SourceGpuComparisons::from_events(&[event, invalid])
            .versions
            .get(&candidate),
        Some(&None)
    );
    let skipped = Event {
        kind: Kind::GpuTimingSkipped { candidate },
        ..event
    };
    assert_eq!(SourceGpuComparisons::from_events(&[skipped]).skipped, 1);
    let bad = Event {
        kind: Kind::GpuComparison {
            candidate,
            copy_ms: f64::NAN,
            compare_ms: 0.,
            observed_ms: 0.,
        },
        ..event
    };
    assert_eq!(SourceGpuComparisons::from_events(&[bad]).invalid, 1);
    assert_eq!(
        SourceGpuComparisons::from_events(&[bad, event])
            .versions
            .get(&candidate),
        Some(&None)
    );
}

#[test]
#[ignore = "35-second capture-query-off calibration with identical paint probes; requires Release and WGC"]
fn gpu_live_capture_paint_latency_without_capture_queries() {
    run_case_with_probe(
        0,
        "paint-latency-large-blur-no-capture-queries",
        true,
        false,
        CompletionScope::Disabled,
    );
}

#[test]
#[ignore = "35-second completion-event experiment with capture timestamps off; requires Release and WGC"]
fn gpu_live_capture_paint_latency_with_completion_event() {
    run_case_with_probe(
        0,
        "paint-latency-large-blur-completion-event",
        true,
        false,
        CompletionScope::Owned,
    );
}

#[test]
#[ignore = "35-second production all-capture completion events with capture timestamps off; requires Release and WGC"]
fn gpu_live_capture_paint_latency_with_all_completion_events() {
    run_case_with_probe(
        0,
        "paint-latency-large-blur-all-completion-events",
        true,
        false,
        CompletionScope::All,
    );
}

#[test]
#[ignore = "35-second isolated output completion-event experiment; requires Release and WGC"]
fn gpu_live_capture_paint_latency_with_output_completion_events() {
    run_case_with_probe(
        0,
        "paint-latency-large-blur-output-completion-events",
        true,
        false,
        CompletionScope::CaptureAndOutput,
    );
}

/// Owned native backend lifecycle evidence; handle/GUI counts do not measure
/// GPU residency, process memory, or the full app's UI and input hooks.
#[test]
#[ignore = "opens owned native windows repeatedly; requires interactive Windows and Release"]
fn gpu_backend_reopen_releases_scene_and_native_handles() {
    if cfg!(debug_assertions) {
        panic!("run lifecycle verification with --release");
    }
    let counts = lifecycle::counts;
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    let source_frame = SourceFrame::new(source.0);
    source_frame.paint(37);
    source_frame.verify(37);
    drop(bootstrap);
    let mut retired = Vec::new();
    for round in 0..10 {
        let port = WindowsAnnotationRenderPort::new().unwrap();
        let mut requested = scene(bounds, input.0, round % 2);
        requested.stamp.session = 100 + round as u64;
        let requested = Arc::new(requested);
        let weak = Arc::downgrade(&requested);
        port.submit(requested).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            surface::pump_messages();
            if port.latest().unwrap().is_some_and(|frame| frame.presented) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "backend did not present round {round}"
            );
            thread::sleep(Duration::from_millis(1));
        }
        port.clear();
        drop(port); // Drop joins the worker before these owned HWNDs retire.
        assert!(
            weak.upgrade().is_none(),
            "retired backend retained scene round {round}"
        );
        // Pump pending window destruction, giving asynchronous WGC teardown a
        // fixed settling interval independent of the measured count.
        let settled = Instant::now() + Duration::from_millis(250);
        while Instant::now() < settled {
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        retired.push(counts());
    }
    eprintln!("backend_reopen_retired_handle_gdi_user_counts={retired:?}");
    let baseline = retired[2]; // First two rounds initialize process-wide driver caches.
    for &(handles, gdi, user) in &retired[3..] {
        assert!(
            handles <= baseline.0 + 4,
            "native handle growth beyond bounded background variation"
        );
        assert!(
            gdi <= baseline.1,
            "GDI objects retained after backend destruction"
        );
        assert!(
            user <= baseline.2,
            "USER objects retained after backend destruction"
        );
    }
    let owned = counts();
    drop(source_frame); // Selected bitmap and both DCs precede their HWND.
    drop(input);
    drop(source);
    surface::pump_messages();
    let released = counts();
    eprintln!("owned_fixture_release_before={owned:?} after={released:?}");
    assert!(
        released.1 < owned.1,
        "owned bitmap/DC resources did not release"
    );
    assert!(released.2 < owned.2, "owned HWND resources did not release");
}

#[test]
fn permit_poll_diagnostics_distinguish_unchecked_ready_and_hidden_waits() {
    let mut polls = PermitPolls::default();
    polls.observe(Stages::default(), Some(&RenderWait::Source));
    assert_eq!(polls, PermitPolls::default());
    polls.observe(
        Stages {
            compose_permit: Some(false),
            ..Default::default()
        },
        Some(&RenderWait::Presentation),
    );
    polls.observe(
        Stages {
            compose_permit: Some(true),
            commit_permit: Some(true),
            ..Default::default()
        },
        None,
    );
    polls.observe(
        Stages {
            commit_permit: Some(false),
            ..Default::default()
        },
        Some(&RenderWait::Source),
    );
    polls.observe(
        Stages {
            commit_permit: Some(false),
            ..Default::default()
        },
        Some(&RenderWait::Presentation),
    );
    assert_eq!(
        polls,
        PermitPolls {
            compose: 2,
            compose_blocked: 1,
            commit: 3,
            commit_blocked: 2,
            commit_blocked_source_wait: 1,
        }
    );
}

/// Retain uncertainty at integer refresh boundaries instead of guessing a missed deadline.
fn report_native_commit_samples(
    report: &mut String,
    name: &str,
    samples: &[super::display_timing::CommitSync],
    period_ms: f64,
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    let mut periods = std::collections::BTreeMap::new();
    let mut ambiguous = 0;
    for sample in samples {
        if let Some(count) = sample.whole_refreshes(period_ms) {
            *periods.entry(count).or_insert(0usize) += 1;
        } else {
            ambiguous += 1;
        }
    }
    report.push_str(&format!(
        "native_commit_{name}_samples={}\nnative_commit_{name}_call_p95_ms={}\nnative_commit_{name}_accepted_tail_p95_ms={}\nnative_commit_{name}_sync_lower_p95_ms={}\nnative_commit_{name}_sync_upper_p95_ms={}\nnative_commit_{name}_whole_refresh_period_histogram={periods:?}\nnative_commit_{name}_refresh_bucket_ambiguous={ambiguous}\n",
        samples.len(), metric(samples.iter().map(|s| s.call_ms).collect()),
        metric(samples.iter().map(|s| s.accepted_tail_ms).collect()),
        metric(samples.iter().map(|s| s.sync_ms.0).collect()),
        metric(samples.iter().map(|s| s.sync_ms.1).collect()),
    ));
}

/// Stratify same-version pairs using the measured three-refresh publication budget.
/// Counts use each group's own valid pairs; absent data is never reported as zero latency.
fn report_cadence_sources(
    report: &mut String,
    pairs: &[super::display_timing::CadencePair],
    publication_budget_ms: f64,
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    use super::display_timing::CadenceSource;
    report.push_str(&format!(
        "cadence_source_group_scope=displayed_source_identifier_gap_and_publication_api_complete_interval_not_gpu_complete_or_photon\ncadence_source_publication_budget_ms={publication_budget_ms:.6}\ncadence_source_budget_basis=3_times_median_valid_os_refresh_period\ncadence_source_unclassified={}\n",
        pairs.iter().filter(|pair| pair.source_group(publication_budget_ms).is_none()).count(),
    ));
    for (name, group) in [
        (
            "continuous_publication_within_budget",
            CadenceSource::ContinuousWithinBudget,
        ),
        (
            "continuous_publication_over_budget",
            CadenceSource::ContinuousOverBudget,
        ),
        (
            "skipped_source_identifiers",
            CadenceSource::SkippedIdentifiers,
        ),
    ] {
        let selected: Vec<_> = pairs
            .iter()
            .filter(|pair| pair.source_group(publication_budget_ms) == Some(group))
            .collect();
        let long: Vec<_> = selected
            .iter()
            .copied()
            .filter(|pair| pair.refresh_cycles > 3)
            .collect();
        let fraction = if selected.is_empty() {
            "unavailable".to_owned()
        } else {
            format!("{:.6}", long.len() as f64 / selected.len() as f64)
        };
        let mut widened = [0usize; 3];
        for pair in &long {
            if let Some(index) = pair.largest_growth() {
                widened[index] += 1;
            }
        }
        report.push_str(&format!(
            "cadence_source_{name}_pairs={}\ncadence_source_{name}_over_3_refreshes={}\ncadence_source_{name}_over_3_fraction={fraction}\ncadence_source_{name}_long_largest_growth_publish_to_dequeue={}\ncadence_source_{name}_long_largest_growth_dequeue_to_commit={}\ncadence_source_{name}_long_largest_growth_commit_to_sync={}\n",
            selected.len(), long.len(), widened[0], widened[1], widened[2],
        ));
        for (index, stage) in ["publication", "dequeue", "commit", "sync"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!(
                "cadence_source_{name}_long_{stage}_interval_p95_ms={}\n",
                metric(long.iter().map(|pair| pair.intervals_ms[index]).collect()),
            ));
        }
    }
}

#[test]
#[ignore = "35-second DXGI source comparison; requires Release and interactive D3D11/WGC"]
fn gpu_live_capture_paint_latency_with_dxgi_source() {
    run_case_with_source(
        0,
        "paint-latency-large-blur-dxgi-source",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
    );
}

#[test]
#[ignore = "35-second same-candidate capture-query observation/confirmation diagnostic; requires Release and WGC"]
fn gpu_live_capture_confirmation_observation_gap() {
    run_case_with_source(
        0,
        "capture-confirmation-observation-gap",
        true,
        true,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
    );
}

#[test]
#[ignore = "35-second same-candidate query poll readiness bracket; requires Release and WGC"]
fn gpu_live_capture_query_poll_readiness_bracket() {
    run_case_with_source(
        0,
        "capture-query-poll-readiness-bracket",
        true,
        true,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
    );
}

#[test]
#[ignore = "35-second paired query-gap worker/wait partition; requires Release and WGC"]
fn gpu_live_capture_query_gap_worker_wait_partition() {
    run_case_with_composition_probe(
        0,
        "capture-query-gap-worker-wait-partition",
        true,
        true,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::WaitsOnly,
    );
}

#[test]
#[ignore = "35-second paired query-gap capture enumeration/view diagnostic; requires Release and WGC"]
fn gpu_live_capture_query_gap_capture_details() {
    run_case_with_composition_probe(
        0,
        "capture-query-gap-capture-details",
        true,
        true,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::WaitsAndEnumeration,
    );
}

fn report_composition_samples(
    report: &mut String,
    name: &str,
    samples: &[super::composition_timing::Sample],
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    let mut rates = std::collections::BTreeMap::new();
    let mut error_cycles = std::collections::BTreeMap::new();
    let mut invalid_error_cycles = 0;
    for s in samples {
        *rates.entry(s.rate).or_insert(0usize) += 1;
        if let Some(cycles) = s.estimate_error_cycles() {
            *error_cycles.entry(cycles).or_insert(0usize) += 1;
        } else {
            invalid_error_cycles += 1;
        }
    }
    report.push_str(&format!(
        "composition_{name}_signed_error_rounded_cycles_histogram={error_cycles:?}\ncomposition_{name}_signed_error_cycles_invalid={invalid_error_cycles}\n"
    ));
    report.push_str(&format!(
        "composition_{name}_scope=same_device_pre_commit_estimate_associated_to_pixel_version_sync_not_pickup_deadline_or_photon\ncomposition_{name}_samples={}\ncomposition_{name}_rate_rational_histogram={rates:?}\ncomposition_{name}_period_p95_ms={}\ncomposition_{name}_query_p95_ms={}\ncomposition_{name}_current_minus_last_frame_p95_ms={}\ncomposition_{name}_estimated_lead_lower_p95_ms={}\ncomposition_{name}_estimated_lead_upper_p95_ms={}\ncomposition_{name}_sync_minus_estimate_p95_ms={}\ncomposition_{name}_absolute_sync_estimate_error_p95_ms={}\ncomposition_{name}_estimate_definitely_stale={}\n",
        samples.len(), metric(samples.iter().map(|s|s.period_ms).collect()),
        metric(samples.iter().map(|s|s.query_ms).collect()), metric(samples.iter().map(|s|s.current_minus_last_ms).collect()),
        metric(samples.iter().map(|s|s.estimated_lead_ms.0).collect()), metric(samples.iter().map(|s|s.estimated_lead_ms.1).collect()),
        metric(samples.iter().map(|s|s.sync_minus_estimate_ms).collect()), metric(samples.iter().map(|s| s.sync_minus_estimate_ms.abs()).collect()),
        samples.iter().filter(|s|s.estimated_lead_ms.1<0.).count(),
    ));
}
#[test]
#[ignore = "35-second same-device composition clock probe; requires Release and WGC"]
fn gpu_live_capture_paint_latency_with_composition_clock() {
    run_case_with_composition_probe(
        0,
        "paint-latency-large-blur-composition-clock",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::Composition,
    );
}

/// Adjacent pixel-identified versions on one swap chain, rather than unrelated
/// marginal percentiles. Missing either clock leaves the pair unclassified.
fn report_prediction_intervals(
    report: &mut String,
    position: &str,
    frames: &[super::display_timing::CadenceFrame],
    samples: &HashMap<u32, super::composition_timing::Sample>,
    publication_budget_ms: f64,
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    for long_only in [false, true] {
        let name = if long_only {
            "continuous_publication_within_budget_long"
        } else {
            "all"
        };
        let mut values = Vec::new();
        let mut expansion_cycles = std::collections::BTreeMap::new();
        let mut missing = 0;
        let mut invalid = 0;
        for frames in frames.windows(2) {
            let Some(pair) = super::display_timing::cadence_pair(frames[0], frames[1]) else {
                continue;
            };
            if long_only
                && (pair.refresh_cycles <= 3
                    || pair.source_group(publication_budget_ms)
                        != Some(super::display_timing::CadenceSource::ContinuousWithinBudget))
            {
                continue;
            }
            let (Some(previous), Some(current)) =
                (samples.get(&frames[0].paint), samples.get(&frames[1].paint))
            else {
                missing += 1;
                continue;
            };
            let Some(intervals) = previous.adjacent_intervals_ms(*current) else {
                invalid += 1;
                continue;
            };
            *expansion_cycles
                .entry((intervals.2 / current.period_ms).round() as i64)
                .or_insert(0usize) += 1;
            values.push(intervals);
        }
        report.push_str(&format!(
            "prediction_intervals_{position}_{name}_samples={}\nprediction_intervals_{position}_{name}_missing={missing}\nprediction_intervals_{position}_{name}_invalid={invalid}\nprediction_intervals_{position}_{name}_predicted_p95_ms={}\nprediction_intervals_{position}_{name}_actual_sync_p95_ms={}\nprediction_intervals_{position}_{name}_signed_expansion_p95_ms={}\nprediction_intervals_{position}_{name}_expansion_rounded_cycles_histogram={expansion_cycles:?}\n",
            values.len(), metric(values.iter().map(|v|v.0).collect()),
            metric(values.iter().map(|v|v.1).collect()), metric(values.iter().map(|v|v.2).collect()),
        ));
    }
}

#[test]
#[ignore = "35-second worker wake probe; requires Release and an owned WGC/DXGI source"]
fn gpu_live_capture_paint_latency_with_worker_wake_clock() {
    run_case_with_composition_probe(
        0,
        "paint-latency-large-blur-worker-wake-clock",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::CompositionAndWaits,
    );
}

#[test]
#[ignore = "35-second capture enumeration probe; requires Release and owned WGC/DXGI source"]
fn gpu_live_capture_paint_latency_with_enumeration_clock() {
    run_case_with_composition_probe(
        0,
        "paint-latency-large-blur-enumeration-clock",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::CompositionAndEnumeration,
    );
}

#[test]
#[ignore = "35-second event fallback regression; requires Release and owned WGC/DXGI source"]
fn gpu_live_capture_paint_latency_without_precise_timer() {
    run_case_with_composition_probe(
        0,
        "paint-latency-large-blur-event-fallback",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::CompositionAndFallback,
    );
}

#[test]
#[ignore = "35-second all-source covered fixture; requires Release and owned WGC/DXGI source"]
fn gpu_covered_effect_latency_with_all_sources() {
    run_case_with_composition_probe(
        0,
        "covered-effect-all-sources",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::CoveredAllSources,
    );
}

#[test]
#[ignore = "35-second single-source covered fixture; requires Release and owned WGC/DXGI source"]
fn gpu_covered_effect_latency_with_owned_source() {
    run_case_with_composition_probe(
        0,
        "covered-effect-owned-source",
        true,
        false,
        CompletionScope::CaptureAndOutput,
        SourceKind::Dxgi,
        ClockProbes::CoveredOwnedSource,
    );
}
