//! Executes Core export commands with independently owned capture and cancellation.
use crate::controller::AppController;
use lexift_core::{
    AppEvent, Error, Result,
    domain::{
        annotation_export::{ExportCommand, ExportEvent, ExportRequest, ExportState},
        geometry::Rect,
    },
    ports::{
        annotation_export::AnnotationExportRenderPort,
        annotation_save::{AnnotationSavePort, PngSaveRequest, SaveOutcome},
        magnifier::WindowToken,
        screen_capture::{ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest},
    },
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
type CaptureFactory = fn() -> Option<Arc<dyn ScreenCapturePort>>;
pub(crate) struct ExportRuntime {
    capture: CaptureFactory,
    save: Arc<dyn AnnotationSavePort>,
    render: Arc<dyn AnnotationExportRenderPort>,
    job: Mutex<Option<(u64, u64, Arc<AtomicBool>)>>,
}
impl ExportRuntime {
    pub(crate) fn new(
        capture: CaptureFactory,
        save: Arc<dyn AnnotationSavePort>,
        render: Arc<dyn AnnotationExportRenderPort>,
    ) -> Self {
        Self {
            capture,
            save,
            render,
            job: Mutex::new(None),
        }
    }
    pub(crate) fn sync(&self, state: &ExportState, quitting: bool) {
        let mut job = self.job.lock().unwrap_or_else(|p| p.into_inner());
        if job.as_ref().is_some_and(|(s, r, _)| {
            quitting || state.session != Some(*s) || state.request != Some(*r)
        }) && let Some((_, _, cancelled)) = job.take()
        {
            cancelled.store(true, Ordering::Release);
        }
    }
    pub(crate) fn execute(
        self: &Arc<Self>,
        controller: &Arc<AppController>,
        command: ExportCommand,
    ) {
        let ExportCommand::Run(request) = command else {
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let state = controller.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.annotation_export.session != Some(request.session)
                || state.annotation_export.request != Some(request.request)
            {
                return;
            }
            let mut job = self.job.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((_, _, old)) =
                job.replace((request.session, request.request, cancelled.clone()))
            {
                old.store(true, Ordering::Release);
            }
        }
        let runtime = self.clone();
        let controller = controller.clone();
        let session = request.session;
        let request_id = request.request;
        let (tx, rx) = tokio::sync::oneshot::channel();
        // A dedicated thread releases font/texture thread-local caches after each export.
        let spawned = std::thread::Builder::new()
            .name("annotation-export".into())
            .spawn(move || {
                let _ = tx.send(runtime.run(&request, cancelled));
            });
        if let Err(error) = spawned {
            controller.dispatch(AppEvent::AnnotationExport(ExportEvent::Completed {
                session,
                request: request_id,
                result: Err(Error::new(format!(
                    "Could not start annotation export: {error}"
                ))),
            }));
            return;
        }
        controller.clone().runtime.spawn(async move {
            let result = rx.await.unwrap_or_else(|error| {
                Err(Error::new(format!(
                    "Annotation export worker stopped: {error}"
                )))
            });
            controller.dispatch(AppEvent::AnnotationExport(ExportEvent::Completed {
                session,
                request: request_id,
                result,
            }));
        });
    }
    fn run(&self, request: &ExportRequest, cancelled: Arc<AtomicBool>) -> Result<SaveOutcome> {
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        let snapshot = &request.snapshot;
        let Some(frame) = capture(
            self.capture,
            self.render.regions(snapshot),
            snapshot.excluded.clone(),
            &cancelled,
        )?
        else {
            return Ok(SaveOutcome::Cancelled);
        };
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        let image = self.render.compose(snapshot, frame, &cancelled)?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        self.save.save_png(
            PngSaveRequest {
                image,
                owner: snapshot.owner,
                default_name: snapshot.name.clone(),
                title: snapshot.title.clone(),
            },
            cancelled,
        )
    }
}
impl Drop for ExportRuntime {
    fn drop(&mut self) {
        if let Some((_, _, cancelled)) =
            self.job.get_mut().unwrap_or_else(|p| p.into_inner()).take()
        {
            cancelled.store(true, Ordering::Release);
        }
    }
}
fn contains(a: Rect, b: Rect) -> bool {
    a.left <= b.left && a.top <= b.top && a.right >= b.right && a.bottom >= b.bottom
}
/// The new instance cannot publish a prior request's cache. Unchanged pixels
/// still produce its first frame; no serial-increment test is needed.
pub(crate) fn capture(
    factory: CaptureFactory,
    regions: Vec<Rect>,
    excluded_windows: Vec<WindowToken>,
    cancelled: &AtomicBool,
) -> Result<Option<Arc<ScreenCaptureFrame>>> {
    capture_with_timeout(
        factory,
        regions,
        excluded_windows,
        cancelled,
        Duration::from_secs(5),
    )
}

fn capture_with_timeout(
    factory: CaptureFactory,
    regions: Vec<Rect>,
    excluded_windows: Vec<WindowToken>,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Option<Arc<ScreenCaptureFrame>>> {
    let deadline = Instant::now() + timeout;
    let port = factory().ok_or_else(|| Error::new("Screen capture unavailable"))?;
    if !port.is_available() {
        return Err(Error::new("Screen capture unavailable"));
    }
    let request = ScreenCaptureRequest {
        revision: 1,
        regions,
        paused_regions: Vec::new(),
        excluded_windows,
    };
    port.request(request.clone())?;
    let mut last_error = None;
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Ok(None);
        }
        match port.latest() {
            Ok(Some(frame))
                if frame.revision == request.revision
                    && request.regions.iter().all(|r| {
                        frame
                            .regions
                            .iter()
                            .any(|source| contains(source.bounds, *r))
                    }) =>
            {
                break Ok(Some(frame));
            }
            Err(error) => last_error = Some(error),
            _ => {}
        }
        if Instant::now() >= deadline {
            break Err(last_error.unwrap_or_else(|| Error::new("Screen capture timed out")));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    port.clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::{annotation::Session, annotation_export::ExportSnapshot as Snapshot},
        ports::screen_capture::CapturedRegion,
    };
    struct Render;
    impl AnnotationExportRenderPort for Render {
        fn regions(&self, _: &Snapshot) -> Vec<Rect> {
            panic!("cancelled export prepared rendering")
        }
        fn compose(
            &self,
            _: &Snapshot,
            _: Arc<ScreenCaptureFrame>,
            _: &AtomicBool,
        ) -> Result<CapturedRegion> {
            panic!("cancelled export composed image")
        }
    }
    fn fixture() -> Snapshot {
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        Snapshot {
            session: Arc::new(Session::new()),
            monitor,
            scale: 1.,
            monitors: vec![(monitor, 1.)],
            excluded: vec![WindowToken(100)],
            owner: None,
            name: "test.png".into(),
            title: String::new(),
        }
    }
    fn frame(bounds: Rect, color: [u8; 4]) -> Arc<ScreenCaptureFrame> {
        Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds,
                pixels: color
                    .repeat(((bounds.right - bounds.left) * (bounds.bottom - bounds.top)) as usize),
            }],
        })
    }
    #[test]
    fn cancelled_export_never_opens_save_dialog() {
        struct Save;
        impl AnnotationSavePort for Save {
            fn is_available(&self) -> bool {
                true
            }
            fn save_png(&self, _: PngSaveRequest, _: Arc<AtomicBool>) -> Result<SaveOutcome> {
                panic!("cancelled job opened dialog")
            }
        }
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                panic!("cancelled job read frame")
            }
            fn clear(&self) {}
        }
        let runtime =
            ExportRuntime::new(|| Some(Arc::new(Capture)), Arc::new(Save), Arc::new(Render));
        let result = runtime
            .run(
                &ExportRequest {
                    session: 1,
                    request: 1,
                    snapshot: Arc::new(fixture()),
                },
                Arc::new(AtomicBool::new(true)),
            )
            .unwrap();
        assert_eq!(result, SaveOutcome::Cancelled);
    }
    #[test]
    fn single_capture_waits_for_request_and_all_sources_not_a_serial_change() {
        use std::sync::atomic::AtomicUsize;
        struct Capture {
            calls: AtomicUsize,
        }
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, request: ScreenCaptureRequest) -> Result<()> {
                assert_eq!(request.regions, vec![fixture().monitor]);
                assert_eq!(request.excluded_windows, vec![WindowToken(100)]);
                assert!(request.paused_regions.is_empty());
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                let calls = self.calls.fetch_add(1, Ordering::Relaxed);
                let mut result = ScreenCaptureFrame {
                    revision: 1,
                    serial: 41,
                    regions: frame(fixture().monitor, [20, 30, 40, 255]).regions.clone(),
                };
                if calls == 0 {
                    result.revision = 0;
                }
                if calls == 1 {
                    result.regions.clear();
                }
                Ok(Some(Arc::new(result)))
            }
            fn clear(&self) {
                assert_eq!(self.calls.load(Ordering::Relaxed), 3);
            }
        }
        let frame = capture(
            || {
                Some(Arc::new(Capture {
                    calls: AtomicUsize::new(0),
                }))
            },
            vec![fixture().monitor],
            vec![WindowToken(100)],
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            frame.serial, 41,
            "unchanged serial must not stall a new independent capture"
        );
    }

    #[test]
    fn capture_timeout_releases_the_job_without_opening_a_dialog() {
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        let result = capture_with_timeout(
            || Some(Arc::new(Capture)),
            vec![fixture().monitor],
            Vec::new(),
            &AtomicBool::new(false),
            Duration::from_millis(1),
        );
        assert!(result.is_err_and(|e| e.to_string().contains("timed out")));
    }
    struct FixedCapture;
    impl ScreenCapturePort for FixedCapture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
            Ok(())
        }
        fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
            Ok(Some(frame(fixture().monitor, [1, 2, 3, 255])))
        }
        fn clear(&self) {}
    }
    struct Compose {
        fail: bool,
        cancel: bool,
    }
    impl AnnotationExportRenderPort for Compose {
        fn regions(&self, snapshot: &Snapshot) -> Vec<Rect> {
            vec![snapshot.monitor]
        }
        fn compose(
            &self,
            _: &Snapshot,
            frame: Arc<ScreenCaptureFrame>,
            cancelled: &AtomicBool,
        ) -> Result<CapturedRegion> {
            if self.fail {
                return Err(Error::new("compose failed"));
            }
            if self.cancel {
                cancelled.store(true, Ordering::Release);
            }
            Ok(frame.regions[0].clone())
        }
    }
    struct CountingSave {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }
    impl AnnotationSavePort for CountingSave {
        fn is_available(&self) -> bool {
            true
        }
        fn save_png(&self, request: PngSaveRequest, _: Arc<AtomicBool>) -> Result<SaveOutcome> {
            request.validate()?;
            assert_eq!(request.default_name, "test.png");
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                Err(Error::new("save failed"))
            } else {
                Ok(SaveOutcome::Saved)
            }
        }
    }
    #[test]
    fn compose_failure_and_cancel_do_not_save_and_save_failure_is_returned() {
        for (compose_fail, cancel, save_fail, count) in [
            (true, false, false, 0),
            (false, true, false, 0),
            (false, false, true, 1),
            (false, false, false, 1),
        ] {
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let runtime = ExportRuntime::new(
                || Some(Arc::new(FixedCapture)),
                Arc::new(CountingSave {
                    calls: calls.clone(),
                    fail: save_fail,
                }),
                Arc::new(Compose {
                    fail: compose_fail,
                    cancel,
                }),
            );
            let result = runtime.run(
                &ExportRequest {
                    session: 1,
                    request: 1,
                    snapshot: Arc::new(fixture()),
                },
                Arc::new(AtomicBool::new(false)),
            );
            assert_eq!(calls.load(Ordering::Relaxed), count);
            if compose_fail || save_fail {
                assert!(result.is_err());
            } else {
                assert_eq!(
                    result.unwrap(),
                    if cancel {
                        SaveOutcome::Cancelled
                    } else {
                        SaveOutcome::Saved
                    }
                );
            }
        }
    }
    #[test]
    fn sync_cancels_retired_job_without_cancelling_current_request() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runtime = ExportRuntime::new(
            || None,
            Arc::new(CountingSave { calls, fail: false }),
            Arc::new(Render),
        );
        let old = Arc::new(AtomicBool::new(false));
        *runtime.job.lock().unwrap() = Some((1, 1, old.clone()));
        let current = ExportState {
            session: Some(2),
            request: Some(2),
            ..Default::default()
        };
        runtime.sync(&current, false);
        assert!(old.load(Ordering::Acquire));
        let active = Arc::new(AtomicBool::new(false));
        *runtime.job.lock().unwrap() = Some((2, 2, active.clone()));
        runtime.sync(&current, false);
        assert!(!active.load(Ordering::Acquire));
        runtime.sync(&current, true);
        assert!(active.load(Ordering::Acquire));
    }
}
