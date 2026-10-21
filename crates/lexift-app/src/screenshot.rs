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
#[path = "screenshot_long.rs"]
mod long;
type CaptureFactory = fn() -> Option<Arc<dyn ScreenCapturePort>>;
struct PreparedScreenshot {
    frames: Vec<CapturedRegion>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
}
struct Session {
    id: u64,
    cancelled: Arc<AtomicBool>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    long_source_frames: Option<Arc<Vec<CapturedRegion>>>,
    element_port: Option<Arc<dyn ScreenCapturePort>>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
}
impl Session {
    /// Retire native preparation and accessibility ownership outside the session lock.
    fn retire(self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(port) = self.element_port {
            port.cancel_screenshot_elements(self.id);
        }
        lexift_platform::clear_screenshot_surface_pixels(self.id);
    }
}
struct PinOcrTask {
    id: u64,
    request: lexift_core::domain::pinned_image::Recognition,
    strategy: lexift_core::domain::ocr::OcrRecognitionStrategy,
    base: Arc<CapturedRegion>,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct PinOcrQueue {
    running: bool,
    current: Option<(
        u64,
        lexift_core::domain::pinned_image::Recognition,
        Arc<AtomicBool>,
    )>,
    pending: Option<PinOcrTask>,
}
pub(crate) struct ScreenshotRuntime {
    long: Mutex<Option<long::Job>>,
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
    pin_ocr: Mutex<PinOcrQueue>,
}
impl ScreenshotRuntime {
    pub fn new(
        capture: CaptureFactory,
        save: Arc<dyn AnnotationSavePort>,
        render: Arc<dyn AnnotationExportRenderPort>,
        clipboard: Arc<dyn ClipboardPort>,
    ) -> Self {
        Self {
            long: Default::default(),
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
            pin_ocr: Default::default(),
            pin_pending_samples: Default::default(),
            pin_samples: Default::default(),
        }
    }
    /// Crop the immutable capture, infer off-thread, then serialize the final clipboard write.
    fn recognize_text(
        &self,
        frames: &[CapturedRegion],
        bounds: lexift_core::domain::geometry::Rect,
        cancel: &AtomicBool,
        recognize: impl FnOnce(
            CapturedRegion,
        ) -> std::result::Result<
            String,
            lexift_core::domain::screenshot::OcrOutcome,
        >,
    ) -> Option<lexift_core::domain::screenshot::OcrOutcome> {
        use lexift_core::domain::screenshot::OcrOutcome;
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let recognized = crop_frames(frames, bounds)
            .map_err(|_| OcrOutcome::RecognitionFailed)
            .and_then(recognize);
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let outcome = match recognized {
            Ok(text) if text.trim().is_empty() => OcrOutcome::Empty,
            Ok(text) => {
                let _serial = self.color_serial.blocking_lock();
                if cancel.load(Ordering::Acquire) {
                    return None;
                }
                match self.clipboard.write_text(&text) {
                    Ok(()) => OcrOutcome::Copied,
                    Err(_) => OcrOutcome::CopyFailed,
                }
            }
            Err(outcome) => outcome,
        };
        (!cancel.load(Ordering::Acquire)).then_some(outcome)
    }
    pub fn sync(&self, state: &lexift_core::domain::screenshot::ScreenshotState, quitting: bool) {
        long::sync(self, state, quitting);
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
        if state.phase != lexift_core::domain::screenshot::ScreenshotPhase::Selecting {
            let port = self
                .session
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(|s| (s.id, s.element_port.clone()));
            if let Some((id, Some(port))) = port {
                port.cancel_screenshot_elements(id);
            }
        }
        let retired = {
            let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
            if session
                .as_ref()
                .is_some_and(|s| quitting || s.id != state.session || !state.active())
            {
                session.take()
            } else {
                None
            }
        };
        if let Some(s) = retired {
            s.retire();
        }
    }
    pub fn sync_pins(&self, pins: &PinStates, quitting: bool) {
        {
            let mut q = self.pin_ocr.lock().unwrap();
            if let Some((id, request, cancel)) = &q.current
                && (quitting || pins.get(id).is_none_or(|p| p.recognition != Some(*request)))
            {
                cancel.store(true, Ordering::Release);
                q.pending = None;
            }
        }
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
    #[cfg(test)]
    pub(crate) fn install_test_pin_source(&self, id: u64, image: Arc<CapturedRegion>) {
        self.pin_images.lock().unwrap().insert(id, image);
    }
    pub fn execute_pin(self: &Arc<Self>, controller: &Arc<AppController>, command: PinCommand) {
        match command {
            PinCommand::Recognize {
                id,
                request,
                strategy,
            } => {
                let base = self.pin_images.lock().unwrap().get(&id).cloned();
                let Some(base) = base else {
                    controller.dispatch(AppEvent::PinnedImage(PinEvent::Recognized {
                        id,
                        operation: request.operation,
                        revision: request.revision,
                        outcome: lexift_core::domain::ocr::OcrOutcome::RecognitionFailed,
                    }));
                    return;
                };
                let cancel = Arc::new(AtomicBool::new(false));
                let start = {
                    let mut q = self.pin_ocr.lock().unwrap();
                    if let Some((_, _, old)) = q.current.take() {
                        old.store(true, Ordering::Release);
                    }
                    q.current = Some((id, request, cancel.clone()));
                    q.pending = Some(PinOcrTask {
                        id,
                        request,
                        strategy,
                        base,
                        cancel,
                    });
                    if q.running {
                        false
                    } else {
                        q.running = true;
                        true
                    }
                };
                if start {
                    let this = self.clone();
                    let c = controller.clone();
                    controller
                        .runtime
                        .spawn_blocking(move || this.run_pin_ocr(&c));
                }
            }
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
                                    c.dispatch(AppEvent::PinnedImage(PinEvent::Rendered {
                                        id,
                                        revision,
                                    }));
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
                    let bounds = image.bounds;
                    self.pin_images.lock().unwrap().insert(id, image);
                    controller
                        .dispatch(AppEvent::PinnedImage(PinEvent::SourceReady { id, bounds }));
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
    /// One worker owns inference scheduling. Replacements retain only a shared base
    /// and the latest descriptor, never a queue of full cropped input buffers.
    fn run_pin_ocr(self: &Arc<Self>, controller: &Arc<AppController>) {
        loop {
            let task = {
                let mut q = self.pin_ocr.lock().unwrap();
                match q.pending.take() {
                    Some(t) => t,
                    None => {
                        q.running = false;
                        return;
                    }
                }
            };
            let PinOcrTask {
                id,
                request,
                strategy,
                base,
                cancel,
            } = task;
            let started = std::time::Instant::now();
            let outcome = self.recognize_text(
                std::slice::from_ref(base.as_ref()),
                request.bounds,
                &cancel,
                |image| {
                    controller
                        .ocr
                        .as_ref()
                        .ok_or(lexift_core::domain::ocr::OcrOutcome::ResourcesUnavailable)?
                        .recognize_for_copy(controller, image, &strategy, &cancel)
                },
            );
            tracing::debug!(
                id,
                operation = request.operation,
                revision = request.revision,
                elapsed_ms = started.elapsed().as_millis(),
                ?outcome,
                "Pinned image OCR finished"
            );
            if let Some(outcome) = outcome {
                controller.dispatch(AppEvent::PinnedImage(PinEvent::Recognized {
                    id,
                    operation: request.operation,
                    revision: request.revision,
                    outcome,
                }));
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
        let visible = document.map_or(base.bounds, |d| {
            d.snapshot.session.image_crop.unwrap_or(base.bounds)
        });
        let Some(document) = document.filter(|_| !hidden) else {
            return crop_frames(&[(**base).clone()], visible);
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
        crop_frames(&rendered, visible)
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
            ScreenshotCommand::Long(command) => long::execute(self, controller, command),
            ScreenshotCommand::Recognize {
                session,
                operation,
                bounds,
                strategy,
            } => {
                let job = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.id == session)
                    .and_then(|s| s.frames.clone().map(|f| (f, s.cancelled.clone())));
                let Some((frames, cancel)) = job else {
                    return;
                };
                let this = self.clone();
                let c = controller.clone();
                controller.runtime.spawn_blocking(move || {
                    let outcome = this.recognize_text(&frames, bounds, &cancel, |image| {
                        c.ocr
                            .as_ref()
                            .ok_or(
                                lexift_core::domain::screenshot::OcrOutcome::ResourcesUnavailable,
                            )
                            .and_then(|ocr| ocr.recognize_for_copy(&c, image, &strategy, &cancel))
                    });
                    if let Some(outcome) = outcome {
                        c.dispatch(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                            session,
                            operation,
                            outcome,
                        }));
                    }
                });
            }
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

            ScreenshotCommand::QueryElements(query) => {
                let owned = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.id == query.session && !s.cancelled.load(Ordering::Acquire))
                    .and_then(|s| s.element_port.clone().map(|p| (p, s.cancelled.clone())));
                if let Some((port, cancelled)) = owned {
                    let controller = controller.clone();
                    let _ = port.query_screenshot_elements(
                        query,
                        cancelled,
                        Box::new(move |result| {
                            controller.dispatch(AppEvent::Screenshot(
                                ScreenshotEvent::ElementsPrepared(result),
                            ));
                        }),
                    );
                }
            }
            ScreenshotCommand::CancelElements { session } => {
                let port = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.id == session)
                    .and_then(|s| s.element_port.clone());
                if let Some(port) = port {
                    port.cancel_screenshot_elements(session);
                }
            }
            ScreenshotCommand::ReuseRegion {
                session,
                host,
                request,
                bounds,
            } => {
                controller
                    .ui
                    .reuse_screenshot_region(session, host, request, bounds);
            }
            ScreenshotCommand::UpdateElements(result) => {
                controller.ui.update_screenshot_elements(result);
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
                let cancelled = Arc::new(AtomicBool::new(false));
                let element_port = (self.capture)();
                let old = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .replace(Session {
                        id: session,
                        cancelled: cancelled.clone(),
                        frames: None,
                        long_source_frames: None,
                        element_port,
                        windows: Vec::new(),
                    });
                if let Some(old) = old {
                    old.retire();
                }
                let this = self.clone();
                let worker_controller = controller.clone();
                controller.ui.suspend_for_screenshot(
                    session,
                    Box::new(move |ready| {
                        if cancelled.load(Ordering::Acquire) {
                            return;
                        }
                        let pointer = match ready {
                            Ok(pointer) => pointer,
                            Err(error) => {
                                worker_controller.dispatch(AppEvent::Screenshot(
                                    ScreenshotEvent::Prepared {
                                        session,
                                        result: Err(error),
                                    },
                                ));
                                return;
                            }
                        };
                        let failure_controller = worker_controller.clone();
                        let spawned = std::thread::Builder::new()
                            .name("screenshot-prepare".into())
                            .spawn(move || {
                                let started = std::time::Instant::now();
                                let result = this.prepare(&request, &cancelled, pointer, session);
                                match result {
                                    Ok(prepared) => {
                                        let layout =
                                            prepared.frames.iter().map(|f| f.bounds).collect();
                                        {
                                            let mut slot = this
                                                .session
                                                .lock()
                                                .unwrap_or_else(|p| p.into_inner());
                                            let Some(s) = slot.as_mut().filter(|s| {
                                                s.id == session
                                                    && !s.cancelled.load(Ordering::Acquire)
                                            }) else {
                                                return;
                                            };
                                            s.frames = Some(Arc::new(prepared.frames));
                                            s.windows = prepared.windows;
                                        }
                                        tracing::debug!(
                                            session,
                                            elapsed_ms = started.elapsed().as_millis(),
                                            "Screenshot first frame dispatched"
                                        );
                                        worker_controller.dispatch(AppEvent::Screenshot(
                                            ScreenshotEvent::Prepared {
                                                session,
                                                result: Ok(layout),
                                            },
                                        ));
                                    }
                                    Err(error) => worker_controller.dispatch(AppEvent::Screenshot(
                                        ScreenshotEvent::Prepared {
                                            session,
                                            result: Err(error),
                                        },
                                    )),
                                }
                            });
                        if let Err(error) = spawned {
                            failure_controller.dispatch(AppEvent::Screenshot(
                                ScreenshotEvent::Failed {
                                    session,
                                    error: error.to_string(),
                                },
                            ));
                        }
                    }),
                );
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
                                            if request.snapshot.session.image_crop.is_some() { request.snapshot.session.image_source.unwrap_or(request.snapshot.monitor) } else { request.snapshot.monitor },
                                        )?);
                                        let mut pin_snapshot = (*request.snapshot).clone();
                                        if request.snapshot.session.image_crop.is_none() {
                                            pin_snapshot.monitors =
                                                lexift_core::domain::screen_snapshot::render_partitions(
                                                    base.bounds, &pin_snapshot.monitors, pin_snapshot.scale,
                                                );
                                        }
                                        let pin_request = ExportRequest {
                                            session: request.session,
                                            request: request.request,
                                            snapshot: Arc::new(pin_snapshot),
                                        };
                                        let mut required =
                                            this.render.regions(&pin_request.snapshot);
                                        // Keep complete frozen sampling pixels for effects crossing a crop edge.
                                        if request.snapshot.session.image_crop.is_some() {
                                            required.extend(frames.iter().map(|f| f.bounds));
                                        }
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
        pointer: Option<lexift_core::domain::geometry::Point>,
        session: u64,
    ) -> Result<PreparedScreenshot> {
        let snapshot = &request.snapshot;
        let prepared = crate::annotation_export::capture_screenshot_frame(
            self.capture,
            snapshot.monitors.iter().map(|(r, _)| *r).collect(),
            snapshot.excluded.clone(),
            cancelled,
            pointer,
        )?
        .ok_or_else(|| Error::new("Screenshot cancelled"))?;
        let captured = prepared.capture;
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
        lexift_platform::prepare_screenshot_surface_pixels(session, &frames, cancelled)?;
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
            s.retire();
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
    fn screenshot_ocr_uses_frozen_cross_screen_pixels_and_never_copies_empty_failed_or_cancelled_text()
     {
        use lexift_core::domain::{geometry::Rect, screenshot::OcrOutcome};
        struct Texts(Mutex<Vec<String>>, AtomicBool);
        impl ClipboardPort for Texts {
            fn read_text(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn write_text(&self, text: &str) -> Result<()> {
                if self.1.load(Ordering::Acquire) {
                    return Err(Error::new("fixture"));
                }
                self.0.lock().unwrap().push(text.into());
                Ok(())
            }
        }
        let clipboard = Arc::new(Texts(Mutex::new(vec![]), AtomicBool::new(false)));
        let runtime = ScreenshotRuntime::new(
            || None,
            Arc::new(Save),
            Arc::new(lexift_ui::SoftwareExportRenderer),
            clipboard.clone(),
        );
        let bounds = Rect {
            left: -1,
            top: 0,
            right: 1,
            bottom: 1,
        };
        let frames = vec![
            CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: 0,
                    right: 0,
                    bottom: 1,
                },
                pixels: [10, 20, 30, 255].repeat(2),
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 2,
                    bottom: 1,
                },
                pixels: [40, 50, 60, 255].repeat(2),
            },
        ];
        let cancel = AtomicBool::new(false);
        let result = runtime.recognize_text(&frames, bounds, &cancel, |image| {
            assert_eq!(image.bounds, bounds);
            assert_eq!(image.pixels, [10, 20, 30, 255, 40, 50, 60, 255]);
            Ok("line one\nline two".into())
        });
        assert_eq!(result, Some(OcrOutcome::Copied));
        assert_eq!(&*clipboard.0.lock().unwrap(), &["line one\nline two"]);
        for result in [
            Ok(" \r\n\t".into()),
            Err(OcrOutcome::RecognitionFailed),
            Err(OcrOutcome::ResourcesUnavailable),
        ] {
            let expected = match &result {
                Ok(_) => OcrOutcome::Empty,
                Err(e) => *e,
            };
            assert_eq!(
                runtime.recognize_text(&frames, bounds, &cancel, |_| result),
                Some(expected)
            );
        }
        clipboard.1.store(true, Ordering::Release);
        assert_eq!(
            runtime.recognize_text(&frames, bounds, &cancel, |_| Ok("text".into())),
            Some(OcrOutcome::CopyFailed)
        );
        clipboard.1.store(false, Ordering::Release);
        assert_eq!(
            runtime.recognize_text(&frames, bounds, &cancel, |_| {
                cancel.store(true, Ordering::Release);
                Ok("stale".into())
            }),
            None
        );
        assert_eq!(
            runtime.recognize_text(&frames, bounds, &cancel, |_| panic!("cancelled inference")),
            None
        );
        assert_eq!(clipboard.0.lock().unwrap().len(), 1);
    }
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
    fn long_image_crop_outputs_exact_full_composition_slice_and_retains_effect_sampling() {
        use lexift_core::domain::annotation::{
            Bounds, Extra, Kind, MagnifierConnector, MosaicAnnotation, MosaicEffect, Object, Style,
        };
        let full = Rect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 300,
        };
        let crop = Rect {
            left: 80,
            top: 90,
            right: 170,
            bottom: 260,
        };
        let pixels = (0..200 * 300)
            .flat_map(|n| {
                [
                    (n % 251) as u8,
                    (n * 7 % 253) as u8,
                    (n * 13 % 255) as u8,
                    255,
                ]
            })
            .collect();
        let frames = Arc::new(vec![CapturedRegion {
            bounds: full,
            pixels,
        }]);
        for effect in [MosaicEffect::Pixelate, MosaicEffect::Blur] {
            let mut doc = Session::new();
            doc.image_source = Some(full);
            let mut mosaic = MosaicAnnotation::new(effect, 60, 12., 1.);
            mosaic.id = 1;
            doc.add(Object {
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((50., 50.), (140., 190.)),
                points: vec![],
                erased: vec![],
                style: Style::default(),
                extra: Extra::Mosaic(mosaic),
            });
            doc.add(Object {
                id: Default::default(),
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((50., 50.), (70., 70.)),
                points: vec![],
                erased: vec![],
                style: Style::default(),
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((145., 220.), (180., 255.)),
                    zoom: 2.,
                    erase_annotations: false,
                    ellipse: false,
                    connector: MagnifierConnector::None,
                    antialias: true,
                    shadow: false,
                },
            });
            let mut snapshot = ExportSnapshot {
                session: Arc::new(doc),
                monitor: full,
                scale: 1.,
                monitors: vec![(full, 1.)],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            };
            let runtime = ScreenshotRuntime::new(
                || None,
                Arc::new(Save),
                Arc::new(lexift_ui::SoftwareExportRenderer),
                Arc::new(Clipboard),
            );
            let original = runtime
                .output_image(
                    &ExportRequest {
                        session: 1,
                        request: 1,
                        snapshot: Arc::new(snapshot.clone()),
                    },
                    frames.clone(),
                    &AtomicBool::new(false),
                )
                .unwrap();
            Arc::make_mut(&mut snapshot.session).set_image_crop(crop, full);
            snapshot.monitor = crop;
            let actual = runtime
                .output_image(
                    &ExportRequest {
                        session: 1,
                        request: 2,
                        snapshot: Arc::new(snapshot.clone()),
                    },
                    frames.clone(),
                    &AtomicBool::new(false),
                )
                .unwrap();
            let expected = crop_frames(&[original], crop).unwrap();
            assert_eq!(actual.pixels, expected.pixels);
            assert_eq!(actual.bounds, crop);
            let base = Arc::new(crop_frames(&frames, full).unwrap());
            let samples = Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: frames.as_ref().clone(),
            });
            let pinned = runtime
                .compose_pin(
                    &base,
                    Some(samples),
                    Some(&ExportRequest {
                        session: 1,
                        request: 3,
                        snapshot: Arc::new(snapshot.clone()),
                    }),
                    false,
                    &AtomicBool::new(false),
                )
                .unwrap();
            assert_eq!(pinned.pixels, expected.pixels);
            let mut request = ExportRequest {
                session: 1,
                request: 4,
                snapshot: Arc::new(snapshot),
            };
            let hidden = runtime
                .compose_pin(&base, None, Some(&request), true, &AtomicBool::new(false))
                .unwrap();
            assert_eq!(
                hidden,
                crop_frames(&frames, crop).unwrap(),
                "hiding annotations retains crop"
            );
            let restored = Arc::make_mut(&mut request.snapshot);
            Arc::make_mut(&mut restored.session).set_image_crop(full, full);
            restored.monitor = full;
            let expanded = runtime
                .compose_pin(
                    &base,
                    Some(Arc::new(ScreenCaptureFrame {
                        revision: 1,
                        serial: 1,
                        regions: frames.as_ref().clone(),
                    })),
                    Some(&request),
                    false,
                    &AtomicBool::new(false),
                )
                .unwrap();
            assert_eq!(
                expanded.pixels,
                runtime
                    .output_image(&request, frames.clone(), &AtomicBool::new(false))
                    .unwrap()
                    .pixels
            );
            assert_eq!(expanded.bounds, full, "pin can restore complete long image");
        }
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
