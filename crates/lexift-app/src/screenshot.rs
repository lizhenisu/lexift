//! Background capture/output of frozen screenshot sessions. No business transitions here.
use crate::controller::AppController;
use lexift_core::domain::pinned_image::{
    Command as PinCommand, Event as PinEvent, Output as PinOutput, States as PinStates,
};
use lexift_core::{
    AppEvent, Error, Result,
    domain::{
        annotation_export::ExportRequest,
        screen_snapshot::crop_frames,
        screenshot::{ScreenshotCommand, ScreenshotEvent, ScreenshotOutput},
    },
    ports::{
        annotation_export::AnnotationExportRenderPort,
        annotation_save::{AnnotationSavePort, PngSaveRequest, SaveOutcome},
        clipboard::ClipboardPort,
        screen_capture::{CapturedRegion, ScreenCaptureFrame, ScreenCapturePort},
    },
};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
type CaptureFactory = fn() -> Option<Arc<dyn ScreenCapturePort>>;
struct PreparedScreenshot {
    frames: Vec<CapturedRegion>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
}
struct Session {
    id: u64,
    cancelled: Arc<AtomicBool>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
}
pub(crate) struct ScreenshotRuntime {
    capture: CaptureFactory,
    save: Arc<dyn AnnotationSavePort>,
    render: Arc<dyn AnnotationExportRenderPort>,
    clipboard: Arc<dyn ClipboardPort>,
    session: Mutex<Option<Session>>,
    color_job: Mutex<Option<(u64, u64, Arc<AtomicBool>)>>,
    color_serial: tokio::sync::Mutex<()>,
    pin_pending: Mutex<BTreeMap<(u64, u64), Arc<CapturedRegion>>>,
    pin_images: Mutex<BTreeMap<u64, Arc<CapturedRegion>>>,
    pin_jobs: Mutex<BTreeMap<u64, Arc<AtomicBool>>>,
    pin_pending_samples: Mutex<BTreeMap<(u64, u64), Arc<ScreenCaptureFrame>>>,
    pin_samples: Mutex<BTreeMap<u64, Arc<ScreenCaptureFrame>>>,
    pin_previews: Mutex<BTreeMap<u64, Arc<AtomicBool>>>,
}
impl ScreenshotRuntime {
    pub fn new(
        capture: CaptureFactory,
        save: Arc<dyn AnnotationSavePort>,
        render: Arc<dyn AnnotationExportRenderPort>,
        clipboard: Arc<dyn ClipboardPort>,
    ) -> Self {
        Self {
            capture,
            save,
            render,
            clipboard,
            session: Mutex::new(None),
            color_job: Mutex::new(None),
            color_serial: Default::default(),
            pin_pending: Default::default(),
            pin_images: Default::default(),
            pin_jobs: Default::default(),
            pin_previews: Default::default(),
            pin_pending_samples: Default::default(),
            pin_samples: Default::default(),
        }
    }
    pub fn sync(&self, state: &lexift_core::domain::screenshot::ScreenshotState, quitting: bool) {
        let mut color = self.color_job.lock().unwrap_or_else(|p| p.into_inner());
        if color.as_ref().is_some_and(|(s, id, _)| {
            quitting
                || *s != state.session
                || !matches!(
                    state.phase,
                    lexift_core::domain::screenshot::ScreenshotPhase::Selecting
                        | lexift_core::domain::screenshot::ScreenshotPhase::Editing
                )
                || state.color_pending.is_none_or(|p| p.id != *id)
        }) && let Some((_, _, cancel)) = color.take()
        {
            cancel.store(true, Ordering::Release);
        }
        drop(color);
        self.pin_pending_samples.lock().unwrap().retain(|key, _| {
            !quitting && state.active() && *key == (state.session, state.operation)
        });
        self.pin_pending.lock().unwrap().retain(|key, _| {
            !quitting && state.active() && *key == (state.session, state.operation)
        });
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if session
            .as_ref()
            .is_some_and(|s| quitting || s.id != state.session || !state.active())
            && let Some(s) = session.take()
        {
            s.cancelled.store(true, Ordering::Release);
        }
    }
    pub fn sync_pins(&self, pins: &PinStates, quitting: bool) {
        self.pin_previews.lock().unwrap().retain(|id, cancel| {
            let keep = !quitting && pins.contains_key(id);
            if !keep {
                cancel.store(true, Ordering::Release);
            }
            keep
        });
        self.pin_samples
            .lock()
            .unwrap()
            .retain(|id, _| !quitting && pins.contains_key(id));
        self.pin_images
            .lock()
            .unwrap()
            .retain(|id, _| !quitting && pins.contains_key(id));
        self.pin_jobs.lock().unwrap().retain(|id, cancel| {
            let keep = !quitting && pins.get(id).is_some_and(|p| p.busy);
            if !keep {
                cancel.store(true, Ordering::Release);
            }
            keep
        });
    }
    pub fn execute_pin(self: &Arc<Self>, controller: &Arc<AppController>, command: PinCommand) {
        match command {
            PinCommand::BeginEdit { id, edit } => controller.ui.begin_pinned_edit(id, edit),
            PinCommand::EndEdit { id, edit } => controller.ui.end_pinned_edit(id, edit),
            PinCommand::Render {
                id,
                revision,
                document,
                hidden,
            } => {
                let Some(base) = self.pin_images.lock().unwrap().get(&id).cloned() else {
                    return;
                };
                let samples = self.pin_samples.lock().unwrap().get(&id).cloned();
                let cancel = Arc::new(AtomicBool::new(false));
                if let Some(old) = self.pin_previews.lock().unwrap().insert(id, cancel.clone()) {
                    old.store(true, Ordering::Release);
                }
                let this = self.clone();
                let c = controller.clone();
                let spawned = std::thread::Builder::new()
                    .name("pinned-image-preview".into())
                    .spawn(move || {
                        let result =
                            this.compose_pin(&base, samples, Some(&document), hidden, &cancel);
                        if !cancel.load(Ordering::Acquire) {
                            match result {
                                Ok(image) => {
                                    c.ui.present_pinned_image(id, revision, Arc::new(image))
                                }
                                Err(error) => {
                                    c.dispatch(AppEvent::PinnedImage(PinEvent::PreviewFailed {
                                        id,
                                        revision,
                                        error: error.to_string(),
                                    }))
                                }
                            }
                        }
                    });
                if let Err(error) = spawned {
                    controller.dispatch(AppEvent::PinnedImage(PinEvent::PreviewFailed {
                        id,
                        revision,
                        error: error.to_string(),
                    }));
                }
            }
            PinCommand::Retain { id, source } => {
                if let Some(samples) = self.pin_pending_samples.lock().unwrap().remove(&source) {
                    self.pin_samples.lock().unwrap().insert(id, samples);
                }
                let image = self.pin_pending.lock().unwrap().remove(&source);
                if let Some(image) = image {
                    self.pin_images.lock().unwrap().insert(id, image);
                }
            }
            PinCommand::Drop { id } => {
                controller.ui.close_pinned_image(id);
                self.pin_images.lock().unwrap().remove(&id);
                self.pin_samples.lock().unwrap().remove(&id);
                if let Some(cancel) = self.pin_jobs.lock().unwrap().remove(&id) {
                    cancel.store(true, Ordering::Release);
                }
            }
            PinCommand::Output {
                id,
                operation,
                kind,
                owner,
                title,
                document,
                hidden,
            } => {
                let image = self.pin_images.lock().unwrap().get(&id).cloned();
                let Some(image) = image else {
                    controller.dispatch(AppEvent::PinnedImage(PinEvent::Completed {
                        id,
                        operation,
                        result: Err(Error::new("Screenshot unavailable")),
                    }));
                    return;
                };
                let cancel = Arc::new(AtomicBool::new(false));
                self.pin_jobs.lock().unwrap().insert(id, cancel.clone());
                let samples = self.pin_samples.lock().unwrap().get(&id).cloned();
                let this = self.clone();
                let c = controller.clone();
                let spawned = std::thread::Builder::new()
                    .name("pinned-image-output".into())
                    .spawn(move || {
                        let result = this
                            .compose_pin(&image, samples, document.as_ref(), hidden, &cancel)
                            .and_then(|image| {
                                this.output_pin_image(&image, kind, owner, title, &cancel)
                            });
                        c.dispatch(AppEvent::PinnedImage(PinEvent::Completed {
                            id,
                            operation,
                            result,
                        }));
                    });
                if let Err(e) = spawned {
                    controller.dispatch(AppEvent::PinnedImage(PinEvent::Completed {
                        id,
                        operation,
                        result: Err(Error::new(e.to_string())),
                    }));
                }
            }
        }
    }
    /// Background-only output of unchanged source pixels; view scale and glow are irrelevant.
    fn compose_pin(
        &self,
        base: &Arc<CapturedRegion>,
        samples: Option<Arc<ScreenCaptureFrame>>,
        document: Option<&ExportRequest>,
        hidden: bool,
        cancel: &AtomicBool,
    ) -> Result<CapturedRegion> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("Screenshot cancelled"));
        }
        let Some(document) = document.filter(|_| !hidden) else {
            return Ok((**base).clone());
        };
        let mut snapshot = (*document.snapshot).clone();
        snapshot.monitor = base.bounds;
        snapshot.monitors = document.snapshot.monitors.clone();
        let capture = Arc::new(ScreenCaptureFrame {
            revision: document.request,
            serial: document.request,
            regions: std::iter::once((**base).clone())
                .chain(samples.iter().flat_map(|s| s.regions.iter().cloned()))
                .collect(),
        });
        let mut rendered = Vec::new();
        for (monitor, scale) in lexift_core::domain::screen_snapshot::render_partitions(
            base.bounds,
            &snapshot.monitors,
            snapshot.scale,
        ) {
            snapshot.monitor = monitor;
            snapshot.scale = scale;
            rendered.push(self.render.compose(&snapshot, capture.clone(), cancel)?);
        }
        crop_frames(&rendered, base.bounds)
    }

    fn output_pin_image(
        &self,
        image: &CapturedRegion,
        kind: PinOutput,
        owner: Option<lexift_core::ports::magnifier::WindowToken>,
        title: String,
        cancel: &Arc<AtomicBool>,
    ) -> Result<SaveOutcome> {
        if cancel.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        match kind {
            PinOutput::Copy => self.write_image(image, cancel).map(|_| SaveOutcome::Saved),
            PinOutput::Save => self.save.save_png(
                PngSaveRequest {
                    image: image.clone(),
                    owner,
                    default_name: "Lexift.png".into(),
                    title,
                },
                cancel.clone(),
            ),
        }
    }
    /// A service gate orders writes; no runtime-state lock is held across the clipboard Port.
    async fn write_color(
        &self,
        color: lexift_core::domain::screen_snapshot::Rgb,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        let _gate = self.color_serial.lock().await;
        let clipboard = self.clipboard.clone();
        tokio::task::spawn_blocking(move || {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::new("Color copy cancelled"));
            }
            clipboard.write_text(&color.hex())
        })
        .await
        .unwrap_or_else(|_| Err(Error::new("Color copy worker failed")))
    }
    /// Image output follows any color write that has already entered the clipboard Port.
    fn write_image(&self, image: &CapturedRegion, cancelled: &AtomicBool) -> Result<()> {
        let _gate = self.color_serial.blocking_lock();
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::new("Screenshot cancelled"));
        }
        self.clipboard.write_image(image)
    }
    pub fn execute(self: &Arc<Self>, controller: &Arc<AppController>, command: ScreenshotCommand) {
        match command {
            ScreenshotCommand::CopyColor { session, request } => {
                let cancel = Arc::new(AtomicBool::new(false));
                if let Some((_, _, old)) = self
                    .color_job
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .replace((session, request.id, cancel.clone()))
                {
                    old.store(true, Ordering::Release);
                }
                let runtime = self.clone();
                let controller = controller.clone();
                let task_runtime = controller.runtime.clone();
                task_runtime.spawn(async move {
                    let result = runtime.write_color(request.color, cancel).await;
                    controller.dispatch(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
                        session,
                        request: request.id,
                        result,
                    }));
                });
            }

            ScreenshotCommand::Restore { session } => {
                controller.ui.restore_screenshot(session);
            }
            ScreenshotCommand::Show { session } => {
                let frames = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.id == session)
                    .and_then(|s| s.frames.clone().map(|f| (f, s.windows.clone())));
                if let Some((frames, windows)) = frames {
                    let capture = Arc::new(lexift_ui::FrozenCapture::new(frames.clone()));
                    controller
                        .ui
                        .show_screenshot_capture(session, frames, capture, windows);
                }
            }
            ScreenshotCommand::Prepare { session, request } => {
                controller.ui.suspend_for_screenshot(session);
                let cancelled = Arc::new(AtomicBool::new(false));
                let old = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .replace(Session {
                        id: session,
                        cancelled: cancelled.clone(),
                        frames: None,
                        windows: Vec::new(),
                    });
                if let Some(old) = old {
                    old.cancelled.store(true, Ordering::Release);
                }
                let this = self.clone();
                let worker_controller = controller.clone();
                let spawned = std::thread::Builder::new()
                    .name("screenshot-prepare".into())
                    .spawn(move || {
                        let result = this.prepare(&request, &cancelled);
                        let result = result.and_then(|prepared| {
                            let mut slot = this.session.lock().unwrap_or_else(|p| p.into_inner());
                            let s = slot
                                .as_mut()
                                .filter(|s| s.id == session && !s.cancelled.load(Ordering::Acquire))
                                .ok_or_else(|| Error::new("Screenshot cancelled"))?;
                            s.frames = Some(Arc::new(prepared.frames));
                            s.windows = prepared.windows;
                            Ok(())
                        });
                        worker_controller.dispatch(AppEvent::Screenshot(
                            ScreenshotEvent::Prepared { session, result },
                        ));
                    });
                if let Err(error) = spawned {
                    controller.dispatch(AppEvent::Screenshot(ScreenshotEvent::Failed {
                        session,
                        error: error.to_string(),
                    }));
                }
            }
            ScreenshotCommand::Output {
                session,
                operation,
                kind,
                request,
            } => {
                let job = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.id == session)
                    .and_then(|s| s.frames.clone().map(|frames| (frames, s.cancelled.clone())));
                let Some((frames, cancelled)) = job else {
                    return;
                };
                let this = self.clone();
                let worker_controller = controller.clone();
                let spawned = std::thread::Builder::new()
                    .name("screenshot-output".into())
                    .spawn(move || {
                        let result = this
                            .output_image(&request, frames.clone(), &cancelled)
                            .and_then(|image| {
                                if cancelled.load(Ordering::Acquire) {
                                    return Ok(SaveOutcome::Cancelled);
                                }
                                match kind {
                                    ScreenshotOutput::Copy => {
                                        this.write_image(&image, &cancelled)?;
                                        Ok(SaveOutcome::Saved)
                                    }
                                    ScreenshotOutput::Save => this.save.save_png(
                                        PngSaveRequest {
                                            image,
                                            owner: request.snapshot.owner,
                                            default_name: request.snapshot.name.clone(),
                                            title: request.snapshot.title.clone(),
                                        },
                                        cancelled.clone(),
                                    ),
                                    ScreenshotOutput::Pin => {
                                        let base = Arc::new(crop_frames(
                                            &frames,
                                            request.snapshot.monitor,
                                        )?);
                                        let mut pin_snapshot = (*request.snapshot).clone();
                                        pin_snapshot.monitors =
                                            lexift_core::domain::screen_snapshot::render_partitions(
                                                base.bounds,
                                                &pin_snapshot.monitors,
                                                pin_snapshot.scale,
                                            );
                                        let pin_request = ExportRequest {
                                            session: request.session,
                                            request: request.request,
                                            snapshot: Arc::new(pin_snapshot),
                                        };
                                        let required = this.render.regions(&pin_request.snapshot);
                                        let samples = Arc::new(ScreenCaptureFrame {
                                            revision: operation,
                                            serial: operation,
                                            regions: required
                                                .into_iter()
                                                .filter(|b| {
                                                    !(b.left >= base.bounds.left
                                                        && b.top >= base.bounds.top
                                                        && b.right <= base.bounds.right
                                                        && b.bottom <= base.bounds.bottom)
                                                })
                                                .map(|b| crop_frames(&frames, b))
                                                .collect::<Result<Vec<_>>>()?,
                                        });
                                        let image = Arc::new(image);
                                        let current = this.session.lock().unwrap();
                                        if current.as_ref().is_some_and(|s| {
                                            s.id == session && !s.cancelled.load(Ordering::Acquire)
                                        }) {
                                            this.pin_pending
                                                .lock()
                                                .unwrap()
                                                .insert((session, operation), base.clone());
                                            this.pin_pending_samples
                                                .lock()
                                                .unwrap()
                                                .insert((session, operation), samples.clone());
                                            drop(current);
                                            worker_controller.ui.show_editable_pinned_screenshot(
                                                session,
                                                operation,
                                                image,
                                                base,
                                                samples,
                                                pin_request,
                                            );
                                        }
                                        Ok(SaveOutcome::Cancelled)
                                    }
                                }
                            });
                        if kind != ScreenshotOutput::Pin || result.is_err() {
                            worker_controller.dispatch(AppEvent::Screenshot(
                                ScreenshotEvent::Completed {
                                    session,
                                    operation,
                                    result,
                                },
                            ));
                        }
                    });
                if let Err(error) = spawned {
                    controller.dispatch(AppEvent::Screenshot(ScreenshotEvent::Completed {
                        session,
                        operation,
                        result: Err(Error::new(error.to_string())),
                    }));
                }
            }
        }
    }
    fn prepare(
        &self,
        request: &ExportRequest,
        cancelled: &AtomicBool,
    ) -> Result<PreparedScreenshot> {
        let snapshot = &request.snapshot;
        let captured = crate::annotation_export::capture_screenshot(
            self.capture,
            snapshot.monitors.iter().map(|(r, _)| *r).collect(),
            snapshot.excluded.clone(),
            cancelled,
        )?
        .ok_or_else(|| Error::new("Screenshot cancelled"))?;
        let frames = snapshot
            .monitors
            .iter()
            .map(|(bounds, scale)| {
                if cancelled.load(Ordering::Acquire) {
                    return Err(Error::new("Screenshot cancelled"));
                }
                let mut snapshot = (**snapshot).clone();
                snapshot.monitor = *bounds;
                snapshot.scale = *scale;
                self.render
                    .compose(&snapshot, captured.frame.clone(), cancelled)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedScreenshot {
            frames,
            windows: captured.windows,
        })
    }
    fn output_image(
        &self,
        request: &ExportRequest,
        frames: Arc<Vec<CapturedRegion>>,
        cancelled: &AtomicBool,
    ) -> Result<CapturedRegion> {
        let capture = Arc::new(ScreenCaptureFrame {
            revision: request.request,
            serial: request.request,
            regions: frames.as_ref().clone(),
        });
        let mut rendered = Vec::new();
        for (monitor, scale) in &request.snapshot.monitors {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::new("Screenshot cancelled"));
            }
            let mut snapshot = (*request.snapshot).clone();
            snapshot.monitor = *monitor;
            snapshot.scale = *scale;
            rendered.push(self.render.compose(&snapshot, capture.clone(), cancelled)?);
        }
        crop_frames(&rendered, request.snapshot.monitor)
    }
}
impl Drop for ScreenshotRuntime {
    fn drop(&mut self) {
        if let Some((_, _, cancel)) = self
            .color_job
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            cancel.store(true, Ordering::Release);
        }
        if let Some(s) = self
            .session
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            s.cancelled.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::{annotation::Session, annotation_export::ExportSnapshot, geometry::Rect},
        ports::annotation_save::PngSaveRequest,
    };
    struct Save;
    #[test]
    fn color_writes_run_off_thread_and_cancel_before_the_service_gate() {
        use lexift_core::domain::screen_snapshot::Rgb;
        struct Colors(Mutex<Vec<(String, std::thread::ThreadId)>>, AtomicBool);
        impl ClipboardPort for Colors {
            fn read_text(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn write_text(&self, text: &str) -> Result<()> {
                if self.1.load(Ordering::Acquire) {
                    return Err(Error::new("fixture"));
                }
                self.0
                    .lock()
                    .unwrap()
                    .push((text.into(), std::thread::current().id()));
                Ok(())
            }
        }
        let clipboard = Arc::new(Colors(Mutex::new(vec![]), AtomicBool::new(false)));
        let runtime = Arc::new(ScreenshotRuntime::new(
            || None,
            Arc::new(Save),
            Arc::new(lexift_ui::SoftwareExportRenderer),
            clipboard.clone(),
        ));
        let origin = std::thread::current().id();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                runtime
                    .write_color(Rgb(1, 2, 3), Arc::new(AtomicBool::new(false)))
                    .await
                    .unwrap();
                assert_eq!(clipboard.0.lock().unwrap()[0].0, "#010203");
                assert_ne!(clipboard.0.lock().unwrap()[0].1, origin);
                let gate = runtime.color_serial.lock().await;
                let cancel = Arc::new(AtomicBool::new(false));
                *runtime.color_job.lock().unwrap() = Some((1, 2, cancel.clone()));
                let worker = runtime.clone();
                let task =
                    tokio::spawn(async move { worker.write_color(Rgb(4, 5, 6), cancel).await });
                runtime.sync(
                    &lexift_core::domain::screenshot::ScreenshotState::default(),
                    false,
                );
                drop(gate);
                assert!(task.await.unwrap().is_err());
                assert_eq!(clipboard.0.lock().unwrap().len(), 1);
                clipboard.1.store(true, Ordering::Release);
                assert!(
                    runtime
                        .write_color(Rgb(7, 8, 9), Arc::new(AtomicBool::new(false)))
                        .await
                        .is_err()
                );
            });
    }
    impl AnnotationSavePort for Save {
        fn is_available(&self) -> bool {
            true
        }
        fn save_png(&self, _: PngSaveRequest, _: Arc<AtomicBool>) -> Result<SaveOutcome> {
            panic!("composition must not save")
        }
    }
    struct Clipboard;
    impl ClipboardPort for Clipboard {
        fn read_text(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn write_text(&self, _: &str) -> Result<()> {
            panic!("image composition must not write text")
        }
    }
    struct Render;
    impl AnnotationExportRenderPort for Render {
        fn regions(&self, snapshot: &ExportSnapshot) -> Vec<Rect> {
            vec![snapshot.monitor]
        }
        fn compose(
            &self,
            snapshot: &ExportSnapshot,
            _: Arc<ScreenCaptureFrame>,
            cancelled: &AtomicBool,
        ) -> Result<CapturedRegion> {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::new("cancelled"));
            }
            let b = snapshot.monitor;
            Ok(CapturedRegion {
                bounds: b,
                pixels: [(snapshot.scale * 40.) as u8, 0, 0, 255]
                    .repeat(((b.right - b.left) * (b.bottom - b.top)) as usize),
            })
        }
    }
    #[test]
    fn cross_dpi_output_composes_each_monitor_before_cropping_and_cancel_has_no_output() {
        let left = Rect {
            left: -20,
            top: 0,
            right: 0,
            bottom: 20,
        };
        let right = Rect {
            left: 0,
            top: 0,
            right: 20,
            bottom: 20,
        };
        let crop = Rect {
            left: -10,
            top: 5,
            right: 10,
            bottom: 15,
        };
        let request = ExportRequest {
            session: 1,
            request: 2,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: crop,
                scale: 1.,
                monitors: vec![(left, 1.), (right, 1.25)],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        let frames = Arc::new(vec![
            CapturedRegion {
                bounds: left,
                pixels: vec![255; 20 * 20 * 4],
            },
            CapturedRegion {
                bounds: right,
                pixels: vec![255; 20 * 20 * 4],
            },
        ]);
        let runtime = ScreenshotRuntime::new(
            || None,
            Arc::new(Save),
            Arc::new(Render),
            Arc::new(Clipboard),
        );
        let image = runtime
            .output_image(&request, frames.clone(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(image.bounds, crop);
        assert_eq!(image.pixels[0], 40);
        assert_eq!(image.pixels[10 * 4], 50);
        assert_eq!(image.pixels.len(), 20 * 10 * 4);
        assert!(
            runtime
                .output_image(&request, frames, &AtomicBool::new(true))
                .is_err()
        );
    }
    #[test]
    fn pin_output_preserves_source_pixels_and_cancellation_prevents_both_ports() {
        #[derive(Default)]
        struct Outputs(Mutex<Vec<Vec<u8>>>);
        impl ClipboardPort for Outputs {
            fn read_text(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn write_text(&self, _: &str) -> Result<()> {
                panic!("not text")
            }
            fn write_image(&self, image: &CapturedRegion) -> Result<()> {
                self.0.lock().unwrap().push(image.pixels.clone());
                Ok(())
            }
        }
        impl AnnotationSavePort for Outputs {
            fn is_available(&self) -> bool {
                true
            }
            fn save_png(&self, request: PngSaveRequest, _: Arc<AtomicBool>) -> Result<SaveOutcome> {
                assert_eq!(
                    request.owner,
                    Some(lexift_core::ports::magnifier::WindowToken(42))
                );
                self.0.lock().unwrap().push(request.image.pixels);
                Ok(SaveOutcome::Cancelled)
            }
        }
        let ports = Arc::new(Outputs::default());
        let runtime =
            ScreenshotRuntime::new(|| None, ports.clone(), Arc::new(Render), ports.clone());
        let image = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 1,
            },
            pixels: vec![20, 30, 40, 255, 50, 60, 70, 255],
        };
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            runtime
                .output_pin_image(&image, PinOutput::Copy, None, String::new(), &cancel)
                .unwrap(),
            SaveOutcome::Saved
        );
        assert_eq!(
            runtime
                .output_pin_image(
                    &image,
                    PinOutput::Save,
                    Some(lexift_core::ports::magnifier::WindowToken(42)),
                    String::new(),
                    &cancel
                )
                .unwrap(),
            SaveOutcome::Cancelled
        );
        assert_eq!(
            *ports.0.lock().unwrap(),
            vec![image.pixels.clone(), image.pixels.clone()]
        );
        cancel.store(true, Ordering::Release);
        for kind in [PinOutput::Copy, PinOutput::Save] {
            assert_eq!(
                runtime
                    .output_pin_image(&image, kind, None, String::new(), &cancel)
                    .unwrap(),
                SaveOutcome::Cancelled
            );
        }
        assert_eq!(ports.0.lock().unwrap().len(), 2);
        let image = Arc::new(image);
        let weak = Arc::downgrade(&image);
        runtime.pin_images.lock().unwrap().insert(5, image);
        runtime.pin_jobs.lock().unwrap().insert(5, cancel.clone());
        runtime.sync_pins(&PinStates::new(), false);
        assert!(weak.upgrade().is_none());
        assert!(runtime.pin_jobs.lock().unwrap().is_empty());
    }
    #[test]
    fn pin_composition_preserves_source_dpi_and_hidden_output_is_exact_base() {
        let runtime = ScreenshotRuntime::new(
            || None,
            Arc::new(Save),
            Arc::new(Render),
            Arc::new(Clipboard),
        );
        let bounds = Rect {
            left: -2,
            top: 0,
            right: 2,
            bottom: 1,
        };
        let base = Arc::new(CapturedRegion {
            bounds,
            pixels: [21, 31, 41, 255].repeat(4),
        });
        let document = ExportRequest {
            session: 1,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: bounds,
                scale: 1.,
                monitors: vec![
                    (
                        Rect {
                            left: -2,
                            top: 0,
                            right: 0,
                            bottom: 1,
                        },
                        1.,
                    ),
                    (
                        Rect {
                            left: 0,
                            top: 0,
                            right: 2,
                            bottom: 1,
                        },
                        1.25,
                    ),
                ],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        let cancel = AtomicBool::new(false);
        let shown = runtime
            .compose_pin(&base, None, Some(&document), false, &cancel)
            .unwrap();
        assert_eq!(
            shown.pixels,
            [
                vec![40, 0, 0, 255, 40, 0, 0, 255],
                vec![50, 0, 0, 255, 50, 0, 0, 255]
            ]
            .concat()
        );
        let hidden = runtime
            .compose_pin(&base, None, Some(&document), true, &cancel)
            .unwrap();
        assert_eq!(hidden.pixels, base.pixels);
        assert_eq!(hidden.bounds, bounds);
        cancel.store(true, Ordering::Release);
        assert!(
            runtime
                .compose_pin(&base, None, Some(&document), true, &cancel)
                .is_err()
        );
    }
    #[test]
    fn pin_keeps_only_required_external_magnifier_samples() {
        use lexift_core::domain::annotation::{
            Bounds, Extra, Kind, MagnifierConnector, Object, ObjectId, Style,
        };
        let runtime = ScreenshotRuntime::new(
            || None,
            Arc::new(Save),
            Arc::new(lexift_ui::SoftwareExportRenderer),
            Arc::new(Clipboard),
        );
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 20,
            bottom: 20,
        };
        let base = Arc::new(CapturedRegion {
            bounds,
            pixels: [220, 220, 220, 255].repeat(400),
        });
        let mut session = Session::new();
        session.add(Object {
            id: ObjectId::DRAFT,
            kind: Kind::Magnifier,
            bounds: Bounds::from_corners((30., 0.), (40., 10.)),
            style: Style {
                width: 0.,
                ..Default::default()
            },
            points: vec![],
            erased: vec![],
            extra: Extra::Magnifier {
                output: Bounds::from_corners((4., 4.), (14., 14.)),
                zoom: 1.,
                ellipse: false,
                connector: MagnifierConnector::None,
                erase_annotations: false,
                antialias: false,
                shadow: false,
            },
        });
        let document = ExportRequest {
            session: 1,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(session),
                monitor: bounds,
                scale: 1.,
                monitors: vec![(
                    Rect {
                        left: 0,
                        top: 0,
                        right: 40,
                        bottom: 20,
                    },
                    1.,
                )],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        let samples = Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds: Rect {
                    left: 30,
                    top: 0,
                    right: 40,
                    bottom: 10,
                },
                pixels: [20, 50, 220, 255].repeat(100),
            }],
        });
        let shown = runtime
            .compose_pin(
                &base,
                Some(samples),
                Some(&document),
                false,
                &AtomicBool::new(false),
            )
            .unwrap();
        let pixel = &shown.pixels[(8 * 20 + 8) * 4..][..4];
        assert!(
            pixel[2] > pixel[0] + 100,
            "outside source samples must survive without retaining full monitor frames"
        );
    }
}
