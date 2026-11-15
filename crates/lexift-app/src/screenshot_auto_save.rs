//! Bounded background ownership for copies that have already succeeded.
use super::*;
use lexift_core::{
    domain::{
        geometry::Point,
        screenshot::{CopiedImage, ScreenshotState},
    },
    ports::{
        annotation_save::{ImageFormat, ImageOpenRequest, ImageSaveOptions},
        magnifier::WindowToken,
    },
};

pub(super) struct Job {
    pub session: u64,
    pub operation: u64,
    pub name: String,
    pub directory: String,
    pub image: CopiedImage,
}

#[derive(Default)]
pub(super) struct Queue {
    active: Option<Arc<AtomicBool>>,
    pending: Option<Job>,
    picker: Option<(u64, Arc<AtomicBool>)>,
    quitting: bool,
}

enum Admission {
    Start(Arc<AtomicBool>, Job),
    Queued,
    Full(Job),
    Closed,
}

impl Queue {
    fn admit(&mut self, job: Job) -> Admission {
        if self.quitting {
            return Admission::Closed;
        }
        if self.active.is_none() {
            let cancel = Arc::new(AtomicBool::new(false));
            self.active = Some(cancel.clone());
            return Admission::Start(cancel, job);
        }
        if self.pending.is_some() {
            return Admission::Full(job);
        }
        self.pending = Some(job);
        Admission::Queued
    }

    fn close(&mut self) {
        self.quitting = true;
        self.pending = None;
        if let Some(cancel) = &self.active {
            cancel.store(true, Ordering::Release);
        }
    }
}

pub(super) fn sync(runtime: &ScreenshotRuntime, state: &ScreenshotState, quitting: bool) {
    let mut queue = runtime.auto_save.lock().unwrap();
    if queue
        .picker
        .as_ref()
        .is_some_and(|(id, _)| quitting || state.save_directory_picker != Some(*id))
        && let Some((_, cancel)) = queue.picker.take()
    {
        cancel.store(true, Ordering::Release);
    }
    if quitting {
        queue.close();
    }
}

pub(super) fn choose_directory(
    runtime: &Arc<ScreenshotRuntime>,
    controller: &Arc<AppController>,
    owner: Option<WindowToken>,
    request: u64,
    enable: bool,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    let language = {
        let state = controller.state.lock().unwrap();
        if state.screenshot.save_directory_picker != Some(request) {
            return;
        }
        let mut queue = runtime.auto_save.lock().unwrap();
        if queue.quitting {
            return;
        }
        queue.picker = Some((request, cancel.clone()));
        state.settings.ui_language
    };
    let controller = controller.clone();
    let save = runtime.save.clone();
    controller.clone().runtime.spawn_blocking(move || {
        let directory = save
            .choose_directory(
                ImageOpenRequest {
                    owner,
                    title: lexift_ui::quick_save_directory_title(language),
                },
                cancel.clone(),
            )
            .and_then(|path| {
                path.map(|path| {
                    path.into_os_string()
                        .into_string()
                        .map_err(|_| Error::new("Invalid quick save directory"))
                })
                .transpose()
            });
        if !cancel.load(Ordering::Acquire) {
            controller.dispatch(AppEvent::Screenshot(
                ScreenshotEvent::SaveDirectorySelected {
                    request,
                    enable,
                    directory,
                },
            ));
        }
    });
}

impl Job {
    fn point(&self) -> Point {
        Point {
            x: self.image.0.bounds.left,
            y: self.image.0.bounds.top,
        }
    }
}

pub(super) fn enqueue(runtime: &Arc<ScreenshotRuntime>, controller: &Arc<AppController>, job: Job) {
    let admission = runtime.auto_save.lock().unwrap().admit(job);
    let (cancel, job) = match admission {
        Admission::Start(cancel, job) => (cancel, job),
        Admission::Queued | Admission::Closed => return,
        Admission::Full(job) => {
            tracing::warn!(
                session = job.session,
                operation = job.operation,
                "Screenshot auto save queue full"
            );
            controller.ui.show_auto_save_failure(job.point());
            return;
        }
    };
    let runtime = runtime.clone();
    let controller = controller.clone();
    controller.clone().runtime.spawn_blocking(move || {
        let mut job = job;
        loop {
            let started = std::time::Instant::now();
            let point = job.point();
            let result = runtime.save.quick_save_png(
                ImageSaveRequest {
                    image: job.image.0,
                    owner: None,
                    default_name: job.name,
                    title: String::new(),
                    options: ImageSaveOptions {
                        formats: vec![ImageFormat::Png],
                        default_format: ImageFormat::Png,
                        extension_mismatch_message: String::new(),
                    },
                },
                std::path::Path::new(&job.directory),
                cancel.clone(),
            );
            tracing::debug!(
                session = job.session,
                operation = job.operation,
                saved = matches!(result, Ok(SaveOutcome::Saved)),
                failed = result.is_err(),
                elapsed_ms = started.elapsed().as_millis(),
                "Screenshot auto save finished"
            );
            if result.is_err() && !cancel.load(Ordering::Acquire) {
                controller.ui.show_auto_save_failure(point);
            }
            let mut queue = runtime.auto_save.lock().unwrap();
            if let Some(next) = queue.pending.take() {
                job = next;
            } else {
                queue.active = None;
                break;
            }
        }
    });
}

#[test]
fn auto_save_keeps_one_waiting_copy_and_cancels_it_on_exit() {
    let mut queue = Queue::default();
    let job = |operation| Job {
        session: 1,
        operation,
        name: "Lexift.png".into(),
        directory: "D:/screenshots".into(),
        image: CopiedImage(Arc::new(CapturedRegion {
            bounds: Default::default(),
            pixels: Vec::new(),
        })),
    };
    let Admission::Start(cancel, _) = queue.admit(job(1)) else {
        panic!("first copy must start");
    };
    assert!(matches!(queue.admit(job(2)), Admission::Queued));
    assert!(matches!(queue.admit(job(3)), Admission::Full(_)));
    assert_eq!(queue.pending.as_ref().unwrap().operation, 2);
    queue.close();
    assert!(cancel.load(Ordering::Acquire));
    assert!(queue.pending.is_none());
    assert!(matches!(queue.admit(job(4)), Admission::Closed));
}
