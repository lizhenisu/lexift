//! Immutable image assets retained independently of screenshot and pin windows.
use super::*;
use lexift_core::domain::annotation_export::ExportSnapshot;
use lexift_core::domain::image_history::{
    self as history, Entry, Kind, Resource, ResourceId, Ticket,
    workflow::{Event, OutputKey, Workflow},
};
use std::sync::Weak;

#[derive(Clone)]
pub(super) struct Asset {
    pub base: Arc<CapturedRegion>,
    pub samples: Arc<ScreenCaptureFrame>,
    pub document: ExportRequest,
    pub hidden: bool,
    pub output: Arc<CapturedRegion>,
    pub preview: Arc<CapturedRegion>,
}

/// Weak identity tables never keep pixels alive and protect against pointer reuse.
#[derive(Default)]
pub(super) struct Cache {
    revision: u64,
    editor: Option<(u64, u64, history::ImageSnapshot)>,
    next_resource: u64,
    pixels: Vec<(Weak<CapturedRegion>, ResourceId)>,
    samples: Vec<(Weak<ScreenCaptureFrame>, ResourceId)>,
    documents: Vec<(Weak<ExportSnapshot>, ResourceId)>,
    pub assets: BTreeMap<Ticket, Arc<Asset>>,
    jobs: BTreeMap<OutputKey, Arc<AtomicBool>>,
}

fn identity<T>(
    items: &mut Vec<(Weak<T>, ResourceId)>,
    next: &mut u64,
    value: &Arc<T>,
) -> ResourceId {
    items.retain(|(weak, _)| weak.strong_count() > 0);
    if let Some((_, id)) = items
        .iter()
        .find(|(weak, _)| weak.ptr_eq(&Arc::downgrade(value)))
    {
        return *id;
    }
    *next = next
        .checked_add(1)
        .expect("image resource identity exhausted");
    let id = ResourceId(*next);
    items.push((Arc::downgrade(value), id));
    id
}

impl Cache {
    pub fn sync(&mut self, workflow: &Workflow) {
        if self.revision != workflow.cache.revision() {
            self.revision = workflow.cache.revision();
            tracing::debug!(
                entries = workflow.cache.entries().len(),
                retained_bytes = workflow.cache.retained_bytes(),
                "Image history cache changed"
            );
        }
        self.jobs.retain(|key, cancel| {
            let keep = workflow.ticket(*key).is_some();
            if !keep {
                cancel.store(true, Ordering::Release);
            }
            keep
        });
        self.assets.retain(|ticket, _| workflow.retains(*ticket));
        if self.editor.as_ref().is_some_and(|(window, id, _)| {
            !workflow
                .editor
                .as_ref()
                .is_some_and(|e| e.window == *window && e.id == *id)
        }) {
            self.editor = None;
        }
        self.pixels.retain(|(weak, _)| weak.strong_count() > 0);
        self.samples.retain(|(weak, _)| weak.strong_count() > 0);
        self.documents.retain(|(weak, _)| weak.strong_count() > 0);
    }

    fn insert(&mut self, ticket: Ticket, kind: Kind, asset: Asset) -> Entry {
        use std::mem::size_of;
        let mut resources = Vec::new();
        for image in [&asset.base, &asset.output, &asset.preview] {
            resources.push(Resource {
                id: identity(&mut self.pixels, &mut self.next_resource, image),
                // The preview also reserves the UI's retained Slint pixel copy.
                bytes: (size_of::<CapturedRegion>() + image.pixels.capacity()) as u64
                    + if Arc::ptr_eq(image, &asset.preview) {
                        image.pixels.len() as u64
                    } else {
                        0
                    },
            });
        }
        resources.push(Resource {
            id: identity(&mut self.samples, &mut self.next_resource, &asset.samples),
            bytes: (size_of::<ScreenCaptureFrame>()
                + asset.samples.regions.capacity() * size_of::<CapturedRegion>()
                + asset
                    .samples
                    .regions
                    .iter()
                    .map(|r| r.pixels.capacity())
                    .sum::<usize>()) as u64,
        });
        let snapshot = &asset.document.snapshot;
        resources.push(Resource {
            id: identity(&mut self.documents, &mut self.next_resource, snapshot),
            bytes: snapshot.session.image_snapshot_bytes()
                + (size_of::<ExportSnapshot>()
                    + snapshot.monitors.capacity()
                        * size_of::<(lexift_core::domain::geometry::Rect, f32)>()
                    + snapshot.excluded.capacity()
                        * size_of::<lexift_core::ports::magnifier::WindowToken>()
                    + snapshot.name.capacity()
                    + snapshot.title.capacity()) as u64,
        });
        let entry = Entry {
            ticket,
            kind,
            hidden: asset.hidden,
            bounds: asset.output.bounds,
            updated_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis().min(u128::from(u64::MAX)) as u64),
            resources,
        };
        self.assets.insert(ticket, Arc::new(asset));
        entry
    }
}

impl ScreenshotRuntime {
    pub fn history_previews(&self, workflow: &Workflow) -> Vec<(Entry, Arc<CapturedRegion>)> {
        let cache = self.history.lock().unwrap();
        workflow
            .cache
            .entries()
            .iter()
            .filter_map(|e| {
                cache
                    .assets
                    .get(&e.ticket)
                    .map(|a| (e.clone(), a.preview.clone()))
            })
            .collect()
    }

    pub fn execute_history(
        self: &Arc<Self>,
        controller: &Arc<AppController>,
        command: history::workflow::Command,
    ) {
        use history::workflow::{Action, Command};
        match command {
            Command::Cancel { window } => {
                self.history.lock().unwrap().jobs.retain(|key, cancel| {
                    let keep = !matches!(key, OutputKey::History { window:id,.. } if *id == window);
                    if !keep {
                        cancel.store(true, Ordering::Release);
                    }
                    keep
                });
            }
            Command::EditorOutput {
                window,
                editor,
                operation,
                kind,
                document,
            } => {
                let context = controller
                    .state
                    .lock()
                    .unwrap()
                    .image_history
                    .editor
                    .clone()
                    .filter(|e| {
                        e.window == window && e.id == editor && e.operation == operation && e.busy
                    });
                let asset = self
                    .history
                    .lock()
                    .unwrap()
                    .editor
                    .as_ref()
                    .filter(|(w, id, _)| *w == window && *id == editor)
                    .map(|(_, _, image)| image.clone());
                let Some((context, asset)) = context.zip(asset) else {
                    controller.dispatch(AppEvent::ImageHistory(Event::EditorCompleted {
                        window,
                        editor,
                        operation,
                        result: Err(Error::new("History image unavailable")),
                    }));
                    return;
                };
                let key = OutputKey::Editor {
                    window,
                    editor,
                    operation,
                };
                let cancel = Arc::new(AtomicBool::new(false));
                self.history
                    .lock()
                    .unwrap()
                    .jobs
                    .insert(key, cancel.clone());
                let this = self.clone();
                let c = controller.clone();
                controller.runtime.spawn_blocking(move || {
                    let result = (|| {
                        let output = Arc::new(this.compose_pin(
                            &asset.base,
                            Some(asset.samples.clone()),
                            Some(&document),
                            context.hidden,
                            &cancel,
                        )?);
                        if cancel.load(Ordering::Acquire) {
                            return Ok(SaveOutcome::Cancelled);
                        }
                        let result = match kind {
                            ScreenshotOutput::Copy => this.output_pin_image(
                                &output,
                                PinOutput::Copy,
                                document.snapshot.owner,
                                document.snapshot.title.clone(),
                                &cancel,
                            ),
                            ScreenshotOutput::Save => this.output_pin_image(
                                &output,
                                PinOutput::Save,
                                document.snapshot.owner,
                                document.snapshot.title.clone(),
                                &cancel,
                            ),
                            ScreenshotOutput::Pin => Ok(SaveOutcome::Saved),
                        };
                        if matches!(result, Ok(SaveOutcome::Saved))
                            && !cancel.load(Ordering::Acquire)
                        {
                            this.retain_history(
                                &c,
                                key,
                                history::ImageSnapshot {
                                    base: asset.base.clone(),
                                    samples: asset.samples.clone(),
                                    document: document.clone(),
                                    hidden: context.hidden,
                                },
                                output.clone(),
                            );
                            if kind == ScreenshotOutput::Pin {
                                c.ui.show_history_pin(
                                    key,
                                    output,
                                    asset.base.clone(),
                                    asset.samples.clone(),
                                    document,
                                    context.hidden,
                                );
                                return Ok(SaveOutcome::Cancelled);
                            }
                        }
                        result
                    })();
                    if kind != ScreenshotOutput::Pin || result.is_err() {
                        c.dispatch(AppEvent::ImageHistory(Event::EditorCompleted {
                            window,
                            editor,
                            operation,
                            result,
                        }));
                    }
                });
            }
            Command::Output {
                window,
                operation,
                ticket,
                action,
                owner,
                title,
            } => {
                let asset = self.history.lock().unwrap().assets.get(&ticket).cloned();
                let Some(asset) = asset else {
                    controller.dispatch(AppEvent::ImageHistory(Event::Completed {
                        window,
                        operation,
                        result: Err(Error::new("History image unavailable")),
                    }));
                    return;
                };
                if action == Action::Edit {
                    let image = history::ImageSnapshot {
                        base: asset.base.clone(),
                        samples: asset.samples.clone(),
                        document: asset.document.clone(),
                        hidden: asset.hidden,
                    };
                    self.history.lock().unwrap().editor = Some((window, operation, image.clone()));
                    controller.ui.open_history_editor(window, operation, image);
                    return;
                }
                let cancel = Arc::new(AtomicBool::new(false));
                self.history
                    .lock()
                    .unwrap()
                    .jobs
                    .insert(OutputKey::History { window, operation }, cancel.clone());
                let this = self.clone();
                let c = controller.clone();
                controller.runtime.spawn_blocking(move || {
                    if action == Action::Pin {
                        let key = OutputKey::History { window, operation };
                        let next = c.state.lock().unwrap().image_history.ticket(key);
                        if let Some((next, kind)) = next.filter(|_| !cancel.load(Ordering::Acquire))
                        {
                            let entry =
                                this.history
                                    .lock()
                                    .unwrap()
                                    .insert(next, kind, (*asset).clone());
                            c.dispatch(AppEvent::ImageHistory(Event::Prepared { key, entry }));
                            c.ui.show_history_pin(
                                key,
                                asset.output.clone(),
                                asset.base.clone(),
                                asset.samples.clone(),
                                asset.document.clone(),
                                asset.hidden,
                            );
                        }
                        return;
                    }
                    let result = (|| {
                        if cancel.load(Ordering::Acquire) {
                            return Ok(SaveOutcome::Cancelled);
                        }
                        match action {
                            Action::Copy => {
                                this.write_image(&asset.output, &cancel)?;
                                Ok(SaveOutcome::Saved)
                            }
                            Action::Save => this.save.save_png(
                                PngSaveRequest {
                                    image: (*asset.output).clone(),
                                    owner,
                                    default_name: "Lexift.png".into(),
                                    title,
                                },
                                cancel.clone(),
                            ),
                            Action::Edit | Action::Pin => {
                                unreachable!("handled before system output")
                            }
                        }
                    })();
                    if matches!(result, Ok(SaveOutcome::Saved)) && !cancel.load(Ordering::Acquire) {
                        let key = OutputKey::History { window, operation };
                        let next = c.state.lock().unwrap().image_history.ticket(key);
                        if let Some((next, kind)) = next {
                            let entry =
                                this.history
                                    .lock()
                                    .unwrap()
                                    .insert(next, kind, (*asset).clone());
                            c.dispatch(AppEvent::ImageHistory(Event::Prepared { key, entry }));
                        }
                    }
                    c.dispatch(AppEvent::ImageHistory(Event::Completed {
                        window,
                        operation,
                        result,
                    }));
                });
            }
        }
    }
    pub fn sync_history(&self, workflow: &Workflow) {
        self.history
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .sync(workflow);
    }

    /// Workers retain only the bounded source and samples needed to restore effects.
    /// No whole-desktop capture is added to the cache for a static screenshot.
    pub(super) fn retain_history(
        &self,
        controller: &Arc<AppController>,
        key: OutputKey,
        source: history::ImageSnapshot,
        output: Arc<CapturedRegion>,
    ) {
        let Some((ticket, kind)) = controller.state.lock().unwrap().image_history.ticket(key)
        else {
            return;
        };
        let result = (|| -> Result<Entry> {
            let started = std::time::Instant::now();
            let preview = Arc::new(history::thumbnail::create(&output, 1000, 800)?);
            let history::ImageSnapshot {
                base,
                samples,
                document,
                hidden,
            } = source;
            let document = retained_document(Some(&document), base.bounds);
            let asset = Asset {
                base,
                samples,
                document,
                hidden,
                output,
                preview,
            };
            let entry = self.history.lock().unwrap().insert(ticket, kind, asset);
            tracing::debug!(
                origin = ticket.origin.0,
                version = ticket.version,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
                "Image history resources prepared"
            );
            Ok(entry)
        })();
        controller.dispatch(AppEvent::ImageHistory(match result {
            Ok(entry) => Event::Prepared { key, entry },
            Err(_) => Event::Unavailable { key, ticket },
        }));
    }
}

pub(super) fn retained_document(
    document: Option<&ExportRequest>,
    bounds: lexift_core::domain::geometry::Rect,
) -> ExportRequest {
    let mut session = match document {
        Some(document) => document.snapshot.session.restored_image(),
        None => lexift_core::domain::annotation::Session::new(),
    };
    session.image_source = Some(bounds);
    let snapshot = document
        .map(|d| (*d.snapshot).clone())
        .unwrap_or_else(|| ExportSnapshot {
            session: Arc::new(lexift_core::domain::annotation::Session::new()),
            monitor: bounds,
            scale: 1.,
            monitors: vec![(bounds, 1.)],
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        });
    ExportRequest {
        session: 0,
        request: 0,
        snapshot: Arc::new(ExportSnapshot {
            session: Arc::new(session),
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
            ..snapshot
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::{annotation::Session, geometry::Rect};
    fn asset() -> Asset {
        let base = Arc::new(CapturedRegion {
            bounds: Rect {
                left: -10,
                top: -5,
                right: 10,
                bottom: 35,
            },
            pixels: [40, 80, 120, 255].repeat(800),
        });
        let mut session = Session::new();
        session.image_source = Some(base.bounds);
        session.set_image_crop(
            Rect {
                left: -5,
                top: 5,
                right: 5,
                bottom: 25,
            },
            base.bounds,
        );
        let request = ExportRequest {
            session: 10,
            request: 20,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(session),
                monitor: base.bounds,
                scale: 1.25,
                monitors: vec![(base.bounds, 1.25)],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        let document = retained_document(Some(&request), base.bounds);
        let output = Arc::new(
            crop_frames(
                std::slice::from_ref(base.as_ref()),
                document.snapshot.session.image_crop.unwrap(),
            )
            .unwrap(),
        );
        Asset {
            preview: Arc::new(history::thumbnail::create(&output, 1000, 800).unwrap()),
            base,
            output,
            document,
            hidden: true,
            samples: Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: vec![],
            }),
        }
    }
    #[test]
    fn identical_shared_assets_are_charged_once_and_clear_releases_cache_references() {
        let mut cache = Cache::default();
        let mut workflow = Workflow::default();
        let a = asset();
        let pixels = Arc::downgrade(&a.base);
        let origin = workflow.cache.origin().unwrap();
        let ticket = workflow.cache.begin(origin).unwrap();
        let entry = cache.insert(ticket, Kind::LongImage, a.clone());
        assert!(matches!(
            workflow.cache.complete(entry),
            history::Admission::Accepted { .. }
        ));
        let bytes = workflow.cache.retained_bytes();
        let other = workflow.cache.origin().unwrap();
        let ticket2 = workflow.cache.begin(other).unwrap();
        let entry = cache.insert(ticket2, Kind::LongImage, a.clone());
        assert!(matches!(
            workflow.cache.complete(entry),
            history::Admission::Accepted { .. }
        ));
        assert_eq!(workflow.cache.retained_bytes(), bytes);
        assert!(bytes > a.base.pixels.len() as u64);
        cache.sync(&workflow);
        assert_eq!(cache.assets.len(), 2);
        workflow.cache.clear();
        cache.sync(&workflow);
        assert!(cache.assets.is_empty());
        assert!(
            pixels.upgrade().is_some(),
            "active editor/pin ownership outlives history"
        );
        drop(a);
        assert!(pixels.upgrade().is_none());
    }
    #[test]
    fn active_editor_keeps_source_but_not_deleted_history_output_or_preview() {
        let mut cache = Cache::default();
        let mut workflow = Workflow::default();
        let a = asset();
        let base = Arc::downgrade(&a.base);
        let output = Arc::downgrade(&a.output);
        let preview = Arc::downgrade(&a.preview);
        let origin = workflow.cache.origin().unwrap();
        let ticket = workflow.cache.begin(origin).unwrap();
        let source = history::ImageSnapshot {
            base: a.base.clone(),
            samples: a.samples.clone(),
            document: a.document.clone(),
            hidden: a.hidden,
        };
        let entry = cache.insert(ticket, Kind::LongImage, a);
        workflow.cache.complete(entry);
        workflow.editor = Some(history::workflow::Editor {
            ready: true,
            failed: false,
            hidden: true,
            window: 1,
            id: 2,
            ticket,
            kind: Kind::LongImage,
            operation: 0,
            busy: false,
        });
        cache.editor = Some((1, 2, source));
        workflow.cache.clear();
        cache.sync(&workflow);
        assert!(cache.assets.is_empty());
        assert!(output.upgrade().is_none());
        assert!(preview.upgrade().is_none());
        assert!(base.upgrade().is_some());
        workflow.editor = None;
        cache.sync(&workflow);
        assert!(base.upgrade().is_none());
    }
    #[test]
    fn retained_document_preserves_crop_origin_visibility_and_fresh_history() {
        let a = asset();
        assert_eq!(
            a.document.snapshot.session.image_source,
            Some(a.base.bounds)
        );
        assert_eq!(
            a.document.snapshot.session.image_crop,
            Some(a.output.bounds)
        );
        assert_eq!(a.document.snapshot.scale, 1.25);
        assert!(!a.document.snapshot.session.can_undo());
        assert!(!a.document.snapshot.session.can_redo());
        assert!(a.document.snapshot.owner.is_none());
        assert!(a.hidden);
        assert_eq!(a.preview.pixels, a.output.pixels);
        assert_eq!(
            a.output.pixels,
            crop_frames(std::slice::from_ref(a.base.as_ref()), a.output.bounds)
                .unwrap()
                .pixels
        );
    }
}
