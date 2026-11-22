//! Sequential background orchestration; matching and acceptance policy belong to Core.
use super::*;
use lexift_core::domain::long_capture::{
    Axis, Feedback, Growth, Preview, Stitcher, Trim,
    auto_scroll::{self, Mode, Next, Steps, Stop},
    workflow::{Command, Event, Phase},
};
use std::time::{Duration, Instant};

use lexift_core::{domain::geometry::Rect, ports::screen_capture::LongScrollTarget};

#[derive(Clone, Copy, Default)]
struct AutoAuthorization {
    state: auto_scroll::State,
    axis: Axis,
}

type AutoControl = Mutex<AutoAuthorization>;
type AutoEvents<'a> = (&'a AutoControl, &'a dyn Fn(u64, Option<Stop>));
type TrimQueue = Mutex<Option<(u64, Option<Trim>)>>;
type TrimEvents<'a> = (&'a TrimQueue, &'a dyn Fn());
type AxisQueue = Mutex<Option<(u64, u64, Axis)>>;
type AxisEvents<'a> = (
    &'a AxisQueue,
    &'a dyn Fn(u64, u64, Axis, Option<Preview>),
    [(usize, usize); 2],
);
type CaptureEvents<'a> = (
    Option<AutoEvents<'a>>,
    Option<TrimEvents<'a>>,
    Option<AxisEvents<'a>>,
);

pub(super) struct Job {
    session: u64,
    operation: u64,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    choosing_directory: Arc<AtomicBool>,
    finish: Arc<AtomicBool>,
    auto: Arc<AutoControl>,
    trim: Arc<TrimQueue>,
    axis: Arc<AxisQueue>,
}
pub(super) fn sync(
    runtime: &ScreenshotRuntime,
    state: &lexift_core::domain::screenshot::ScreenshotState,
    quitting: bool,
) {
    let mut job = runtime.long.lock().unwrap();
    if job.as_ref().is_some_and(|j| {
        quitting
            || state.session != j.session
            || !state.active()
            || state.long.as_ref().is_none_or(|l| {
                l.operation != j.operation || matches!(l.phase, Phase::Editing | Phase::Frozen)
            })
    }) && let Some(j) = job.take()
    {
        let mut desired = j.auto.lock().unwrap();
        *desired = AutoAuthorization::default();
        j.cancel.store(true, Ordering::Release);
    } else if let (Some(job), Some(long)) = (job.as_ref(), state.long.as_ref()) {
        *job.auto.lock().unwrap() = AutoAuthorization {
            state: long.auto_scroll,
            axis: long.axis,
        };
        job.choosing_directory
            .store(state.quick_directory_pending, Ordering::Release);
    }
}
fn emit(c: &Arc<AppController>, event: Event) {
    c.dispatch(AppEvent::Screenshot(ScreenshotEvent::Long(event)));
}

pub(super) fn execute(runtime: &Arc<ScreenshotRuntime>, c: &Arc<AppController>, command: Command) {
    match command {
        Command::Prepare { .. } | Command::Present { .. } => c.ui.long_screenshot(command),
        Command::ChangeAxis {
            session,
            operation,
            request,
            sequence,
            axis,
        } => {
            if let Some(job) = runtime
                .long
                .lock()
                .unwrap()
                .as_ref()
                .filter(|job| job.session == session && job.operation == operation)
            {
                *job.axis.lock().unwrap() = Some((request, sequence, axis));
            }
        }
        Command::Trim {
            session,
            operation,
            sequence,
            direction,
        } => {
            if let Some(job) = runtime
                .long
                .lock()
                .unwrap()
                .as_ref()
                .filter(|job| job.session == session && job.operation == operation)
            {
                *job.trim.lock().unwrap() = Some((sequence, direction));
            }
        }
        Command::Restore { session, operation } => {
            {
                let mut owned = runtime.session.lock().unwrap();
                if let Some(s) = owned.as_mut().filter(|s| s.id == session)
                    && let Some(original) = s.long_source_frames.take()
                {
                    s.frames = Some(original);
                }
            }
            c.ui.long_screenshot(Command::Restore { session, operation });
        }
        Command::Pause {
            session,
            operation,
            paused,
        } => {
            if let Some(j) = runtime
                .long
                .lock()
                .unwrap()
                .as_ref()
                .filter(|j| j.session == session && j.operation == operation)
            {
                j.paused.store(paused, Ordering::Release);
            }
        }
        Command::Finish { session, operation } => {
            if let Some(j) = runtime
                .long
                .lock()
                .unwrap()
                .as_ref()
                .filter(|j| j.session == session && j.operation == operation)
            {
                j.finish.store(true, Ordering::Release);
            }
        }
        Command::Edit {
            session,
            operation,
            image,
            target,
        } => {
            // Replacing the screenshot base happens only after Core accepts the completion identity.
            let accepted = {
                let mut owned = runtime.session.lock().unwrap();
                if let Some(s) = owned
                    .as_mut()
                    .filter(|s| s.id == session && !s.cancelled.load(Ordering::Acquire))
                {
                    if s.long_source_frames.is_none() {
                        s.long_source_frames = s.frames.clone();
                    }
                    s.frames = Some(Arc::new(vec![(*image).clone()]));
                    true
                } else {
                    false
                }
            };
            if accepted {
                c.ui.long_screenshot(Command::Edit {
                    session,
                    operation,
                    image,
                    target,
                });
            }
        }
        Command::Capture {
            session,
            operation,
            bounds,
            excluded,
            preview_size,
        } => {
            let cancel = Arc::new(AtomicBool::new(false));
            let paused = Arc::new(AtomicBool::new(false));
            let choosing_directory = Arc::new(AtomicBool::new(false));
            let finish = Arc::new(AtomicBool::new(false));
            let auto = Arc::new(AutoControl::default());
            let trim = Arc::new(TrimQueue::default());
            let axis = Arc::new(AxisQueue::default());
            let horizontal_preview = (preview_size.2, preview_size.0.min(preview_size.1));
            let vertical_preview = (preview_size.0, preview_size.1);
            if let Some(old) = runtime.long.lock().unwrap().replace(Job {
                session,
                operation,
                cancel: cancel.clone(),
                paused: paused.clone(),
                choosing_directory: choosing_directory.clone(),
                finish: finish.clone(),
                auto: auto.clone(),
                trim: trim.clone(),
                axis: axis.clone(),
            }) {
                let mut desired = old.auto.lock().unwrap();
                *desired = AutoAuthorization::default();
                old.cancel.store(true, Ordering::Release);
            }
            let factory = runtime.capture;
            let c = c.clone();
            c.clone().runtime.spawn_blocking(move || {
                let result = run(
                    factory,
                    bounds,
                    excluded,
                    (&cancel, &paused, &finish, &choosing_directory),
                    (
                        Some((&auto, &|generation, stop| {
                            emit(
                                &c,
                                match stop {
                                    Some(reason) => Event::AutoScrollStopped {
                                        session,
                                        operation,
                                        generation,
                                        reason,
                                    },
                                    None => Event::AutoScrollStarted {
                                        session,
                                        operation,
                                        generation,
                                    },
                                },
                            );
                        })),
                        Some((&trim, &|| emit(&c, Event::TrimReady { session, operation }))),
                        Some((
                            &axis,
                            &|request, sequence, axis, preview| {
                                emit(
                                    &c,
                                    Event::AxisChanged {
                                        session,
                                        operation,
                                        request,
                                        sequence,
                                        axis,
                                        preview,
                                    },
                                );
                            },
                            [vertical_preview, horizontal_preview],
                        )),
                    ),
                    || c.physical_displays(),
                    |sequence, stitch, feedback| {
                        let (width, height) = stitch.size();
                        emit(
                            &c,
                            Event::Progress {
                                session,
                                operation,
                                sequence,
                                width,
                                height,
                                feedback,
                                preview: {
                                    let (w, h) = if stitch.axis() == Axis::Horizontal {
                                        horizontal_preview
                                    } else {
                                        vertical_preview
                                    };
                                    stitch.preview(w, h)
                                },
                            },
                        );
                    },
                );
                if !cancel.load(Ordering::Acquire) {
                    emit(
                        &c,
                        Event::Finished {
                            session,
                            operation,
                            result: result.map_err(|error| {
                                tracing::debug!(%error, "Long capture preparation failed");
                                Error::new(
                                    "Long capture unavailable. Select a smaller scrolling area.",
                                )
                            }),
                        },
                    );
                }
            });
        }
    }
}

struct AutoRun {
    generation: u64,
    axis: Axis,
    started: Instant,
    anchor: Option<(LongScrollTarget, Instant)>,
    steps: Steps,
    sent: Option<Instant>,
    stopped: bool,
}

impl AutoRun {
    fn new(generation: u64, axis: Axis) -> Self {
        Self {
            generation,
            axis,
            started: Instant::now(),
            anchor: None,
            steps: Steps::default(),
            sent: None,
            stopped: false,
        }
    }

    fn settling(&self, aligned: bool) -> bool {
        self.steps.settling(self.started.elapsed(), aligned)
    }

    fn stop(&mut self, notify: &dyn Fn(u64, Option<Stop>), reason: Stop) {
        self.stopped = true;
        tracing::debug!(
            generation = self.generation,
            axis = ?self.axis,
            ?reason,
            "Long capture automatic scroll stopped"
        );
        notify(self.generation, Some(reason));
    }

    fn tick(
        &mut self,
        (control, notify): AutoEvents<'_>,
        mode: Mode,
        port: &dyn ScreenCapturePort,
        bounds: Rect,
        aligned: bool,
    ) {
        if self.stopped {
            return;
        }
        if mode == Mode::Waiting {
            if !port.supports_long_scroll() {
                self.stop(notify, Stop::InputUnavailable);
                return;
            }
            if self.started.elapsed() >= auto_scroll::TARGET_WAIT {
                self.stop(notify, Stop::TargetTimeout);
                return;
            }
            let current = port.long_scroll_target(bounds);
            match self.anchor {
                Some((anchor, since)) if Some(anchor) == current => {
                    if since.elapsed() >= auto_scroll::TARGET_DWELL {
                        notify(self.generation, None);
                    }
                }
                _ => self.anchor = current.map(|target| (target, Instant::now())),
            }
            return;
        }
        let Some((target, _)) = self.anchor else {
            return;
        };
        if !port.long_scroll_matches(bounds, target) {
            self.stop(notify, Stop::TargetChanged);
            return;
        }
        match self.steps.next(self.started.elapsed(), aligned) {
            Next::Wait => {}
            Next::Stop(reason) => self.stop(notify, reason),
            Next::Scroll => {
                // Keep final authorization and wheel injection ordered with session retirement.
                let result = {
                    let desired = control.lock().unwrap();
                    if desired.state.generation != self.generation
                        || desired.state.mode != Mode::Running
                        || desired.axis != self.axis
                    {
                        return;
                    }
                    port.long_scroll_step(bounds, target, self.axis)
                };
                match result {
                    Ok(()) => {
                        let now = Instant::now();
                        self.sent = Some(now);
                        self.steps.sent(now.duration_since(self.started));
                    }
                    Err(error) => {
                        tracing::debug!(%error, "Long capture wheel input failed");
                        self.stop(notify, Stop::InputUnavailable);
                    }
                }
            }
        }
    }
}

fn run(
    factory: impl FnOnce() -> Option<Arc<dyn ScreenCapturePort>>,
    bounds: lexift_core::domain::geometry::Rect,
    excluded: Vec<lexift_core::ports::magnifier::WindowToken>,
    control: (&AtomicBool, &AtomicBool, &AtomicBool, &AtomicBool),
    events: CaptureEvents<'_>,
    layout: impl Fn() -> Result<Vec<lexift_core::domain::geometry::Rect>>,
    mut progress: impl FnMut(u64, &Stitcher, Feedback),
) -> Result<Arc<CapturedRegion>> {
    let (cancel, paused, finish, choosing_directory) = control;
    let (auto, trim, axis) = events;
    let normalize = |mut b: Vec<lexift_core::domain::geometry::Rect>| {
        b.sort_by_key(|r| (r.left, r.top, r.right, r.bottom));
        b
    };
    let original_layout = normalize(layout()?);
    let mut layout_checked = Instant::now();
    let port = factory().ok_or_else(|| Error::new("Long capture unavailable"))?;
    struct Capture(Arc<dyn ScreenCapturePort>);
    impl Drop for Capture {
        fn drop(&mut self) {
            self.0.clear();
        }
    }
    let _capture = Capture(port.clone());
    let request = lexift_core::ports::screen_capture::ScreenCaptureRequest {
        revision: 1,
        regions: vec![bounds],
        paused_regions: vec![],
        excluded_windows: excluded,
    };
    port.request(request.clone())?;
    let mut stitch = None::<Stitcher>;
    let mut serial = None;
    let mut sequence = 0;
    let mut suspended = false;
    let mut last_feedback = Feedback::Unchanged;
    let mut auto_run: Option<AutoRun> = None;
    let mut trim_request = None;
    let mut last_frame = Instant::now();
    let mut unsettled_since: Option<Instant> = None;
    struct Stats {
        start: Instant,
        frames: u64,
        accepted: u64,
        rejected: u64,
        matching: Duration,
        feedback: Duration,
        flatten: Duration,
        working_peak: usize,
        peak: usize,
        trimming: Duration,
        trims: u64,
        located: u64,
        locating: Duration,
    }
    impl Drop for Stats {
        fn drop(&mut self) {
            tracing::debug!(
                frames = self.frames,
                accepted = self.accepted,
                rejected = self.rejected,
                matching_ms = self.matching.as_secs_f64() * 1000.,
                feedback_ms = self.feedback.as_secs_f64() * 1000.,
                elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.,
                retained_pixel_peak_bytes = self.peak,
                accounted_worker_pixel_peak_bytes = self.working_peak,
                flatten_ms = self.flatten.as_secs_f64() * 1000.,
                trim_ms = self.trimming.as_secs_f64() * 1000.,
                trims = self.trims,
                located = self.located,
                located_frame_matching_ms = self.locating.as_secs_f64() * 1000.,
                "Long capture summary"
            );
        }
    }
    let mut stats = Stats {
        start: Instant::now(),
        frames: 0,
        accepted: 0,
        rejected: 0,
        matching: Duration::ZERO,
        feedback: Duration::ZERO,
        flatten: Duration::ZERO,
        working_peak: 0,
        peak: 0,
        trimming: Duration::ZERO,
        trims: 0,
        located: 0,
        locating: Duration::ZERO,
    };
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("Long capture cancelled"));
        }
        if finish.load(Ordering::Acquire) {
            port.clear();
            let stitch = stitch
                .as_ref()
                .ok_or_else(|| Error::new("Long capture unavailable"))?;
            let started = Instant::now();
            let (width, height) = stitch.size();
            stats.working_peak = stats
                .working_peak
                .max(stitch.retained_bytes() + width * height * 4);
            let result = stitch.flatten(cancel).map(Arc::new);
            stats.flatten += started.elapsed();
            return result;
        }
        if let Some((queue, notify, sizes)) = axis
            && unsettled_since.is_none()
            && !auto_run.as_ref().is_some_and(|run| {
                run.settling(
                    stitch.as_ref().is_some_and(Stitcher::at_tail)
                        && matches!(last_feedback, Feedback::Accepted | Feedback::Unchanged),
                )
            })
        {
            let request = queue.lock().unwrap().take();
            if let Some((request, expected, desired)) = request {
                let started = Instant::now();
                let preview = if expected == sequence
                    && !paused.load(Ordering::Acquire)
                    && !choosing_directory.load(Ordering::Acquire)
                    && let Some(stitch) = stitch.as_mut()
                    && stitch.change_axis(desired, cancel).unwrap_or(false)
                {
                    // Re-evaluate the last delivered capture on the new axis even if the desktop is static.
                    serial = None;
                    sequence += 1;
                    last_feedback = Feedback::Unchanged;
                    let (width, height) = sizes[usize::from(desired == Axis::Horizontal)];
                    Some(stitch.preview(width, height))
                } else {
                    None
                };
                tracing::debug!(
                    request,
                    sequence,
                    ?desired,
                    applied = preview.is_some(),
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
                    "Long capture direction resolved"
                );
                notify(request, sequence, desired, preview);
            }
        }
        if paused.load(Ordering::Acquire) || choosing_directory.load(Ordering::Acquire) {
            if let Some((queue, ready)) = trim
                && (trim_request.take().is_some() || queue.lock().unwrap().take().is_some())
            {
                ready();
            }
            if !suspended {
                port.clear();
                unsettled_since = None;
                suspended = true;
            }
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }
        if suspended {
            port.request(request.clone())?;
            suspended = false;
            serial = None;
            last_frame = Instant::now();
            if let Some(stitch) = &mut stitch {
                stitch.invalidate_position();
                sequence += 1;
                last_feedback = Feedback::Unchanged;
                progress(sequence, stitch, last_feedback);
            }
        }
        if layout_checked.elapsed() >= Duration::from_millis(250) {
            layout_checked = Instant::now();
            if layout().map_or(true, |b| normalize(b) != original_layout) {
                if let Some(s) = &mut stitch {
                    unsettled_since = None;
                    s.invalidate_position();
                    paused.store(true, Ordering::Release);
                    sequence += 1;
                    progress(sequence, s, Feedback::CaptureLost);
                    continue;
                }
                return Err(Error::new("Display layout changed"));
            }
        }
        match port.latest() {
            Ok(Some(frame)) if serial != Some(frame.serial) => {
                serial = Some(frame.serial);
                last_frame = Instant::now();
                let image = Arc::new(crop_frames(&frame.regions, bounds)?);
                if choosing_directory.load(Ordering::Acquire) {
                    continue;
                }
                let retained_before = stitch.as_ref().map_or(0, Stitcher::retained_bytes);
                let incoming = image.pixels.len();
                let horizontal = stitch
                    .as_ref()
                    .is_some_and(|s| s.axis() == Axis::Horizontal);
                let first = stitch.is_none();
                let matching = Instant::now();
                let prior_position = stitch.as_ref().and_then(Stitcher::position);
                let tail_only = auto
                    .is_some_and(|(control, _)| control.lock().unwrap().state.mode != Mode::Manual)
                    || auto_run.as_ref().is_some_and(|run| {
                        let aligned = stitch.as_ref().is_some_and(Stitcher::at_tail)
                            && matches!(last_feedback, Feedback::Accepted | Feedback::Unchanged);
                        run.settling(aligned)
                    });
                stats.frames += 1;
                let feedback = if let Some(stitch) = &mut stitch {
                    let growth = if tail_only {
                        Growth::TailOnly
                    } else {
                        Growth::Both
                    };
                    stitch.sample(image, growth, cancel)?
                } else {
                    stitch = Some(Stitcher::new(image)?);
                    Feedback::Accepted
                };
                stats.matching += matching.elapsed();
                if feedback == Feedback::Located {
                    stats.located += 1;
                    stats.locating += matching.elapsed();
                }
                stats.accepted += u64::from(feedback == Feedback::Accepted);
                stats.rejected += u64::from(feedback == Feedback::Uncertain);
                let retained = stitch.as_ref().unwrap().retained_bytes();
                stats.peak = stats.peak.max(retained);
                stats.working_peak = stats
                    .working_peak
                    .max(retained.max(retained_before) + incoming * if horizontal { 2 } else { 1 });
                let stitch = stitch.as_mut().unwrap();
                // The capture port advances its serial only when pixels change. An
                // intermediate compositor frame can be unmatchable during a scroll.
                let unsettled = feedback == Feedback::Uncertain && stitch.position().is_some();
                if unsettled {
                    unsettled_since.get_or_insert(last_frame);
                } else {
                    unsettled_since = None;
                    if feedback == Feedback::Limit {
                        stitch.invalidate_position();
                    }
                }
                if !unsettled
                    && (first
                        || feedback != last_feedback
                        || feedback == Feedback::Accepted
                        || stitch.position() != prior_position)
                {
                    sequence += 1;
                    let feedback_started = Instant::now();
                    progress(sequence, stitch, feedback);
                    stats.feedback += feedback_started.elapsed();
                    if feedback == Feedback::Located || feedback == Feedback::Uncertain {
                        tracing::trace!(
                            sequence,
                            ?feedback,
                            "Long capture viewport located or lost"
                        );
                    }
                }
                last_feedback = feedback;
                if tail_only && let Some(run) = &mut auto_run {
                    run.steps.frame(feedback, run.started.elapsed());
                    if feedback == Feedback::Accepted
                        && let Some(sent) = run.sent.take()
                    {
                        tracing::debug!(
                            generation = run.generation,
                            axis = ?run.axis,
                            step_ms = sent.elapsed().as_secs_f64() * 1000.,
                            elapsed_ms = run.started.elapsed().as_secs_f64() * 1000.,
                            "Long capture automatic scroll confirmed"
                        );
                    }
                }
                if feedback == Feedback::Limit {
                    paused.store(true, Ordering::Release);
                }
            }
            Err(error) if stitch.is_none() => return Err(error),
            Err(_) => {
                unsettled_since = None;
                paused.store(true, Ordering::Release);
                sequence += 1;
                let stitch = stitch.as_mut().unwrap();
                stitch.invalidate_position();
                progress(sequence, stitch, Feedback::CaptureLost);
            }
            _ if stitch.is_none() && last_frame.elapsed() > Duration::from_secs(3) => {
                return Err(Error::new("Long capture unavailable"));
            }
            _ => {}
        }
        if unsettled_since.is_some_and(|since| {
            last_frame.elapsed() >= auto_scroll::SETTLE
                || since.elapsed() >= auto_scroll::STEP_TIMEOUT
        }) && let Some(stitch) = &mut stitch
        {
            unsettled_since = None;
            stitch.invalidate_position();
            sequence += 1;
            progress(sequence, stitch, Feedback::Uncertain);
        }
        let aligned = stitch.as_ref().is_some_and(Stitcher::at_tail)
            && matches!(last_feedback, Feedback::Accepted | Feedback::Unchanged);
        let settling = auto_run.as_ref().is_some_and(|run| run.settling(aligned));
        if let Some((queue, ready)) = trim {
            if trim_request.is_none() {
                trim_request = queue.lock().unwrap().take();
            }
            if !settling
                && unsettled_since.is_none()
                && let Some((expected, direction)) = trim_request.take()
            {
                let started = Instant::now();
                if let Some(direction) = direction
                    && sequence == expected
                    && !cancel.load(Ordering::Acquire)
                    && !paused.load(Ordering::Acquire)
                    && let Some(stitch) = stitch.as_mut()
                    && stitch.trim(direction)
                {
                    stats.trims += 1;
                    sequence += 1;
                    last_feedback = if stitch.at_tail() {
                        Feedback::Unchanged
                    } else {
                        Feedback::Located
                    };
                    progress(sequence, stitch, last_feedback);
                    tracing::debug!(
                        sequence,
                        ?direction,
                        width = stitch.size().0,
                        height = stitch.size().1,
                        elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
                        "Long capture trim committed"
                    );
                }
                stats.trimming += started.elapsed();
                ready();
            }
        }
        if let Some((control, notify)) = auto {
            let desired = *control.lock().unwrap();
            if desired.state.mode == Mode::Manual {
                if !settling {
                    auto_run = None;
                }
            } else if !paused.load(Ordering::Acquire)
                && !finish.load(Ordering::Acquire)
                && !cancel.load(Ordering::Acquire)
                && stitch.as_ref().is_some_and(|s| s.axis() == desired.axis)
                // A quick restart must leave the preceding wheel step time to settle.
                && (!settling
                    || auto_run.as_ref().is_some_and(|run| {
                        run.generation == desired.state.generation && run.axis == desired.axis
                    }))
            {
                let run = auto_run
                    .get_or_insert_with(|| AutoRun::new(desired.state.generation, desired.axis));
                if run.generation != desired.state.generation || run.axis != desired.axis {
                    *run = AutoRun::new(desired.state.generation, desired.axis);
                }
                run.tick(
                    (control, notify),
                    desired.state.mode,
                    port.as_ref(),
                    bounds,
                    aligned,
                );
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::geometry::Rect;
    use std::{collections::VecDeque, sync::atomic::AtomicUsize};
    struct Capture {
        frames: Mutex<VecDeque<Arc<ScreenCaptureFrame>>>,
        requests: AtomicUsize,
        clears: AtomicUsize,
    }
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(
            &self,
            _: lexift_core::ports::screen_capture::ScreenCaptureRequest,
        ) -> Result<()> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
            let mut frames = self.frames.lock().unwrap();
            Ok(if frames.len() > 1 {
                frames.pop_front()
            } else {
                frames.front().cloned()
            })
        }
        fn clear(&self) {
            self.clears.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn frame(y0: usize) -> CapturedRegion {
        let mut pixels = vec![];
        for y in y0..y0 + 80 {
            for x in 0..64 {
                let v = (y as u32)
                    .wrapping_mul(747796405)
                    .wrapping_add((x as u32).wrapping_mul(2891336453));
                pixels.extend_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]);
            }
        }
        CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 80,
            },
            pixels,
        }
    }
    fn capture() -> Arc<Capture> {
        Arc::new(Capture {
            frames: Mutex::new(
                [0, 20]
                    .into_iter()
                    .enumerate()
                    .map(|(i, y)| {
                        Arc::new(ScreenCaptureFrame {
                            revision: 1,
                            serial: i as u64 + 1,
                            regions: vec![frame(y)],
                        })
                    })
                    .collect(),
            ),
            requests: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
        })
    }
    #[test]
    fn direction_switch_rechecks_the_latest_capture_even_with_the_same_serial() {
        fn columns(x0: usize, width: usize) -> CapturedRegion {
            let mut image = frame(0);
            image.bounds.right = width as i32;
            image.pixels = (0u32..80)
                .flat_map(|y| {
                    (x0..x0 + width).flat_map(move |x| {
                        let v = (x as u32 + 1).wrapping_mul(0x9e3779b9).rotate_left(y % 29)
                            ^ y.wrapping_mul(0x85ebca6b);
                        [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]
                    })
                })
                .collect();
            image
        }
        let bounds = columns(0, 64).bounds;
        let port = Arc::new(Capture {
            frames: Mutex::new(
                [0, 20]
                    .into_iter()
                    .enumerate()
                    .map(|(index, x)| {
                        Arc::new(ScreenCaptureFrame {
                            revision: 1,
                            serial: index as u64 + 1,
                            regions: vec![columns(x, 64)],
                        })
                    })
                    .collect(),
            ),
            requests: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
        });
        let queue = AxisQueue::default();
        let cancel = AtomicBool::new(false);
        let finish = AtomicBool::new(false);
        let (done, deadline) = std::sync::mpsc::channel();
        let result = std::thread::scope(|scope| {
            let cancelled = &cancel;
            scope.spawn(move || {
                if deadline.recv_timeout(Duration::from_secs(3)).is_err() {
                    cancelled.store(true, Ordering::Release);
                }
            });
            let result = run(
                || Some(port),
                bounds,
                vec![],
                (
                    &cancel,
                    &AtomicBool::new(false),
                    &finish,
                    &AtomicBool::new(false),
                ),
                (
                    None,
                    None,
                    Some((
                        &queue,
                        &|request, version, axis, preview| {
                            assert_eq!((request, version, axis), (1, 3, Axis::Horizontal));
                            assert!(preview.is_some());
                        },
                        [(30, 50), (50, 30)],
                    )),
                ),
                || Ok(vec![bounds]),
                |sequence, stitch, feedback| match sequence {
                    1 => assert_eq!(feedback, Feedback::Accepted),
                    2 => {
                        assert_eq!(feedback, Feedback::Uncertain);
                        *queue.lock().unwrap() = Some((1, sequence, Axis::Horizontal));
                    }
                    4 => {
                        assert_eq!(feedback, Feedback::Accepted);
                        assert_eq!(stitch.size(), (84, 80));
                        finish.store(true, Ordering::Release);
                    }
                    _ => unreachable!(),
                },
            );
            let _ = done.send(());
            result
        })
        .unwrap();
        assert_eq!(result.pixels, columns(0, 84).pixels);
    }

    #[test]
    fn automatic_scroll_revalidates_authorization_before_injecting() {
        use lexift_core::{domain::geometry::Point, ports::magnifier::WindowToken};
        struct Wheel(Mutex<Vec<Axis>>, bool);
        impl ScreenCapturePort for Wheel {
            fn is_available(&self) -> bool {
                true
            }
            fn request(
                &self,
                _: lexift_core::ports::screen_capture::ScreenCaptureRequest,
            ) -> Result<()> {
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
            fn long_scroll_matches(&self, _: Rect, _: LongScrollTarget) -> bool {
                true
            }
            fn long_scroll_step(&self, _: Rect, _: LongScrollTarget, axis: Axis) -> Result<()> {
                self.0.lock().unwrap().push(axis);
                if self.1 {
                    return Err(Error::new("fixture rejected input"));
                }
                Ok(())
            }
        }
        let gate = AutoControl::new(AutoAuthorization {
            state: auto_scroll::State {
                generation: 2,
                mode: Mode::Running,
                reason: None,
            },
            axis: Axis::Horizontal,
        });
        let bounds = frame(0).bounds;
        let mut run = AutoRun::new(1, Axis::Horizontal);
        run.anchor = Some((
            LongScrollTarget {
                hit: WindowToken(10),
                root: WindowToken(11),
                process: 12,
                thread: 13,
                point: Point { x: 10, y: 10 },
                dpi: 96,
            },
            Instant::now(),
        ));
        let wheel = Wheel(Mutex::default(), false);
        let notify = |_, _| panic!("unexpected scroll event");
        run.tick((&gate, &notify), Mode::Running, &wheel, bounds, true);
        assert!(wheel.0.lock().unwrap().is_empty());
        run.generation = 2;
        run.axis = Axis::Vertical;
        run.tick((&gate, &notify), Mode::Running, &wheel, bounds, true);
        assert!(wheel.0.lock().unwrap().is_empty());
        run.axis = Axis::Horizontal;
        run.tick((&gate, &notify), Mode::Running, &wheel, bounds, true);
        run.tick((&gate, &notify), Mode::Running, &wheel, bounds, true);
        assert_eq!(*wheel.0.lock().unwrap(), [Axis::Horizontal]);

        gate.lock().unwrap().state.mode = Mode::Manual;
        run.steps = Steps::default();
        run.tick((&gate, &notify), Mode::Running, &wheel, bounds, true);
        assert_eq!(*wheel.0.lock().unwrap(), [Axis::Horizontal]);

        gate.lock().unwrap().state.mode = Mode::Running;
        run.steps = Steps::default();
        let rejected = Wheel(Mutex::default(), true);
        let events = Mutex::new(Vec::new());
        let notify = |generation, reason| events.lock().unwrap().push((generation, reason));
        run.tick((&gate, &notify), Mode::Running, &rejected, bounds, true);
        run.tick((&gate, &notify), Mode::Running, &rejected, bounds, true);
        assert_eq!(*rejected.0.lock().unwrap(), [Axis::Horizontal]);
        assert_eq!(
            *events.lock().unwrap(),
            vec![(2, Some(Stop::InputUnavailable))]
        );
    }

    #[test]
    fn long_capture_pause_releases_resources_and_finishes_latest_accepted_pixels() {
        let port = capture();
        port.frames.lock().unwrap().insert(
            1,
            Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 3,
                regions: vec![frame(0)],
            }),
        );
        let cancel = AtomicBool::new(false);
        let paused = Arc::new(AtomicBool::new(false));
        let finish = AtomicBool::new(false);
        let watcher = port.clone();
        let resume = paused.clone();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while watcher.clears.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            resume.store(false, Ordering::Release);
        });
        let result = run(
            || Some(port.clone()),
            frame(0).bounds,
            vec![],
            (&cancel, &paused, &finish, &AtomicBool::new(false)),
            (None, None, None),
            || Ok(vec![frame(0).bounds]),
            |seq, _, feedback| {
                if feedback == Feedback::Unchanged {
                    return;
                }
                assert_eq!(feedback, Feedback::Accepted);
                if seq == 1 {
                    paused.store(true, Ordering::Release);
                } else {
                    finish.store(true, Ordering::Release);
                }
            },
        )
        .unwrap();
        worker.join().unwrap();
        assert_eq!(result.bounds.bottom, 100);
        let mut expected = frame(0).pixels;
        expected.extend_from_slice(&frame(20).pixels[60 * 64 * 4..]);
        assert_eq!(result.pixels, expected);
        assert_eq!(port.requests.load(Ordering::SeqCst), 2);
        assert!(port.clears.load(Ordering::SeqCst) >= 2);
    }
    #[test]
    fn compositor_transition_can_recover_but_stable_unknown_invalidates_position() {
        let port = capture();
        let mut invalid = frame(0);
        invalid.pixels.fill(0);
        *port.frames.lock().unwrap() = [frame(20), invalid.clone(), frame(0), invalid]
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                Arc::new(ScreenCaptureFrame {
                    revision: 1,
                    serial: index as u64 + 1,
                    regions: vec![image],
                })
            })
            .collect();
        let cancel = AtomicBool::new(false);
        let finish = AtomicBool::new(false);
        let (done, deadline) = std::sync::mpsc::channel();
        let image = std::thread::scope(|scope| {
            let cancelled = &cancel;
            scope.spawn(move || {
                if deadline.recv_timeout(Duration::from_secs(3)).is_err() {
                    cancelled.store(true, Ordering::Release);
                }
            });
            let result = run(
                || Some(port),
                frame(0).bounds,
                vec![],
                (
                    &cancel,
                    &AtomicBool::new(false),
                    &finish,
                    &AtomicBool::new(false),
                ),
                (None, None, None),
                || Ok(vec![frame(0).bounds]),
                |sequence, stitch, feedback| match sequence {
                    1 => assert_eq!(feedback, Feedback::Accepted),
                    2 => {
                        assert_eq!(feedback, Feedback::Accepted);
                        assert_eq!(stitch.position(), Some(0));
                        assert_eq!(stitch.size(), (64, 100));
                    }
                    3 => {
                        assert_eq!(feedback, Feedback::Uncertain);
                        assert_eq!(stitch.position(), None);
                        finish.store(true, Ordering::Release);
                    }
                    _ => unreachable!(),
                },
            );
            let _ = done.send(());
            result
        })
        .unwrap();
        let mut expected = frame(0).pixels;
        expected.extend_from_slice(&frame(20).pixels[60 * 64 * 4..]);
        assert_eq!(image.pixels, expected);
    }

    #[test]
    fn long_capture_cancel_drops_result_and_clears_owned_capture() {
        let port = capture();
        let cancel = AtomicBool::new(false);
        let paused = AtomicBool::new(false);
        let finish = AtomicBool::new(false);
        assert!(
            run(
                || Some(port.clone()),
                frame(0).bounds,
                vec![],
                (&cancel, &paused, &finish, &AtomicBool::new(false)),
                (None, None, None),
                || Ok(vec![frame(0).bounds]),
                |_, _, _| cancel.store(true, Ordering::Release)
            )
            .is_err()
        );
        assert_eq!(port.clears.load(Ordering::SeqCst), 1);
        assert_eq!(port.frames.lock().unwrap().len(), 1);
    }

    #[test]
    fn long_capture_worker_rejects_stale_trim_and_exports_confirmed_rows() {
        let port = capture();
        let cancel = AtomicBool::new(false);
        let paused = AtomicBool::new(false);
        let finish = AtomicBool::new(false);
        let queue = TrimQueue::default();
        let settled = AtomicUsize::new(0);
        let result = run(
            || Some(port.clone()),
            frame(0).bounds,
            vec![],
            (&cancel, &paused, &finish, &AtomicBool::new(false)),
            (
                None,
                Some((&queue, &|| {
                    if settled.fetch_add(1, Ordering::Relaxed) == 0 {
                        *queue.lock().unwrap() = Some((2, Some(Trim::Above)));
                    }
                })),
                None,
            ),
            || Ok(vec![frame(0).bounds]),
            |sequence, stitch, _| match sequence {
                1 => *queue.lock().unwrap() = Some((0, Some(Trim::Above))),
                2 => assert_eq!(stitch.size(), (64, 100)),
                3 => finish.store(true, Ordering::Release),
                _ => unreachable!(),
            },
        )
        .unwrap();
        assert_eq!(settled.load(Ordering::Relaxed), 2);
        assert_eq!(*result, frame(20));
    }
}
