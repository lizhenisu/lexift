//! Sequential background orchestration; matching and acceptance policy belong to Core.
use super::*;
use lexift_core::domain::long_capture::{
    Feedback, Stitcher,
    workflow::{Command, Event, Phase},
};
use std::time::{Duration, Instant};

pub(super) struct Job {
    session: u64,
    operation: u64,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    finish: Arc<AtomicBool>,
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
        j.cancel.store(true, Ordering::Release);
    }
}
fn emit(c: &Arc<AppController>, event: Event) {
    c.dispatch(AppEvent::Screenshot(ScreenshotEvent::Long(event)));
}

pub(super) fn execute(runtime: &Arc<ScreenshotRuntime>, c: &Arc<AppController>, command: Command) {
    match command {
        Command::Prepare { .. } | Command::Present { .. } => c.ui.long_screenshot(command),
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
            let finish = Arc::new(AtomicBool::new(false));
            if let Some(old) = runtime.long.lock().unwrap().replace(Job {
                session,
                operation,
                cancel: cancel.clone(),
                paused: paused.clone(),
                finish: finish.clone(),
            }) {
                old.cancel.store(true, Ordering::Release);
            }
            let factory = runtime.capture;
            let c = c.clone();
            c.clone().runtime.spawn_blocking(move || {
                let result = run(
                    factory,
                    bounds,
                    excluded,
                    (&cancel, &paused, &finish),
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
                                preview: stitch.preview(preview_size.0, preview_size.1),
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

fn run(
    factory: impl FnOnce() -> Option<Arc<dyn ScreenCapturePort>>,
    bounds: lexift_core::domain::geometry::Rect,
    excluded: Vec<lexift_core::ports::magnifier::WindowToken>,
    control: (&AtomicBool, &AtomicBool, &AtomicBool),
    layout: impl Fn() -> Result<Vec<lexift_core::domain::geometry::Rect>>,
    mut progress: impl FnMut(u64, &Stitcher, Feedback),
) -> Result<Arc<CapturedRegion>> {
    let (cancel, paused, finish) = control;
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
    let mut last_frame = Instant::now();
    struct Stats {
        start: Instant,
        frames: u64,
        accepted: u64,
        rejected: u64,
        matching: Duration,
        feedback: Duration,
        peak: usize,
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
        peak: 0,
    };
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("Long capture cancelled"));
        }
        if finish.load(Ordering::Acquire) {
            port.clear();
            return stitch
                .as_ref()
                .ok_or_else(|| Error::new("Long capture unavailable"))?
                .flatten(cancel)
                .map(Arc::new);
        }
        if paused.load(Ordering::Acquire) {
            if !suspended {
                port.clear();
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
        }
        if layout_checked.elapsed() >= Duration::from_millis(250) {
            layout_checked = Instant::now();
            if layout().map_or(true, |b| normalize(b) != original_layout) {
                if let Some(s) = &stitch {
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
                let first = stitch.is_none();
                let matching = Instant::now();
                stats.frames += 1;
                let feedback = if let Some(stitch) = &mut stitch {
                    stitch.push(image, cancel)?
                } else {
                    stitch = Some(Stitcher::new(image)?);
                    Feedback::Accepted
                };
                stats.matching += matching.elapsed();
                stats.accepted += u64::from(feedback == Feedback::Accepted);
                stats.rejected += u64::from(feedback == Feedback::Uncertain);
                stats.peak = stats.peak.max(stitch.as_ref().unwrap().retained_bytes());
                if first || feedback != last_feedback || feedback == Feedback::Accepted {
                    sequence += 1;
                    let feedback_started = Instant::now();
                    progress(sequence, stitch.as_ref().unwrap(), feedback);
                    stats.feedback += feedback_started.elapsed();
                }
                last_feedback = feedback;
                if feedback == Feedback::Limit {
                    paused.store(true, Ordering::Release);
                }
            }
            Err(error) if stitch.is_none() => return Err(error),
            Err(_) => {
                paused.store(true, Ordering::Release);
                sequence += 1;
                progress(sequence, stitch.as_ref().unwrap(), Feedback::CaptureLost);
            }
            _ if stitch.is_none() && last_frame.elapsed() > Duration::from_secs(3) => {
                return Err(Error::new("Long capture unavailable"));
            }
            _ => {}
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
            Ok(self.frames.lock().unwrap().pop_front())
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
    fn long_capture_pause_releases_resources_and_finishes_latest_accepted_pixels() {
        let port = capture();
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
            (&cancel, &paused, &finish),
            || Ok(vec![frame(0).bounds]),
            |seq, _, feedback| {
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
                (&cancel, &paused, &finish),
                || Ok(vec![frame(0).bounds]),
                |_, _, _| cancel.store(true, Ordering::Release)
            )
            .is_err()
        );
        assert_eq!(port.clears.load(Ordering::SeqCst), 1);
        assert_eq!(port.frames.lock().unwrap().len(), 1);
    }
}
