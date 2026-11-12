//! Single-slot handoff between the native capture owner and the Slint event loop.
use super::*;
use lexift_core::domain::pinned_image::live::{Command, Event, Failure};
use lexift_core::domain::screenshot::ScreenshotPhase;
use lexift_core::ports::screen_capture::WindowPinUpdate;

pub(crate) type Pull = Box<dyn FnOnce() -> Option<Arc<CapturedRegion>> + Send>;
#[derive(Default)]
struct Mailbox {
    queued: bool,
    latest: Option<Arc<CapturedRegion>>,
}
pub(super) struct Job {
    source: (u64, u64),
    stop: Arc<AtomicBool>,
}
pub(crate) fn sync(runtime: &ScreenshotRuntime, state: &lexift_core::AppState, quitting: bool) {
    if let Some(job) = runtime.live_job.lock().unwrap().as_ref()
        && (quitting
            || state.live_pin.is_none_or(|live| {
                live.source != job.source
                    || live.finished
                    || live.id.is_none()
                        && (state.screenshot.session != job.source.0
                            || state.screenshot.operation != job.source.1
                            || state.screenshot.phase != ScreenshotPhase::Output)
            }))
    {
        job.stop.store(true, Ordering::Release);
    }
}
fn emit(c: &Arc<AppController>, event: Event) {
    c.dispatch(AppEvent::PinnedImage(PinEvent::Live(event)));
}

pub(super) fn execute(runtime: &Arc<ScreenshotRuntime>, c: &Arc<AppController>, command: Command) {
    match command {
        Command::Stop { source } => {
            if let Some(job) = runtime
                .live_job
                .lock()
                .unwrap()
                .as_ref()
                .filter(|job| job.source == source)
            {
                job.stop.store(true, Ordering::Release);
            }
        }
        Command::Install { id, image } => {
            let bounds = image.bounds;
            let samples = Arc::new(ScreenCaptureFrame {
                serial: id,
                revision: 0,
                regions: vec![],
            });
            let document = history::retained_document(None, bounds);
            {
                let state = c.state.lock().unwrap();
                if !state.pinned_images.get(&id).is_some_and(|p| p.live)
                    || state
                        .live_pin
                        .is_none_or(|live| live.id != Some(id) || !live.finished)
                {
                    return;
                }
                runtime.pin_images.lock().unwrap().insert(id, image.clone());
                runtime
                    .pin_samples
                    .lock()
                    .unwrap()
                    .insert(id, samples.clone());
            }
            emit(c, Event::Installed { id, bounds });
            c.ui.window_pin_installed(
                id,
                lexift_core::domain::image_history::ImageSnapshot {
                    base: image,
                    samples,
                    document,
                    hidden: false,
                },
            );
        }
        Command::Start { source, selection } => {
            let prepared = runtime
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|session| session.id == source.0)
                .and_then(|session| {
                    let target = lexift_core::ports::screen_capture::window_pin_candidate(
                        &session.windows,
                        selection,
                    )?;
                    Some((target, session.element_port.clone()))
                });
            let Some((target, port)) = prepared else {
                emit(
                    c,
                    Event::Failed {
                        source,
                        reason: Failure::Selection,
                    },
                );
                emit(c, Event::Retired { source });
                return;
            };
            let stop = Arc::new(AtomicBool::new(false));
            {
                let state = c.state.lock().unwrap();
                if state.live_pin.is_none_or(|live| live.source != source)
                    || state.screenshot.phase != ScreenshotPhase::Output
                {
                    drop(state);
                    emit(
                        c,
                        Event::Failed {
                            source,
                            reason: Failure::Unavailable,
                        },
                    );
                    emit(c, Event::Retired { source });
                    return;
                }
                *runtime.live_job.lock().unwrap() = Some(Job {
                    source,
                    stop: stop.clone(),
                });
            }
            let owner = runtime.clone();
            let c = c.clone();
            c.clone().runtime.spawn_blocking(move || {
                let port = port.or_else(|| (owner.capture)());
                let inbox = Arc::new(Mutex::new(Mailbox::default()));
                let started = std::time::Instant::now();
                let mut failure = Failure::Unavailable;
                if let Some(port) = port {
                    let mut paused = false;
                    let result = port.run_window_pin(target, &stop, &mut |update| {
                        let frame = match update {
                            WindowPinUpdate::Pause(reason) => {
                                paused = true;
                                inbox.lock().unwrap().latest = None;
                                emit(
                                    &c,
                                    Event::Pause {
                                        source,
                                        reason: Some(reason),
                                    },
                                );
                                return;
                            }
                            WindowPinUpdate::Frame(frame) => {
                                if std::mem::take(&mut paused) {
                                    emit(
                                        &c,
                                        Event::Pause {
                                            source,
                                            reason: None,
                                        },
                                    );
                                }
                                frame
                            }
                        };
                        let mut state = inbox.lock().unwrap();
                        state.latest = Some(Arc::new(frame));
                        if state.queued {
                            return;
                        }
                        state.queued = true;
                        drop(state);
                        let inbox = inbox.clone();
                        let stop_pull = stop.clone();
                        if !c.ui.window_pin_frame(
                            source,
                            Box::new(move || {
                                let mut state = inbox.lock().unwrap();
                                state.queued = false;
                                let image = state.latest.take();
                                if stop_pull.load(Ordering::Acquire) {
                                    None
                                } else {
                                    image
                                }
                            }),
                        ) {
                            stop.store(true, Ordering::Release);
                        }
                    });
                    failure = result.err().unwrap_or_default();
                    tracing::debug!(
                        ?source,
                        elapsed_ms = started.elapsed().as_millis(),
                        success = result.is_ok(),
                        "Window pin worker retired"
                    );
                }
                {
                    let mut job = owner.live_job.lock().unwrap();
                    if job.as_ref().is_some_and(|job| job.source == source) {
                        job.take();
                    }
                }
                if !c.ui.window_pin_ended(source, failure) {
                    emit(
                        &c,
                        Event::Failed {
                            source,
                            reason: failure,
                        },
                    );
                }
                emit(&c, Event::Retired { source });
            });
        }
    }
}
