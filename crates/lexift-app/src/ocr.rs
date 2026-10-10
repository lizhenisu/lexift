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
}
impl OcrRuntime {
    pub fn shutdown(&self) {
        let mut w = self.work.lock().unwrap_or_else(|p| p.into_inner());
        w.cancel.store(true, Ordering::Release);
        w.frames = None;
        self.idle.fetch_add(1, Ordering::AcqRel);
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
        }
    }
    pub fn sync(&self, state: &AppState) {
        let mut w = self.work.lock().unwrap_or_else(|p| p.into_inner());
        if w.generation != state.ocr.generation {
            w.cancel.store(true, Ordering::Release);
            w.generation = state.ocr.generation;
            w.cancel = Arc::new(AtomicBool::new(false));
            w.frames = None;
        }
    }
    pub fn execute(self: &Arc<Self>, controller: &Arc<AppController>, command: OcrCommand) {
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
                        c.dispatch(AppEvent::Ocr(OcrEvent::Resources {
                            generation,
                            ready: r.ready,
                            bytes: r.download_bytes,
                        }));
                    }
                    OcrCommand::Download {
                        generation: requested,
                    } if requested == generation => {
                        let progress_controller = Arc::downgrade(&c);
                        this.port.download(
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
                        language,
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
                        let text = this.port.recognize(image, &language, &cancel)?;
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
        self.work
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .cancel
            .store(true, Ordering::Release);
    }
}
