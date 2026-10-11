//! Executes OCR capabilities and returns versioned events; policy remains in Core.
use crate::controller::AppController;
use lexift_core::{
    AppEvent, AppState,
    domain::ocr::{OcrCommand, OcrEvent},
    ports::{
        ocr::OcrPort,
        screen_capture::{CapturedRegion, ScreenCapturePort},
    },
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Work {
    generation: u64,
    cancel: Arc<AtomicBool>,
    frames: Option<Arc<Vec<CapturedRegion>>>,
}
pub(crate) struct OcrRuntime {
    port: Arc<dyn OcrPort>,
    capture: Arc<dyn ScreenCapturePort>,
    work: Mutex<Work>,
    idle: Arc<AtomicU64>,
    resource: Mutex<(u64, Arc<AtomicBool>)>,
}
impl OcrRuntime {
    pub fn shutdown(&self) {
        let mut w = self.work.lock().unwrap_or_else(|p| p.into_inner());
        w.cancel.store(true, Ordering::Release);
        w.frames = None;
        self.idle.fetch_add(1, Ordering::AcqRel);
        self.resource
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .1
            .store(true, Ordering::Release);
    }
    pub fn new(port: Arc<dyn OcrPort>, capture: Arc<dyn ScreenCapturePort>) -> Self {
        Self {
            port,
            capture,
            work: Mutex::new(Work {
                generation: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                frames: None,
            }),
            idle: Arc::new(AtomicU64::new(0)),
            resource: Mutex::new((0, Arc::new(AtomicBool::new(false)))),
        }
    }
    pub fn sync(&self, state: &AppState) {
        let mut resource = self.resource.lock().unwrap_or_else(|p| p.into_inner());
        if resource.0 != state.ocr.resources.request_id {
            resource.1.store(true, Ordering::Release);
            *resource = (
                state.ocr.resources.request_id,
                Arc::new(AtomicBool::new(false)),
            );
        }
        drop(resource);
        let mut w = self.work.lock().unwrap_or_else(|p| p.into_inner());
        if w.generation != state.ocr.generation {
            w.cancel.store(true, Ordering::Release);
            w.generation = state.ocr.generation;
            w.cancel = Arc::new(AtomicBool::new(false));
            w.frames = None;
        }
    }
    pub fn execute(self: &Arc<Self>, controller: &Arc<AppController>, command: OcrCommand) {
        if let OcrCommand::Resource {
            request_id,
            operation,
        } = command
        {
            let this = self.clone();
            let c = controller.clone();
            let cancel = {
                let r = self.resource.lock().unwrap_or_else(|p| p.into_inner());
                if r.0 != request_id {
                    return;
                }
                r.1.clone()
            };
            controller.runtime.spawn_blocking(move || {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                use lexift_core::domain::ocr::ResourceOperation;
                let result = match operation {
                    ResourceOperation::Check => this.port.resources(),
                    ResourceOperation::Remove | ResourceOperation::RemoveKorean => {
                        this.port.remove_kind(operation.kind())
                    }
                    ResourceOperation::Download | ResourceOperation::DownloadKorean => {
                        let progress_controller = Arc::downgrade(&c);
                        this.port
                            .download_kind(
                                operation.kind(),
                                &cancel,
                                Arc::new(move |received, _| {
                                    if let Some(c) = progress_controller.upgrade() {
                                        c.dispatch(AppEvent::Ocr(OcrEvent::ResourceProgress {
                                            request_id,
                                            received,
                                        }));
                                    }
                                }),
                            )
                            .and_then(|()| this.port.resources())
                    }
                };
                if !cancel.load(Ordering::Acquire) {
                    let event = match result {
                        Ok(info) => OcrEvent::ResourceCompleted {
                            request_id,
                            result: Ok(info),
                        },
                        Err(error) => OcrEvent::ResourceFailed {
                            request_id,
                            info: this.port.resources().ok(),
                            error: error.to_string(),
                        },
                    };
                    c.dispatch(AppEvent::Ocr(event));
                }
            });
            return;
        }
        if command == OcrCommand::ReleaseKorean {
            let port = self.port.clone();
            controller
                .runtime
                .spawn_blocking(move || port.release_korean());
            return;
        }
        if command == OcrCommand::CancelResourceDownload {
            return;
        }
        if command == OcrCommand::Cancel {
            self.work.lock().unwrap_or_else(|p| p.into_inner()).frames = None;
            return;
        }
        let this = self.clone();
        let c = controller.clone();
        let (generation, cancel) = {
            let w = self.work.lock().unwrap_or_else(|p| p.into_inner());
            (w.generation, w.cancel.clone())
        };
        let idle_version = self.idle.fetch_add(1, Ordering::AcqRel) + 1;
        controller.runtime.spawn_blocking(move || {
            let result: lexift_core::Result<()> = (|| {
                match command {
                    OcrCommand::Check {
                        generation: requested,
                    } if requested == generation => {
                        let r = this.port.resources()?;
                        c.dispatch(AppEvent::Ocr(OcrEvent::Checked {
                            generation,
                            resources: r,
                        }));
                    }
                    OcrCommand::Download {
                        generation: requested,
                        kind,
                    } if requested == generation => {
                        let progress_controller = Arc::downgrade(&c);
                        this.port.download_kind(
                            kind,
                            &cancel,
                            Arc::new(move |received, total| {
                                if let Some(c) = progress_controller.upgrade() {
                                    c.dispatch(AppEvent::Ocr(OcrEvent::DownloadProgress {
                                        generation,
                                        received,
                                        total,
                                    }));
                                }
                            }),
                        )?;
                        c.dispatch(AppEvent::Ocr(OcrEvent::DownloadFinished { generation }));
                    }
                    OcrCommand::Capture {
                        generation: requested,
                    } if requested == generation => {
                        let frames = Arc::new(this.capture.freeze_desktop()?);
                        if cancel.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        {
                            let mut w = this.work.lock().unwrap_or_else(|p| p.into_inner());
                            if w.generation != generation {
                                return Ok(());
                            }
                            w.frames = Some(frames.clone());
                        }
                        c.ui.show_ocr_capture(generation, frames);
                    }
                    OcrCommand::Recognize {
                        generation: requested,
                        snapshot_id,
                        bounds,
                        strategy,
                    } if requested == generation && snapshot_id == generation => {
                        let frames = {
                            this.work
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .frames
                                .take()
                        }
                        .ok_or_else(|| lexift_core::Error::new("OCR screen capture expired"))?;
                        let image =
                            lexift_core::domain::screen_snapshot::crop_frames(&frames, bounds)?;
                        drop(frames);
                        let text = this.port.recognize(image, &strategy, &cancel)?;
                        c.dispatch(AppEvent::Ocr(OcrEvent::Recognized { generation, text }));
                    }
                    _ => {}
                }
                Ok(())
            })();
            if !cancel.load(Ordering::Acquire)
                && let Err(e) = result
            {
                c.dispatch(AppEvent::Ocr(OcrEvent::Failed {
                    generation,
                    error: e.to_string(),
                }));
            }
            let port = this.port.clone();
            let idle = this.idle.clone();
            c.runtime.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                if idle.load(Ordering::Acquire) == idle_version {
                    let _ = tokio::task::spawn_blocking(move || port.release()).await;
                }
            });
        });
    }
}
impl Drop for OcrRuntime {
    fn drop(&mut self) {
        self.resource
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .1
            .store(true, Ordering::Release);
        self.work
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .cancel
            .store(true, Ordering::Release);
    }
}
