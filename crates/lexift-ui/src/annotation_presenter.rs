//! Native scene submission and software handoff, independent of UI object state.
use lexift_core::{
    Error, Result,
    domain::{
        annotation::{Extra, ObjectId},
        geometry::Rect,
    },
    ports::annotation_render::{
        AnnotationRenderPort, AnnotationScene, RenderBackend, RenderLayer, RenderStamp, RenderWait,
    },
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Presenter {
    requested: Option<RenderStamp>,
    accepted: Option<Arc<AnnotationScene>>,
    confirmed: Option<RenderStamp>,
    waiting: Option<Instant>,
    required: Vec<Rect>,
    objects: Vec<ObjectId>,
    excluded: Vec<lexift_core::ports::magnifier::WindowToken>,
    pub active: bool,
    failed: bool,
    timer: slint::Timer,
}

pub(super) enum Poll {
    Waiting,
    Ready { activated: bool, pending: bool },
}

impl Presenter {
    pub fn available(&self, port: Option<&Arc<dyn AnnotationRenderPort>>) -> bool {
        !self.failed && port.is_some_and(|port| port.capabilities().backend == RenderBackend::Gpu)
    }

    pub fn submit(
        &mut self,
        port: &dyn AnnotationRenderPort,
        scene: Arc<AnnotationScene>,
    ) -> Result<()> {
        if !self.failed
            && self
                .accepted
                .as_ref()
                .is_some_and(|accepted| crate::annotation_scene::same_content(accepted, &scene))
        {
            // Preserve the accepted stamp and its timeout. The adapter retries
            // presentation/recovery and live capture without a fresh UI request.
            return Ok(());
        }
        let stamp = scene.stamp;
        let objects = scene
            .layers
            .iter()
            .filter_map(|layer| match layer {
                RenderLayer::Mosaic(layer) if !layer.masks.is_empty() => Some(layer.object.id),
                _ => None,
            })
            .filter(|id| *id != ObjectId::DRAFT)
            .collect();
        let excluded = scene.excluded_windows.to_vec();
        let required = scene.layers.iter().flat_map(|layer| match layer {
            RenderLayer::Mosaic(layer) if matches!(&layer.object.extra, Extra::Mosaic(settings) if settings.strength > 0) => layer.masks.iter().map(|mask| mask.bounds).collect::<Vec<_>>(),
            RenderLayer::Magnifier(layer) => layer.masks.iter().map(|mask| mask.bounds).collect(),
            _ => Vec::new(),
        }).collect();
        port.submit(scene.clone())?;
        self.accepted = Some(scene);
        self.required = required;
        self.objects = objects;
        self.excluded = excluded;
        self.requested = Some(stamp);
        self.waiting.get_or_insert_with(Instant::now);
        Ok(())
    }

    /// Old revisions may make progress while input continues, but a previous
    /// session or a non-presented capture cannot authorize clearing the canvas.
    pub fn poll(&mut self, port: &dyn AnnotationRenderPort) -> Result<Poll> {
        let Some(requested) = self.requested else {
            return Ok(Poll::Waiting);
        };
        let completion = port.latest()?;
        if completion.as_ref().is_some_and(|frame| {
            frame.stamp == requested
                && !frame.presented
                && matches!(frame.wait, Some(RenderWait::Recovering))
        }) {
            self.waiting.get_or_insert_with(Instant::now);
        }
        if completion.as_ref().is_some_and(|frame| {
            frame.stamp == requested
                && !frame.presented
                && matches!(frame.wait, Some(RenderWait::Paused))
        }) {
            self.waiting = None;
            return Ok(Poll::Waiting);
        }
        if let Some(frame) = completion
            && frame.presented
            && frame.stamp.session == requested.session
            && frame.stamp.revision <= requested.revision
            && (self.required.is_empty()
                || (frame.stamp == requested
                    && self.required.iter().all(|required| {
                        frame.sampled_regions.iter().any(|covered| {
                            covered.left <= required.left
                                && covered.top <= required.top
                                && covered.right >= required.right
                                && covered.bottom >= required.bottom
                        })
                    }))
                || (self.active && frame.stamp != requested))
            && self
                .confirmed
                .is_none_or(|stamp| frame.stamp.revision >= stamp.revision)
        {
            let activated = !self.active;
            self.active = true;
            if self.confirmed != Some(frame.stamp) {
                self.confirmed = Some(frame.stamp);
                self.waiting = Some(Instant::now());
            }
            let pending = frame.stamp != requested;
            if !pending {
                self.waiting = None;
            }
            self.check_timeout()?;
            return Ok(Poll::Ready { activated, pending });
        }
        self.check_timeout()?;
        Ok(Poll::Waiting)
    }

    fn check_timeout(&self) -> Result<()> {
        if self
            .waiting
            .is_some_and(|time| time.elapsed() >= Duration::from_secs(3))
        {
            return Err(Error::new("Native annotation presentation timed out"));
        }
        Ok(())
    }

    pub fn confirmed_objects(&self) -> &[ObjectId] {
        if self.confirmed == self.requested {
            &self.objects
        } else {
            &[]
        }
    }

    pub fn live(&self) -> bool {
        !self.required.is_empty()
    }

    pub fn in_use(&self) -> bool {
        self.requested.is_some() && !self.failed
    }

    pub fn exclusions_changed(
        &self,
        excluded: &[lexift_core::ports::magnifier::WindowToken],
    ) -> bool {
        self.requested.is_some() && self.excluded != excluded
    }

    pub fn arm(&self, generation: u64, callback: fn(u64)) {
        if !self.timer.running() {
            self.timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(16),
                move || callback(generation),
            );
        }
    }

    pub fn suspend(&mut self, port: Option<&Arc<dyn AnnotationRenderPort>>) {
        self.timer.stop();
        if self.requested.take().is_some()
            && let Some(port) = port
        {
            port.clear();
        }
        self.confirmed = None;
        self.accepted = None;
        self.waiting = None;
        self.required.clear();
        self.objects.clear();
        self.excluded.clear();
        self.active = false;
    }

    pub fn fail(&mut self, port: Option<&Arc<dyn AnnotationRenderPort>>) {
        if self.active || self.retaining() {
            self.timer.stop();
            self.waiting = None;
            self.active = false;
        } else {
            self.suspend(port);
        }
        self.failed = true;
    }

    /// A shown native frame remains owned until software covers every canvas.
    pub fn retaining(&self) -> bool {
        self.failed && self.requested.is_some()
    }

    pub fn finish_software_handoff(&mut self, port: Option<&Arc<dyn AnnotationRenderPort>>) {
        if self.retaining() {
            self.suspend(port);
        }
    }

    pub fn close(&mut self, port: Option<&Arc<dyn AnnotationRenderPort>>) {
        self.suspend(port);
        self.failed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::ports::annotation_render::{RenderCapabilities, RenderCompletion};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Port {
        completion: Mutex<Option<RenderCompletion>>,
        error: Mutex<Option<Error>>,
        submitted: Mutex<Vec<RenderStamp>>,
        clears: AtomicUsize,
    }

    impl AnnotationRenderPort for Port {
        fn capabilities(&self) -> RenderCapabilities {
            RenderCapabilities {
                backend: RenderBackend::Gpu,
                max_refresh_hz: 60,
            }
        }
        fn submit(&self, scene: Arc<AnnotationScene>) -> Result<()> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(error);
            }
            self.submitted.lock().unwrap().push(scene.stamp);
            Ok(())
        }
        fn latest(&self) -> Result<Option<RenderCompletion>> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(self.completion.lock().unwrap().clone())
        }
        fn clear(&self) {
            self.clears.fetch_add(1, Ordering::Relaxed);
            *self.completion.lock().unwrap() = None;
            *self.error.lock().unwrap() = None;
        }
    }

    fn scene(session: u64, revision: u64) -> Arc<AnnotationScene> {
        Arc::new(AnnotationScene {
            stamp: RenderStamp { session, revision },
            surfaces: Arc::from([]),
            layers: Arc::from([]),
            interaction: Arc::from([lexift_core::ports::annotation_render::RasterLayer {
                key: lexift_core::ports::annotation_render::RasterKey { asset: 1, revision },
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                pixels: Arc::from([0u8; 4]),
            }]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        })
    }

    fn complete(port: &Port, session: u64, revision: u64, presented: bool) {
        *port.completion.lock().unwrap() = Some(RenderCompletion {
            stamp: RenderStamp { session, revision },
            sampled_regions: Arc::from([]),
            presented,
            wait: None,
        });
    }

    #[test]
    fn exclusion_replacement_is_resubmitted_and_retired_with_session() {
        use lexift_core::ports::magnifier::WindowToken;
        let port = Port::default();
        let mut state = Presenter::default();
        let mut initial = (*scene(4, 1)).clone();
        initial.excluded_windows = vec![WindowToken(10)].into();
        state.submit(&port, Arc::new(initial.clone())).unwrap();
        assert!(!state.exclusions_changed(&[WindowToken(10)]));
        assert!(state.exclusions_changed(&[WindowToken(20)]));
        initial.stamp.revision = 2;
        initial.excluded_windows = vec![WindowToken(20)].into();
        state.submit(&port, Arc::new(initial)).unwrap();
        assert_eq!(port.submitted.lock().unwrap().len(), 2);
        assert!(!state.exclusions_changed(&[WindowToken(20)]));
        assert!(state.exclusions_changed(&[]));
        state.suspend(None);
        assert!(!state.exclusions_changed(&[WindowToken(10)]));
    }

    #[test]
    fn identical_content_keeps_the_accepted_stamp_and_pending_deadline() {
        let port = Port::default();
        let mut state = Presenter::default();
        let initial = scene(4, 1);
        state.submit(&port, initial.clone()).unwrap();
        let waiting = state.waiting;
        let mut repeat = (*initial).clone();
        repeat.stamp.revision = 2;
        state.submit(&port, Arc::new(repeat)).unwrap();
        assert_eq!(port.submitted.lock().unwrap().len(), 1);
        assert_eq!(state.requested, Some(initial.stamp));
        assert_eq!(state.waiting, waiting);
        complete(&port, 4, 1, true);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready { pending: false, .. }
        ));
        let mut repeat = (*initial).clone();
        repeat.stamp.revision = 3;
        state.submit(&port, Arc::new(repeat)).unwrap();
        assert_eq!(port.submitted.lock().unwrap().len(), 1);
        assert!(state.waiting.is_none());
        state.close(None);
        state.submit(&port, initial).unwrap();
        assert_eq!(port.submitted.lock().unwrap().len(), 2);
    }

    #[test]
    fn rejected_submission_can_retry_and_close_releases_the_accepted_scene() {
        let port = Arc::new(Port::default());
        let adapter: Arc<dyn AnnotationRenderPort> = port.clone();
        let mut state = Presenter::default();
        let initial = scene(4, 1);
        let weak = Arc::downgrade(&initial);
        *port.error.lock().unwrap() = Some(Error::new("submit failed"));
        assert!(state.submit(port.as_ref(), initial.clone()).is_err());
        assert!(state.accepted.is_none() && state.requested.is_none());
        *port.error.lock().unwrap() = None;
        state.submit(port.as_ref(), initial.clone()).unwrap();
        drop(initial);
        assert_eq!(port.submitted.lock().unwrap().len(), 1);
        state.close(Some(&adapter));
        assert!(weak.upgrade().is_none());
        assert!(state.accepted.is_none());
        state.submit(port.as_ref(), scene(4, 1)).unwrap();
        assert_eq!(port.submitted.lock().unwrap().len(), 2);
    }

    #[test]
    fn duplicate_requests_cannot_extend_recovery_deadlines() {
        let port = Port::default();
        let mut state = Presenter::default();
        let initial = scene(4, 1);
        state.submit(&port, initial.clone()).unwrap();
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        let waiting = state.waiting;
        let mut duplicate = (*initial).clone();
        duplicate.stamp.revision = 100;
        state.submit(&port, Arc::new(duplicate)).unwrap();
        assert_eq!(state.waiting, waiting);
        complete(&port, 4, 1, false);
        port.completion.lock().unwrap().as_mut().unwrap().wait = Some(RenderWait::Recovering);
        assert!(state.poll(&port).is_err());
        assert_eq!(port.submitted.lock().unwrap().len(), 1);
    }

    #[test]
    fn handoff_requires_presented_current_session_and_rejects_future_feedback() {
        let port = Port::default();
        let mut state = Presenter::default();
        state.submit(&port, scene(4, 2)).unwrap();
        for (session, revision, presented) in [(3, 2, true), (4, 3, true), (4, 2, false)] {
            complete(&port, session, revision, presented);
            assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
            assert!(!state.active);
        }
        complete(&port, 4, 2, true);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: true,
                pending: false
            }
        ));
        assert!(state.active);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: false,
                pending: false
            }
        ));
    }

    #[test]
    fn stalled_revision_times_out_but_new_feedback_advances_deadline() {
        let port = Port::default();
        let mut state = Presenter::default();
        state.submit(&port, scene(4, 1)).unwrap();
        complete(&port, 4, 1, true);
        state.poll(&port).unwrap();
        state.submit(&port, scene(4, 3)).unwrap();
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        assert!(state.poll(&port).is_err());
        complete(&port, 4, 2, true);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: false,
                pending: true
            }
        ));
        assert!(state.waiting.unwrap().elapsed() < Duration::from_secs(1));
        complete(&port, 4, 1, true);
        assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
        complete(&port, 4, 3, true);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: false,
                pending: false
            }
        ));
        assert!(state.waiting.is_none());
    }

    #[test]
    fn failure_clears_native_resources_and_close_allows_new_session() {
        let port = Arc::new(Port::default());
        let adapter: Arc<dyn AnnotationRenderPort> = port.clone();
        let mut state = Presenter::default();
        assert!(state.available(Some(&adapter)));
        state.submit(port.as_ref(), scene(4, 1)).unwrap();
        *port.error.lock().unwrap() = Some(Error::new("device removed"));
        assert!(state.poll(port.as_ref()).is_err());
        state.fail(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 1);
        assert!(!state.available(Some(&adapter)) && !state.active);
        assert!(state.requested.is_none());
        state.close(Some(&adapter));
        assert!(state.available(Some(&adapter)));
        state.submit(port.as_ref(), scene(5, 1)).unwrap();
        complete(&port, 4, 1, true);
        assert!(matches!(state.poll(port.as_ref()).unwrap(), Poll::Waiting));
        state.suspend(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 2);
        state.suspend(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn shown_failure_retains_native_until_handoff_and_close_always_releases() {
        let port = Arc::new(Port::default());
        let adapter: Arc<dyn AnnotationRenderPort> = port.clone();
        let mut state = Presenter::default();
        state.submit(port.as_ref(), scene(4, 1)).unwrap();
        complete(&port, 4, 1, true);
        state.poll(port.as_ref()).unwrap();
        state.fail(Some(&adapter));
        state.fail(Some(&adapter));
        assert!(state.retaining() && !state.active && !state.available(Some(&adapter)));
        assert_eq!(port.clears.load(Ordering::Relaxed), 0);
        state.finish_software_handoff(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 1);
        assert!(!state.retaining() && !state.available(Some(&adapter)));
        state.finish_software_handoff(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 1);
        state.close(Some(&adapter));
        state.submit(port.as_ref(), scene(5, 1)).unwrap();
        complete(&port, 5, 1, true);
        state.poll(port.as_ref()).unwrap();
        state.fail(Some(&adapter));
        state.close(Some(&adapter));
        assert_eq!(port.clears.load(Ordering::Relaxed), 2);
        assert!(!state.retaining() && state.available(Some(&adapter)));
    }

    #[test]
    fn incomplete_effect_coverage_cannot_confirm_native_handoff() {
        use lexift_core::{
            domain::annotation::{Bounds, Kind, MosaicAnnotation, MosaicEffect, Object, Style},
            ports::annotation_render::{MosaicLayer, RasterKey, RasterLayer},
        };
        let port = Port::default();
        let mut state = Presenter::default();
        let bounds = Rect {
            left: -10,
            top: -10,
            right: 10,
            bottom: 10,
        };
        let mut request = (*scene(4, 1)).clone();
        request.layers = vec![RenderLayer::Mosaic(MosaicLayer {
            object: Arc::new(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-10., -10.), (10., 10.)),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 50, 12., 1.)),
            }),
            masks: vec![RasterLayer {
                key: RasterKey {
                    asset: 1,
                    revision: 1,
                },
                bounds,
                pixels: vec![255; 1600].into(),
            }]
            .into(),
        })]
        .into();
        state.submit(&port, Arc::new(request.clone())).unwrap();
        complete(&port, 4, 1, true);
        assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
        assert!(!state.active);
        port.completion
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .sampled_regions = vec![Rect { right: 0, ..bounds }].into();
        assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
        port.completion
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .sampled_regions = vec![bounds].into();
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: true,
                pending: false
            }
        ));
        request.stamp.revision = 2;
        request.interaction = scene(4, 2).interaction.clone();
        state.submit(&port, Arc::new(request)).unwrap();
        complete(&port, 4, 2, true);
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        assert!(
            state.poll(&port).is_err(),
            "surface presentation cannot mask a stalled first effect sample"
        );
    }

    #[test]
    fn deliberate_pause_does_not_timeout_and_old_pause_cannot_extend_new_request() {
        let port = Port::default();
        let mut state = Presenter::default();
        state.submit(&port, scene(4, 1)).unwrap();
        complete(&port, 4, 1, false);
        port.completion.lock().unwrap().as_mut().unwrap().wait = Some(RenderWait::Paused);
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
        assert!(state.waiting.is_none() && !state.active);
        state.submit(&port, scene(4, 2)).unwrap();
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        assert!(state.poll(&port).is_err());
    }

    #[test]
    fn recovery_wait_has_a_deadline_after_an_active_scene() {
        let port = Port::default();
        let mut state = Presenter::default();
        state.submit(&port, scene(4, 1)).unwrap();
        complete(&port, 4, 1, true);
        state.poll(&port).unwrap();
        assert!(state.active && state.waiting.is_none());
        complete(&port, 4, 1, false);
        port.completion.lock().unwrap().as_mut().unwrap().wait = Some(RenderWait::Recovering);
        assert!(matches!(state.poll(&port).unwrap(), Poll::Waiting));
        assert!(state.waiting.is_some() && state.active);
        state.waiting = Some(Instant::now() - Duration::from_secs(4));
        assert!(state.poll(&port).is_err());
        complete(&port, 4, 1, true);
        assert!(matches!(
            state.poll(&port).unwrap(),
            Poll::Ready {
                activated: false,
                pending: false
            }
        ));
        assert!(state.waiting.is_none());
    }
}
