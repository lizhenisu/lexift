//! Single latest-request queue shared by pin and image-editor copy-only OCR.
use super::*;
use lexift_core::domain::{
    editor_ocr as editor,
    ocr::{OcrOutcome, OcrRecognitionStrategy},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Identity {
    Pin {
        id: u64,
        request: lexift_core::domain::pinned_image::Recognition,
    },
    Editor(editor::Request),
}
pub(super) enum Source {
    Single(Arc<CapturedRegion>),
    Group(Arc<Vec<CapturedRegion>>),
}
impl Source {
    fn frames(&self) -> &[CapturedRegion] {
        match self {
            Self::Single(f) => std::slice::from_ref(f.as_ref()),
            Self::Group(f) => f.as_slice(),
        }
    }
}
struct Task {
    identity: Identity,
    bounds: lexift_core::domain::geometry::Rect,
    strategy: OcrRecognitionStrategy,
    frames: Source,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
pub(super) struct Queue {
    running: bool,
    current: Option<(Identity, Arc<AtomicBool>)>,
    pending: Option<Task>,
}
impl ScreenshotRuntime {
    pub fn sync_image_ocr(&self, state: &lexift_core::AppState, quitting: bool) {
        let mut q = self.image_ocr.lock().unwrap();
        if let Some((identity, cancel)) = &q.current {
            let valid = match identity {
                Identity::Pin { id, request } => state
                    .pinned_images
                    .get(id)
                    .is_some_and(|p| p.recognition == Some(*request)),
                Identity::Editor(r) => state.editor_ocr.request == Some(*r),
            };
            if quitting || !valid {
                cancel.store(true, Ordering::Release);
                q.pending = None;
            }
        }
    }
    pub fn execute_editor_ocr(self: &Arc<Self>, c: &Arc<AppController>, command: editor::Command) {
        let request = command.request;
        if c.state.lock().unwrap().editor_ocr.request != Some(request) {
            return;
        }
        let frames = match request.owner {
            editor::Owner::Document { window, editor } => self
                .history
                .lock()
                .unwrap()
                .editor
                .as_ref()
                .filter(|(w, e, _)| *w == window && *e == editor)
                .map(|(_, _, image)| Source::Single(image.base.clone())),
            editor::Owner::Long { session, .. } => self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|s| s.id == session)
                .and_then(|s| s.frames.clone())
                .map(Source::Group),
        };
        let Some(frames) = frames.filter(|frames| {
            frames.frames().len() == 1 && frames.frames()[0].bounds == request.source
        }) else {
            c.dispatch(AppEvent::EditorOcr(editor::Event::Completed {
                request,
                outcome: OcrOutcome::RecognitionFailed,
            }));
            return;
        };
        self.enqueue_image_ocr(
            c,
            Identity::Editor(request),
            request.bounds,
            command.strategy,
            frames,
        );
    }
    pub(super) fn enqueue_image_ocr(
        self: &Arc<Self>,
        c: &Arc<AppController>,
        identity: Identity,
        bounds: lexift_core::domain::geometry::Rect,
        strategy: OcrRecognitionStrategy,
        frames: Source,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let start = {
            // Coordinate installation with cancellation snapshots before releasing domain identity.
            let state = c.state.lock().unwrap();
            let valid = match identity {
                Identity::Editor(r) => state.editor_ocr.request == Some(r),
                Identity::Pin { id, request } => state
                    .pinned_images
                    .get(&id)
                    .is_some_and(|p| p.recognition == Some(request)),
            };
            if !valid {
                return;
            }
            let mut q = self.image_ocr.lock().unwrap();
            if let Some((_, old)) = q.current.take() {
                old.store(true, Ordering::Release);
            }
            q.current = Some((identity, cancel.clone()));
            q.pending = Some(Task {
                identity,
                bounds,
                strategy,
                frames,
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
            let c = c.clone();
            let runtime = c.runtime.clone();
            runtime.spawn_blocking(move || this.run_image_ocr(&c));
        }
    }
    fn run_image_ocr(self: &Arc<Self>, c: &Arc<AppController>) {
        loop {
            let task = {
                let mut q = self.image_ocr.lock().unwrap();
                match q.pending.take() {
                    Some(t) => t,
                    None => {
                        q.running = false;
                        return;
                    }
                }
            };
            let started = std::time::Instant::now();
            let outcome =
                self.recognize_text(task.frames.frames(), task.bounds, &task.cancel, |image| {
                    c.ocr
                        .as_ref()
                        .ok_or(OcrOutcome::ResourcesUnavailable)?
                        .recognize_for_copy(c, image, &task.strategy, &task.cancel)
                });
            tracing::debug!(?task.identity, elapsed_ms = started.elapsed().as_millis(), ?outcome, "Image OCR finished");
            if let Some(outcome) = outcome {
                c.dispatch(match task.identity {
                    Identity::Editor(request) => {
                        AppEvent::EditorOcr(editor::Event::Completed { request, outcome })
                    }
                    Identity::Pin { id, request } => AppEvent::PinnedImage(PinEvent::Recognized {
                        id,
                        operation: request.operation,
                        revision: request.revision,
                        outcome,
                    }),
                });
            }
        }
    }
}
