//! Image acquisition and immutable resource assembly; domain decisions remain in Core.
use super::*;
use lexift_core::domain::{
    image_history::{ImageSnapshot, workflow::OutputKey},
    image_import::{Command, Event, Target},
};
pub(super) struct Job {
    cancel: Arc<AtomicBool>,
    image: Option<Arc<CapturedRegion>>,
}
impl ScreenshotRuntime {
    pub fn execute_import(self: &Arc<Self>, controller: &Arc<AppController>, command: Command) {
        match command {
            Command::Cancel { id } => {
                if let Some(job) = self.imports.lock().unwrap().remove(&id) {
                    job.cancel.store(true, Ordering::Release);
                }
            }
            Command::Read {
                id,
                target,
                request,
            } => {
                let cancel = Arc::new(AtomicBool::new(false));
                self.imports.lock().unwrap().insert(
                    id,
                    Job {
                        cancel: cancel.clone(),
                        image: None,
                    },
                );
                let this = self.clone();
                let c = controller.clone();
                controller.runtime.spawn_blocking(move || {
                    let result = match target {
                        Target::FileEditor => this.save.open_image(request, cancel.clone()),
                        Target::ClipboardPin => this.clipboard.read_image(),
                    };
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    let result = result.map(|image| {
                        image.map(|image| {
                            let bounds = image.bounds;
                            if let Some(job) = this.imports.lock().unwrap().get_mut(&id) {
                                job.image = Some(Arc::new(image));
                            }
                            bounds
                        })
                    });
                    c.dispatch(AppEvent::ImageImport(Event::Loaded { id, result }));
                });
            }
            Command::Present { id, target } => {
                let image = self
                    .imports
                    .lock()
                    .unwrap()
                    .get(&id)
                    .and_then(|j| j.image.clone());
                let Some(base) = image else {
                    controller.dispatch(AppEvent::ImageImport(Event::Completed {
                        id,
                        result: Err(Error::new("Imported image unavailable")),
                    }));
                    return;
                };
                let this = self.clone();
                let c = controller.clone();
                controller.runtime.spawn_blocking(move || {
                    if !c
                        .state
                        .lock()
                        .unwrap()
                        .image_import
                        .request
                        .as_ref()
                        .is_some_and(|r| r.id == id)
                    {
                        return;
                    }
                    let document = history::retained_document(None, base.bounds);
                    let image = ImageSnapshot {
                        base: base.clone(),
                        samples: Arc::new(ScreenCaptureFrame {
                            regions: vec![(*base).clone()],
                            serial: id,
                            revision: 0,
                        }),
                        document,
                        hidden: false,
                    };
                    if target == Target::FileEditor {
                        // Install under the same identity lock as cancellation.
                        // Otherwise cancellation could prune the cache just before
                        // a late worker reinstalls an orphaned source allocation.
                        let owner = {
                            let state = c.state.lock().unwrap();
                            let owner = state
                                .image_import
                                .request
                                .as_ref()
                                .filter(|r| r.id == id)
                                .and_then(|r| r.editor_owner);
                            if let Some(owner) = owner {
                                this.history.lock().unwrap().install_editor(
                                    owner,
                                    owner,
                                    image.clone(),
                                );
                            }
                            owner
                        };
                        if let Some(owner) = owner {
                            c.ui.open_image_editor(owner, owner, image);
                        }
                    } else {
                        let key = OutputKey::Import { operation: id };
                        this.retain_history(&c, key, image.clone(), base.clone());
                        c.ui.show_history_pin(
                            key,
                            base,
                            image.base,
                            image.samples,
                            image.document,
                            false,
                        );
                    }
                });
            }
        }
    }
}
