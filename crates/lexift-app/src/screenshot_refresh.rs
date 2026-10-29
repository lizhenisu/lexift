//! Candidate pixels stay private until the complete editor has acknowledged them.
use super::*;
use lexift_core::domain::screenshot::refresh::{Command, Event};

pub(super) struct Job {
    session: u64,
    operation: u64,
    cancelled: Arc<AtomicBool>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
pub(super) fn sync(
    r: &ScreenshotRuntime,
    state: &lexift_core::domain::screenshot::ScreenshotState,
    quitting: bool,
) {
    let mut slot = r.refresh.lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().is_some_and(|j| {
        quitting
            || j.session != state.session
            || state
                .refresh
                .as_ref()
                .is_none_or(|s| s.operation != j.operation)
    }) {
        slot.take();
    }
}
fn send(c: &Arc<AppController>, event: Event) {
    c.dispatch(AppEvent::Screenshot(ScreenshotEvent::Refresh(event)));
}
pub(super) fn execute(r: &Arc<ScreenshotRuntime>, c: &Arc<AppController>, command: Command) {
    // Dispatch can race cancellation on another thread. Commands carry identity,
    // but must still belong to the Core phase that produced them before touching resources.
    let (session, operation, expected) = match &command {
        Command::Suspend { session, operation } => (
            *session,
            *operation,
            Some(lexift_core::domain::screenshot::refresh::Stage::Suspending),
        ),
        Command::Capture {
            session, operation, ..
        } => (
            *session,
            *operation,
            Some(lexift_core::domain::screenshot::refresh::Stage::Capturing),
        ),
        Command::Install { session, operation } => (
            *session,
            *operation,
            Some(lexift_core::domain::screenshot::refresh::Stage::Presenting),
        ),
        Command::Commit { session, operation } => (
            *session,
            *operation,
            Some(lexift_core::domain::screenshot::refresh::Stage::Committing),
        ),
        Command::Rollback { session, operation } => (
            *session,
            *operation,
            Some(lexift_core::domain::screenshot::refresh::Stage::RollingBack),
        ),
        Command::Finish {
            session, operation, ..
        } => (*session, *operation, None),
    };
    let current = {
        let state = c.state.lock().unwrap_or_else(|p| p.into_inner());
        let state = &state.screenshot;
        state.session == session
            && state.operation == operation
            && match expected {
                Some(stage) => state
                    .refresh
                    .as_ref()
                    .is_some_and(|s| s.operation == operation && s.stage == stage),
                None => {
                    state.phase == lexift_core::domain::screenshot::ScreenshotPhase::Editing
                        && state.refresh.is_none()
                }
            }
    };
    if !current {
        return;
    }
    match command {
        Command::Suspend { session, operation } => {
            *r.refresh.lock().unwrap() = Some(Job {
                session,
                operation,
                cancelled: Arc::new(AtomicBool::new(false)),
                frames: None,
            });
            let c = c.clone();
            c.ui.suspend_screenshot_refresh(
                session,
                operation,
                Box::new({
                    let c = c.clone();
                    move |result| {
                        send(
                            &c,
                            Event::Suspended {
                                session,
                                operation,
                                result,
                            },
                        )
                    }
                }),
            );
        }
        Command::Capture {
            session,
            operation,
            request,
        } => {
            let source = r
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|s| s.id == session)
                .map(|s| s.source.clone());
            let cancel = r
                .refresh
                .lock()
                .unwrap()
                .as_ref()
                .filter(|j| j.session == session && j.operation == operation)
                .map(|j| j.cancelled.clone());
            let (Some(source), Some(cancel)) = (source, cancel) else {
                return;
            };
            let (r, c) = (r.clone(), c.clone());
            let runtime = c.runtime.clone();
            runtime.spawn_blocking(move || {
                let started = std::time::Instant::now();
                let result = prepare(
                    &r,
                    &source,
                    &request,
                    &cancel,
                    lexift_platform::screenshot_display_metrics,
                );
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                let result = result.and_then(|frames| {
                    let mut slot = r.refresh.lock().unwrap();
                    let j = slot
                        .as_mut()
                        .filter(|j| {
                            j.session == session
                                && j.operation == operation
                                && !j.cancelled.load(Ordering::Acquire)
                        })
                        .ok_or_else(|| Error::new("Screenshot refresh cancelled"))?;
                    j.frames = Some(Arc::new(frames));
                    Ok(())
                });
                tracing::debug!(
                    session,
                    operation,
                    elapsed_ms = started.elapsed().as_millis(),
                    success = result.is_ok(),
                    "Screenshot refresh candidate captured"
                );
                send(
                    &c,
                    Event::Captured {
                        session,
                        operation,
                        result,
                    },
                );
            });
        }
        Command::Install { session, operation } => {
            let frames = r
                .refresh
                .lock()
                .unwrap()
                .as_ref()
                .filter(|j| j.session == session && j.operation == operation)
                .and_then(|j| j.frames.clone());
            if let Some(frames) = frames {
                c.ui.present_screenshot_refresh(session, operation, frames, false);
            } else {
                send(
                    c,
                    Event::Presented {
                        session,
                        operation,
                        result: Err(Error::new("Screenshot refresh failed")),
                    },
                );
            }
        }
        Command::Commit { session, operation } => {
            let frames = r
                .refresh
                .lock()
                .unwrap()
                .as_ref()
                .filter(|j| j.session == session && j.operation == operation)
                .and_then(|j| j.frames.clone());
            let result = frames
                .ok_or_else(|| Error::new("Screenshot refresh failed"))
                .and_then(|frames| {
                    let mut slot = r.session.lock().unwrap();
                    let s = slot
                        .as_mut()
                        .filter(|s| s.id == session && !s.cancelled.load(Ordering::Acquire))
                        .ok_or_else(|| Error::new("Screenshot refresh cancelled"))?;
                    s.frames = Some(frames);
                    Ok(())
                });
            send(
                c,
                Event::Committed {
                    session,
                    operation,
                    result,
                },
            );
        }
        Command::Rollback { session, operation } => {
            {
                let mut slot = r.refresh.lock().unwrap();
                if slot
                    .as_ref()
                    .is_some_and(|j| j.session == session && j.operation == operation)
                {
                    slot.take();
                }
            }
            let frames = r
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|s| s.id == session)
                .and_then(|s| s.frames.clone());
            if let Some(frames) = frames {
                c.ui.present_screenshot_refresh(session, operation, frames, true);
            }
        }
        Command::Finish {
            session,
            operation,
            success,
        } => c.ui.finish_screenshot_refresh(session, operation, success),
    }
}
fn prepare(
    r: &ScreenshotRuntime,
    source: &ExportRequest,
    request: &ExportRequest,
    cancel: &AtomicBool,
    metrics: impl Fn() -> Result<Vec<(lexift_core::domain::geometry::Rect, f32)>>,
) -> Result<Vec<CapturedRegion>> {
    use lexift_core::domain::screenshot::refresh::layout_matches;
    if cancel.load(Ordering::Acquire) {
        return Err(Error::new("Screenshot refresh cancelled"));
    }
    if !layout_matches(&source.snapshot.monitors, &request.snapshot.monitors)
        || !layout_matches(&request.snapshot.monitors, &metrics()?)
    {
        return Err(Error::new(
            "Display layout changed. Start a new screenshot.",
        ));
    }
    let pixels = request
        .snapshot
        .monitors
        .iter()
        .try_fold(0u64, |sum, (b, _)| {
            let count = u64::try_from(i64::from(b.right) - i64::from(b.left))
                .ok()?
                .checked_mul(u64::try_from(i64::from(b.bottom) - i64::from(b.top)).ok()?)?;
            sum.checked_add(count)
        })
        .ok_or_else(|| Error::new("Screenshot refresh failed"))?;
    // Old pixels, candidate, software caches, native upload and rollback feedback
    // can overlap. Reserve fourteen RGBA surfaces, not just the final image.
    if pixels > lexift_core::domain::image_resize::MAX_PIXELS
        || pixels.saturating_mul(56) > lexift_core::domain::long_capture::MAX_BUFFER_BYTES as u64
    {
        return Err(Error::new("Image exceeds the processing memory limit"));
    }
    let port = (r.capture)().ok_or_else(|| Error::new("Screen capture unavailable"))?;
    let capture_started = std::time::Instant::now();
    let (captured, fast_path) = crate::annotation_export::capture_frozen_pixels(
        port,
        request.snapshot.monitors.iter().map(|(b, _)| *b).collect(),
        request.snapshot.excluded.clone(),
        cancel,
    )?;
    tracing::debug!(
        session = request.session,
        operation = request.request,
        fast_path,
        pixels,
        reserved_bytes = pixels.saturating_mul(56),
        elapsed_ms = capture_started.elapsed().as_millis(),
        "Screenshot refresh fresh pixels acquired"
    );
    let captured = captured.ok_or_else(|| Error::new("Screenshot refresh cancelled"))?;
    if cancel.load(Ordering::Acquire) {
        return Err(Error::new("Screenshot refresh cancelled"));
    }
    if !layout_matches(&request.snapshot.monitors, &metrics()?) {
        return Err(Error::new(
            "Display layout changed. Start a new screenshot.",
        ));
    }
    let raw_layout = captured
        .regions
        .iter()
        .map(|f| f.bounds)
        .collect::<Vec<_>>();
    if request
        .snapshot
        .monitors
        .iter()
        .any(|(b, _)| !raw_layout.contains(b))
    {
        return Err(Error::new("Screenshot refresh failed"));
    }
    let composition_started = std::time::Instant::now();
    let result = request
        .snapshot
        .monitors
        .iter()
        .map(|(bounds, scale)| {
            let mut snapshot = (*source.snapshot).clone();
            snapshot.monitor = *bounds;
            snapshot.scale = *scale;
            r.render.compose(&snapshot, captured.clone(), cancel)
        })
        .collect();
    tracing::debug!(
        session = request.session,
        operation = request.request,
        elapsed_ms = composition_started.elapsed().as_millis(),
        "Screenshot refresh original desktop annotation composed"
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::{
            annotation::Session as Document, annotation_export::ExportSnapshot, geometry::Rect,
        },
        ports::{
            annotation_export::AnnotationExportRenderPort, clipboard::ClipboardPort,
            screen_capture::ScreenCaptureRequest,
        },
    };
    struct Capture;
    fn bounds() -> Rect {
        Rect {
            left: -3,
            top: 0,
            right: 3,
            bottom: 2,
        }
    }
    fn image() -> CapturedRegion {
        CapturedRegion {
            bounds: bounds(),
            pixels: [20, 40, 90, 255].repeat(12),
        }
    }
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
            panic!("fast path expected")
        }
        fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
            panic!("no polling on fast path")
        }
        fn clear(&self) {}
        fn screenshot_windows(
            &self,
            _: &[lexift_core::ports::magnifier::WindowToken],
        ) -> Result<Vec<lexift_core::domain::window_candidate::WindowCandidate>> {
            panic!("refresh never queries window metadata")
        }
        fn try_freeze_screenshot(
            &self,
            _: &ScreenCaptureRequest,
            _: &AtomicBool,
        ) -> Result<Option<Arc<ScreenCaptureFrame>>> {
            Ok(Some(Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: vec![image()],
            })))
        }
    }
    struct Render;
    impl AnnotationExportRenderPort for Render {
        fn regions(&self, s: &ExportSnapshot) -> Vec<Rect> {
            vec![s.monitor]
        }
        fn compose(
            &self,
            s: &ExportSnapshot,
            c: Arc<ScreenCaptureFrame>,
            cancel: &AtomicBool,
        ) -> Result<CapturedRegion> {
            assert!(!cancel.load(Ordering::Acquire));
            assert_eq!(s.session.spotlight_opacity, 0.37);
            let mut i = c.regions[0].clone();
            i.pixels[..4].copy_from_slice(&[190, 20, 30, 255]);
            Ok(i)
        }
    }
    struct Save;
    impl ImageFilePort for Save {
        fn is_available(&self) -> bool {
            true
        }
        fn save_image(
            &self,
            _: lexift_core::ports::annotation_save::ImageSaveRequest,
            _: Arc<AtomicBool>,
        ) -> Result<SaveOutcome> {
            panic!("refresh must not save")
        }
    }
    struct Clipboard;
    impl ClipboardPort for Clipboard {
        fn read_text(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn write_text(&self, _: &str) -> Result<()> {
            panic!("refresh does not write clipboard")
        }
    }
    fn request(opacity: f32) -> ExportRequest {
        let mut s = Document::new();
        s.spotlight_opacity = opacity;
        ExportRequest {
            session: 1,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(s),
                monitor: bounds(),
                monitors: vec![(bounds(), 1.25)],
                scale: 1.25,
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        }
    }
    #[test]
    fn refresh_composes_initial_desktop_snapshot_once_and_ignores_current_annotation_snapshot() {
        let r = ScreenshotRuntime::new(
            || Some(Arc::new(Capture)),
            Arc::new(Save),
            Arc::new(Render),
            Arc::new(Clipboard),
        );
        let frames = prepare(
            &r,
            &request(0.37),
            &request(0.9),
            &AtomicBool::new(false),
            || Ok(vec![(bounds(), 1.25)]),
        )
        .unwrap();
        assert_eq!(frames[0].bounds, bounds());
        assert_eq!(&frames[0].pixels[..4], &[190, 20, 30, 255]);
        assert_eq!(&frames[0].pixels[4..], &image().pixels[4..]);
        let calls = std::cell::Cell::new(0);
        assert!(
            prepare(
                &r,
                &request(0.37),
                &request(0.9),
                &AtomicBool::new(false),
                || {
                    calls.set(calls.get() + 1);
                    Ok(vec![(bounds(), if calls.get() == 1 { 1.25 } else { 1. })])
                }
            )
            .is_err()
        );
        assert_eq!(
            calls.get(),
            2,
            "layout/DPI is rechecked after fresh capture"
        );
        let mut huge = request(0.37);
        let snapshot = Arc::make_mut(&mut huge.snapshot);
        snapshot.monitor = Rect {
            left: 0,
            top: 0,
            right: 10000,
            bottom: 10000,
        };
        snapshot.monitors = vec![(snapshot.monitor, 1.25)];
        assert!(
            prepare(&r, &huge, &huge, &AtomicBool::new(false), || Ok(huge
                .snapshot
                .monitors
                .clone()))
            .unwrap_err()
            .to_string()
            .contains("memory limit")
        );
        let layout = || Ok(vec![(bounds(), 1.)]);
        assert!(
            prepare(
                &r,
                &request(0.37),
                &request(0.9),
                &AtomicBool::new(false),
                layout
            )
            .is_err()
        );
        assert!(
            prepare(
                &r,
                &request(0.37),
                &request(0.9),
                &AtomicBool::new(true),
                || panic!("cancelled before metrics or capture")
            )
            .is_err()
        );
    }
}
