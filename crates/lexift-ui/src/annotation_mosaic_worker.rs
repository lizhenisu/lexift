//! Latest-only effect processing. No Slint component or native window crosses threads.
use super::mosaic::{self, Tile};
use lexift_core::{
    domain::{
        annotation::{Extra, Session},
        geometry::Rect,
    },
    ports::screen_capture::ScreenCaptureFrame,
};
use std::sync::{Arc, Condvar, Mutex};

pub(crate) struct Frame {
    pub key: u64,
    pub context: u64,
    pub serial: u64,
    pub tiles: Vec<Tile>,
    pub snapshot: Arc<Session>,
    pub complete: bool,
}
/// Current drag feedback can reuse a completed sample, never an old callback.
#[derive(Clone, Copy)]
pub(crate) struct View<'a> {
    pub frame: &'a Frame,
    pub preview: Option<(usize, &'a lexift_core::domain::annotation::Object)>,
    pub remask: bool,
}
impl<'a> From<&'a Frame> for View<'a> {
    fn from(frame: &'a Frame) -> Self {
        Self {
            frame,
            preview: None,
            remask: false,
        }
    }
}

impl View<'_> {
    /// Only sampled tiles that can affect this monitor invalidate its raster.
    /// Include the original and current bounds during drag feedback, and source
    /// tiles seen through a later lens whose output lies on this monitor.
    #[cfg(test)]
    pub fn raster_signature<'a>(
        self,
        monitor: Rect,
        document: impl Into<crate::annotation_scene::Document<'a>>,
        draft: Option<&'a lexift_core::domain::annotation::Object>,
    ) -> u64 {
        self.raster_signature_with(
            monitor,
            &crate::annotation_signature::Frame::new(document, draft),
        )
    }

    pub fn raster_signature_with(
        self,
        monitor: Rect,
        signatures: &crate::annotation_signature::Frame<'_>,
    ) -> u64 {
        let document = signatures.document();
        let draft = signatures.draft();
        use std::hash::{Hash, Hasher};
        let intersects = |bounds: lexift_core::domain::annotation::Bounds| {
            bounds.right + 1. > monitor.left as f32
                && bounds.left - 1. < monitor.right as f32
                && bounds.bottom + 1. > monitor.top as f32
                && bounds.top - 1. < monitor.bottom as f32
        };
        let overlaps = |a: lexift_core::domain::annotation::Bounds,
                        b: lexift_core::domain::annotation::Bounds| {
            a.right + 1. > b.left
                && a.left - 1. < b.right
                && a.bottom + 1. > b.top
                && a.top - 1. < b.bottom
        };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let mut relevant = false;
        for tile in &self.frame.tiles {
            let current = document
                .get(tile.index)
                .or_else(|| (tile.index == document.len()).then_some(draft).flatten());
            let current_bounds = current.map_or(tile.object.bounds, |o| o.bounds);
            let through_lens = document.objects().chain(draft).enumerate().any(|(index, lens)| {
                matches!(lens.extra, Extra::Magnifier { output, erase_annotations: false, .. }
                    if index > tile.index && intersects(output)
                        && (overlaps(lens.bounds, tile.object.bounds) || overlaps(lens.bounds, current_bounds)))
            });
            if intersects(tile.object.bounds) || intersects(current_bounds) || through_lens {
                relevant = true;
                tile.index.hash(&mut hash);
                tile.raster_key.hash(&mut hash);
                let owns_preview = self.preview.is_some_and(|(index, original)| {
                    index == tile.index && original == &tile.object
                });
                owns_preview.hash(&mut hash);
                (owns_preview && self.remask).hash(&mut hash);
                if let Some(key) = signatures.object_key(tile.index) {
                    key.hash(&mut hash);
                }
            }
        }
        if relevant { hash.finish() } else { 0 }
    }
}

/// One immutable document snapshot shared by successive capture jobs. Exact
/// content comparison includes off-screen objects and display metadata: the
/// viewport fingerprint is not a complete document identity.
#[derive(Default)]
pub(crate) struct Snapshot {
    current: Option<Arc<Session>>,
}
impl Snapshot {
    pub fn get<'a>(
        &mut self,
        document: impl Into<crate::annotation_scene::Document<'a>>,
        draft: Option<&lexift_core::domain::annotation::Object>,
    ) -> Arc<Session> {
        let document = document.into();
        let session = document.session;
        let matches = self.current.as_ref().is_some_and(|snapshot| {
            snapshot.objects.len() == document.len() + usize::from(draft.is_some())
                && snapshot
                    .objects
                    .iter()
                    .take(document.len())
                    .eq(document.objects())
                && snapshot.objects.get(document.len()) == draft
                && snapshot.watermark == session.watermark
                && snapshot.displays == session.displays
                && snapshot.spotlight_opacity.to_bits() == session.spotlight_opacity.to_bits()
        });
        if !matches {
            let mut snapshot = Session::new();
            snapshot.objects = document.objects().cloned().collect();
            snapshot.objects.extend(draft.into_iter().cloned());
            snapshot.watermark = session.watermark.clone();
            snapshot.displays = session.displays.clone();
            snapshot.spotlight_opacity = session.spotlight_opacity;
            self.current = Some(Arc::new(snapshot));
        }
        self.current.as_ref().unwrap().clone()
    }
    pub fn is_current(&self, snapshot: &Arc<Session>) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, snapshot))
    }

    pub fn clear(&mut self) {
        self.current = None;
    }
}

pub(crate) struct Job {
    pub key: u64,
    pub context: u64,
    pub session: Arc<Session>,
    pub provisional_index: Option<usize>,
    pub monitors: Vec<(Rect, f32)>,
    pub capture: Arc<ScreenCaptureFrame>,
}
#[cfg(test)]
impl Job {
    /// Editing a fixture leaves any earlier result's snapshot frozen.
    /// Production jobs have no mutable document access.
    fn session_mut(&mut self) -> &mut Session {
        if Arc::get_mut(&mut self.session).is_none() {
            let mut snapshot = Session::new();
            snapshot.objects = self.session.objects.clone();
            snapshot.displays = self.session.displays.clone();
            snapshot.watermark = self.session.watermark.clone();
            snapshot.spotlight_opacity = self.session.spotlight_opacity;
            snapshot.next_sequence = self.session.next_sequence;
            snapshot.selected = self.session.selected;
            self.session = Arc::new(snapshot);
        }
        Arc::get_mut(&mut self.session).unwrap()
    }
}

#[derive(Default)]
struct Mailbox {
    job: Option<Job>,
    result: Option<Arc<Frame>>,
    context: u64,
    stopped: bool,
}
pub(crate) struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
    submitted: Option<(u64, u64, u64, Arc<Session>)>,
}
impl Default for Worker {
    fn default() -> Self {
        let mut scratch = mosaic::Scratch::default();
        Self::start(move |job, cancelled| process(job, cancelled, &mut scratch))
    }
}
impl Worker {
    fn start(
        mut process_job: impl FnMut(&Job, &dyn Fn() -> bool) -> Option<Frame> + Send + 'static,
    ) -> Self {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let state = shared.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let (lock, wake) = &*state;
                let job = {
                    let mut mailbox = lock.lock().unwrap();
                    while mailbox.job.is_none() && !mailbox.stopped {
                        mailbox = wake.wait(mailbox).unwrap();
                    }
                    if mailbox.stopped {
                        break;
                    }
                    mailbox.job.take().unwrap()
                };
                let cancelled = || {
                    let mailbox = lock.lock().unwrap();
                    mailbox.stopped || mailbox.context != job.context
                };
                if let Some(frame) = process_job(&job, &cancelled) {
                    let mut mailbox = lock.lock().unwrap();
                    if !mailbox.stopped && mailbox.context == frame.context {
                        mailbox.result = Some(Arc::new(frame));
                    }
                }
            }
        });
        Self {
            shared,
            thread: Some(thread),
            submitted: None,
        }
    }
}
impl Worker {
    /// Visible fingerprints cannot stand in for the full sampling document.
    pub fn needs_submission(
        &self,
        key: u64,
        context: u64,
        serial: u64,
        snapshot: &Arc<Session>,
    ) -> bool {
        self.submitted
            .as_ref()
            .is_none_or(|(old_key, old_context, old_serial, old_snapshot)| {
                (*old_key, *old_context, *old_serial) != (key, context, serial)
                    || !Arc::ptr_eq(old_snapshot, snapshot)
            })
    }
    pub fn submit(&mut self, job: Job) {
        let identity = (
            job.key,
            job.context,
            job.capture.serial,
            job.session.clone(),
        );
        if !self.needs_submission(job.key, job.context, job.capture.serial, &job.session) {
            return;
        }
        self.submitted = Some(identity);
        let (lock, wake) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        mailbox.context = job.context;
        mailbox.job = Some(job);
        wake.notify_one();
    }
    pub fn latest(&self, context: u64) -> Option<Arc<Frame>> {
        let mailbox = self.shared.0.lock().unwrap();
        if mailbox.context != context {
            return None;
        }
        mailbox
            .result
            .as_ref()
            .filter(|f| f.context == context)
            .cloned()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        {
            let mut mailbox = lock.lock().unwrap();
            mailbox.stopped = true;
            mailbox.job = None;
            mailbox.result = None;
            wake.notify_one();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// Render every monitor intersection into the sampling ROI, including marks on
/// the other side of a DPI boundary. Earlier effects are composited once.
fn process(
    job: &Job,
    cancelled: &dyn Fn() -> bool,
    scratch: &mut mosaic::Scratch,
) -> Option<Frame> {
    let mut result = Frame {
        key: job.key,
        context: job.context,
        serial: job.capture.serial,
        tiles: Vec::new(),
        snapshot: job.session.clone(),
        complete: job.provisional_index.is_none(),
    };
    for (index, object) in job.session.objects.iter().enumerate() {
        if cancelled() {
            return None;
        }
        if matches!(&object.extra,Extra::Mosaic(m) if m.strength > 0) {
            let monitors: Vec<_> = job.monitors.iter().map(|(bounds, _)| *bounds).collect();
            let regions = mosaic::work_regions(object, &monitors);
            let previous_tiles = result.tiles.len();
            let partial = job.provisional_index == Some(index);
            for (body, bounds) in &regions {
                let bounds = *bounds;
                let Some(source) = job
                    .capture
                    .regions
                    .iter()
                    .find(|r| {
                        r.bounds.left <= bounds.left
                            && r.bounds.top <= bounds.top
                            && r.bounds.right >= bounds.right
                            && r.bounds.bottom >= bounds.bottom
                    })
                    .or_else(|| {
                        (job.provisional_index == Some(index))
                            .then(|| {
                                job.capture
                                    .regions
                                    .iter()
                                    .filter_map(|r| {
                                        let overlap = mosaic::intersect(bounds, r.bounds)?;
                                        Some((
                                            r,
                                            (overlap.right as i64 - overlap.left as i64)
                                                * (overlap.bottom as i64 - overlap.top as i64),
                                        ))
                                    })
                                    .max_by_key(|(_, area)| *area)
                                    .map(|(r, _)| r)
                            })
                            .flatten()
                    })
                else {
                    if partial { continue } else { return None }
                };
                let roi = mosaic::intersect(bounds, source.bounds)?;
                let mut overlay = tiny_skia::Pixmap::new(
                    (roi.right - roi.left) as u32,
                    (roi.bottom - roi.top) as u32,
                )?;
                for &(monitor, scale) in &job.monitors {
                    if cancelled() {
                        return None;
                    }
                    if let Some(shared) = mosaic::intersect(roi, monitor) {
                        let marks = super::render_captured_effects(
                            &job.session,
                            index,
                            shared,
                            scale,
                            (&result).into(),
                            &job.capture,
                        )?;
                        overlay.draw_pixmap(
                            shared.left - roi.left,
                            shared.top - roi.top,
                            marks.as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
                let Some(mut tile) =
                    mosaic::prepare(&overlay, object, roi, &job.capture, cancelled, scratch)
                else {
                    if partial { continue } else { return None }
                };
                tile.index = index;
                tile.visible_monitors = job.monitors.iter().map(|(bounds, _)| *bounds).collect();
                if mosaic::clip_output(&mut tile, *body).is_none() {
                    if partial { continue } else { return None }
                }
                mosaic::set_snapshot_feedback(&mut tile, &job.capture, object, &monitors);
                tile.finalize_raster_key();
                result.tiles.push(tile);
            }
            if !regions.is_empty() && result.tiles.len() == previous_tiles {
                return None;
            }
        }
    }
    (!cancelled()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::annotation::{Bounds, Kind, MosaicAnnotation, MosaicEffect, Object, Style},
        ports::screen_capture::CapturedRegion,
    };
    fn job(key: u64) -> Job {
        let region = Rect {
            left: -8,
            top: -8,
            right: 8,
            bottom: 8,
        };
        let mut session = Session::new();
        session.objects.push(Object {
            id: Default::default(),
            bounds: Bounds::from_corners((-8., -8.), (8., 8.)),
            kind: Kind::Rectangle,
            style: Style {
                color: [200, 0, 0],
                fill: true,
                outline: false,
                rounding: 0.,
                ..Style::default()
            },
            points: Vec::new(),
            extra: Extra::None,
        });
        session.objects.push(Object {
            id: Default::default(),
            bounds: Bounds::from_corners((-4., -4.), (4., 4.)),
            kind: Kind::MosaicArea,
            style: Style {
                rounding: 0.,
                ..Style::default()
            },
            points: Vec::new(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 12., 1.)),
        });
        if let Extra::Mosaic(m) = &mut session.objects[1].extra {
            m.id = 1;
        }
        Job {
            key,
            context: key,
            session: Arc::new(session),
            provisional_index: None,
            monitors: vec![
                (Rect { right: 0, ..region }, 1.),
                (Rect { left: 0, ..region }, 1.25),
            ],
            capture: Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: vec![CapturedRegion {
                    bounds: region,
                    pixels: [0, 100, 0, 255].repeat(256),
                }],
            }),
        }
    }
    #[test]
    fn shared_snapshot_reuses_capture_only_documents_and_freezes_old_versions() {
        let mut source = job(7);
        source.session_mut().objects[0].kind = Kind::Pencil;
        source.session_mut().objects[0].points = (0..20_000)
            .map(|i| (-1000. - (i % 300) as f32, -1000. - (i / 300) as f32))
            .collect();
        source.session_mut().objects[0].refresh_bounds();
        let mut cache = Snapshot::default();
        let first = cache.get(source.session.as_ref(), None);
        let next_capture = cache.get(source.session.as_ref(), None);
        assert!(Arc::ptr_eq(&first, &next_capture));
        assert_eq!(
            first.objects[0].points.as_ptr(),
            next_capture.objects[0].points.as_ptr()
        );
        source.session_mut().selected = Some(1);
        assert!(Arc::ptr_eq(
            &first,
            &cache.get(source.session.as_ref(), None)
        ));
        assert!(
            first.selected.is_none(),
            "sampling excludes selection state"
        );

        // This point is off screen, so a viewport-only key cannot validate it.
        let original_point = first.objects[0].points[0];
        source.session_mut().objects[0].points[0].0 -= 50.;
        let changed = cache.get(source.session.as_ref(), None);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(first.objects[0].points[0], original_point);
        assert_eq!(
            changed.objects[0].points[0],
            source.session.objects[0].points[0]
        );
        source.session_mut().displays.push(Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        });
        let displays = cache.get(source.session.as_ref(), None);
        assert!(!Arc::ptr_eq(&changed, &displays));
        assert!(changed.displays.is_empty());
        source.session_mut().spotlight_opacity = 0.75;
        let backdrop = cache.get(source.session.as_ref(), None);
        assert!(!Arc::ptr_eq(&displays, &backdrop));
        assert_eq!(backdrop.spotlight_opacity, 0.75);

        source.session_mut().watermark = Some(lexift_core::domain::annotation::Watermark {
            template: "水印".into(),
            content: "水印".into(),
            font_family: "Arial".into(),
            font_size: 20.,
            opacity: 180,
            color: [20, 80, 160],
            position: lexift_core::domain::annotation::WatermarkPosition::Center,
        });
        let watermark = cache.get(source.session.as_ref(), None);
        assert!(!Arc::ptr_eq(&backdrop, &watermark));
        assert!(backdrop.watermark.is_none());
        assert_eq!(watermark.watermark.as_ref().unwrap().content, "水印");

        let mut draft = source.session.objects[1].clone();
        let draft_snapshot = cache.get(source.session.as_ref(), Some(&draft));
        assert!(Arc::ptr_eq(
            &draft_snapshot,
            &cache.get(source.session.as_ref(), Some(&draft))
        ));
        assert_eq!(
            draft_snapshot.objects.len(),
            source.session.objects.len() + 1
        );
        draft.bounds.right += 4.;
        let updated_draft = cache.get(source.session.as_ref(), Some(&draft));
        assert!(!Arc::ptr_eq(&draft_snapshot, &updated_draft));
        assert_ne!(
            draft_snapshot.objects.last().unwrap().bounds,
            updated_draft.objects.last().unwrap().bounds
        );
        let cancelled = cache.get(source.session.as_ref(), None);
        assert_eq!(cancelled.objects.len(), source.session.objects.len());
        let released = Arc::downgrade(&cancelled);
        drop(cancelled);
        cache.clear();
        assert!(released.upgrade().is_none());
        assert_eq!(
            first.objects[0].points[0], original_point,
            "an old task retains its frozen document"
        );
    }

    #[test]
    fn duplicate_identity_is_rejected_before_snapshot_and_new_inputs_are_accepted() {
        let mut worker = Worker::start(|_, _| None);
        let input = job(7);
        let (key, context, serial) = (input.key, input.context, input.capture.serial);
        let snapshot = input.session.clone();
        assert!(worker.needs_submission(key, context, serial, &snapshot));
        worker.submit(input);
        assert!(!worker.needs_submission(key, context, serial, &snapshot));
        assert!(worker.needs_submission(key + 1, context, serial, &snapshot));
        assert!(worker.needs_submission(key, context + 1, serial, &snapshot));
        assert!(worker.needs_submission(key, context, serial + 1, &snapshot));
        let mut changed = job(key);
        changed.context = context;
        changed.capture = Arc::new(ScreenCaptureFrame {
            serial,
            ..(*changed.capture).clone()
        });
        changed.session_mut().displays.push(Rect {
            left: -1000,
            top: -1000,
            right: -900,
            bottom: -900,
        });
        assert!(worker.needs_submission(key, context, serial, &changed.session));
        worker.submit(changed);
        assert!(worker.needs_submission(key, context, serial, &snapshot));
    }

    #[test]
    fn raster_signatures_track_cross_screen_lenses_and_drag_feedback() {
        use lexift_core::domain::annotation::MagnifierConnector;
        let mut job = job(7);
        job.session_mut().objects[0].style.fill = false;
        job.session_mut().objects[0].style.outline = false;
        let initial = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        let remote = Rect {
            left: 100,
            top: -8,
            right: 116,
            bottom: 8,
        };
        let signature = |frame: &Frame, session: &Session| {
            View::from(frame).raster_signature(remote, session, None)
        };
        assert_eq!(
            signature(&initial, &job.session),
            0,
            "unrelated screen has no effect dependency"
        );
        Arc::make_mut(&mut job.capture).serial = 2;
        Arc::make_mut(&mut job.capture).regions[0].pixels = [0, 0, 210, 255].repeat(256);
        let changed = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        assert_eq!(signature(&changed, &job.session), 0);
        let original = job.session.objects[1].clone();
        job.session_mut().objects.push(Object {
            id: Default::default(),
            kind: Kind::Magnifier,
            bounds: original.bounds,
            points: vec![],
            style: Style::default(),
            extra: Extra::Magnifier {
                output: Bounds::from_corners((100., -4.), (108., 4.)),
                zoom: 1.,
                ellipse: false,
                antialias: false,
                erase_annotations: false,
                shadow: false,
                connector: MagnifierConnector::None,
            },
        });
        assert_ne!(
            signature(&initial, &job.session),
            signature(&changed, &job.session),
            "remote lens must refresh when its earlier source mosaic changes"
        );
        let first = super::super::render_editing_with_effects(
            &job.session,
            remote,
            1.25,
            None,
            None,
            None,
            Some((&initial).into()),
        )
        .unwrap();
        let second = super::super::render_editing_with_effects(
            &job.session,
            remote,
            1.25,
            None,
            None,
            None,
            Some((&changed).into()),
        )
        .unwrap();
        assert!(
            first.data() != second.data(),
            "lens dependency must correspond to a real raster difference"
        );
        if let Extra::Magnifier {
            erase_annotations, ..
        } = &mut job.session_mut().objects[2].extra
        {
            *erase_annotations = true;
        }
        assert_eq!(
            signature(&initial, &job.session),
            0,
            "erased lens does not sample earlier effect tiles"
        );
        job.session_mut().objects.pop();
        job.session_mut().objects[1].bounds = Bounds::from_corners((102., -4.), (110., 4.));
        let moving = View {
            frame: &initial,
            preview: Some((1, &original)),
            remask: false,
        };
        assert_ne!(
            moving.raster_signature(remote, job.session.as_ref(), None),
            0,
            "old source tile must invalidate the destination screen during drag"
        );
        assert_ne!(
            moving.raster_signature(remote, job.session.as_ref(), None),
            signature(&initial, &job.session),
            "ending feedback must invalidate tiles that no longer match current geometry"
        );
    }

    #[test]
    fn mixed_dpi_sampling_includes_lower_marks_and_leaves_later_marks_clear() {
        let mut job = job(7);
        let mut later = job.session.objects[0].clone();
        later.bounds = Bounds::from_corners((0., 0.), (3., 3.));
        later.style.color = [0, 0, 220];
        job.session_mut().objects.push(later);
        let effects = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        assert_eq!(effects.tiles.len(), 1);
        let region = job.capture.regions[0].bounds;
        let output = super::super::render_editing_with_effects(
            &job.session,
            region,
            1.,
            None,
            None,
            None,
            Some((&effects).into()),
        )
        .unwrap();
        let pixel = |x: i32, y: i32| &output.data()[((y + 8) * 16 + (x + 8)) as usize * 4..][..4];
        assert_eq!(pixel(-2, 0), [200, 0, 0, 255]);
        assert_eq!(pixel(1, -2), [200, 0, 0, 255]);
        assert_eq!(pixel(1, 1), [0, 0, 220, 255]);
    }
    #[test]
    fn sampled_prefix_excludes_selection_and_future_spotlight_backdrop() {
        let mut job = job(7);
        let baseline = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        let mut later = job.session.objects[0].clone();
        later.kind = Kind::SpotlightRectangle;
        // Its hole is elsewhere: including this later backdrop would darken
        // the whole sampling ROI, even though ordinary object order is correct.
        later.bounds = Bounds::from_corners((100., 100.), (110., 110.));
        job.session_mut().objects.push(later);
        job.session_mut().spotlight_opacity = 1.;
        for selected in [None, Some(0), Some(1), Some(2)] {
            job.session_mut().selected = selected;
            let actual = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
            assert_eq!(actual.tiles.len(), baseline.tiles.len());
            for (actual, baseline) in actual.tiles.iter().zip(&baseline.tiles) {
                assert_eq!(actual.index, baseline.index);
                assert_eq!(actual.bounds, baseline.bounds);
                assert_eq!(
                    actual.pixels.data(),
                    baseline.pixels.data(),
                    "selected={selected:?}"
                );
            }
        }
    }

    #[test]
    fn effects_above_lens_use_clean_source_and_wait_for_missing_source() {
        use lexift_core::domain::annotation::MagnifierConnector;
        for effect in [MosaicEffect::Pixelate, MosaicEffect::Blur] {
            let mut job = job(7);
            let source = Rect {
                left: -100,
                top: -100,
                right: -92,
                bottom: -92,
            };
            let lens = Object {
                id: Default::default(),
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((-100., -100.), (-92., -92.)),
                points: Vec::new(),
                style: Style {
                    outline: false,
                    ..Style::default()
                },
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((-8., -8.), (8., 8.)),
                    zoom: 2.,
                    ellipse: false,
                    antialias: false,
                    erase_annotations: true,
                    shadow: false,
                    connector: MagnifierConnector::None,
                },
            };
            assert_eq!(
                super::super::mosaic_lens::sources(
                    std::slice::from_ref(&lens),
                    job.capture.regions[0].bounds
                ),
                vec![source]
            );
            assert!(
                super::super::mosaic_lens::sources(std::slice::from_ref(&lens), source).is_empty()
            );
            job.session_mut().objects.insert(1, lens);
            let Extra::Mosaic(settings) = &mut job.session_mut().objects[2].extra else {
                unreachable!()
            };
            settings.effect = effect;
            assert!(process(&job, &|| false, &mut mosaic::Scratch::default()).is_none());
            Arc::make_mut(&mut job.capture)
                .regions
                .push(CapturedRegion {
                    bounds: source,
                    pixels: [0, 0, 210, 255].repeat(64),
                });
            let result = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
            let tile = &result.tiles[0];
            assert_eq!(tile.pixels.pixel(4, 4).unwrap().blue(), 210);
            assert_eq!(
                tile.pixels.pixel(4, 4).unwrap().red(),
                0,
                "the earlier ordinary mark at lens output must be occluded by clean lens content"
            );
            job.session_mut().objects[1].bounds = Bounds::from_corners((-4., -4.), (4., 4.));
            if let Extra::Magnifier {
                erase_annotations, ..
            } = &mut job.session_mut().objects[1].extra
            {
                *erase_annotations = false;
            }
            let output = super::super::render_editing_with_effects(
                &job.session,
                job.capture.regions[0].bounds,
                1.,
                None,
                None,
                None,
                Some((&result).into()),
            )
            .unwrap();
            assert_eq!(
                output.pixel(2, 8).unwrap().red(),
                200,
                "a later mosaic in the lens source must not be magnified into its earlier output"
            );
            job.session_mut().objects[1].bounds =
                Bounds::from_corners((-100., -100.), (-92., -92.));
            if let Extra::Magnifier {
                erase_annotations, ..
            } = &mut job.session_mut().objects[1].extra
            {
                *erase_annotations = true;
            }
            job.session_mut().objects.swap(1, 2);
            let result = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
            assert_eq!(
                result.tiles[0].pixels.pixel(4, 4).unwrap().red(),
                200,
                "a later lens must not change an earlier effect"
            );
            let output = super::super::render_editing_with_effects(
                &job.session,
                job.capture.regions[0].bounds,
                1.,
                None,
                None,
                None,
                Some((&result).into()),
            )
            .unwrap();
            assert_eq!(
                output.pixel(8, 8).unwrap().alpha(),
                0,
                "the canvas must reveal the later native lens instead of covering it with the earlier effect"
            );
            if let Extra::Magnifier { ellipse, .. } = &mut job.session_mut().objects[2].extra {
                *ellipse = true;
            }
            let output = super::super::render_editing_with_effects(
                &job.session,
                job.capture.regions[0].bounds,
                1.,
                None,
                None,
                None,
                Some((&result).into()),
            )
            .unwrap();
            assert_eq!(output.pixel(8, 8).unwrap().alpha(), 0);
            assert_eq!(
                output.pixel(0, 0).unwrap().red(),
                200,
                "outside an elliptical lens lower marks remain visible"
            );
        }
    }

    #[test]
    fn partitioned_brush_matches_single_roi_without_seams_or_empty_cell_work() {
        let monitor = Rect {
            left: -520,
            top: -520,
            right: 520,
            bottom: 520,
        };
        let captured = Rect {
            left: -610,
            top: -610,
            right: 610,
            bottom: 610,
        };
        let mut pixels = Vec::with_capacity(1220 * 1220 * 4);
        for y in captured.top..captured.bottom {
            for x in captured.left..captured.right {
                pixels.extend_from_slice(&[
                    (x.rem_euclid(251)) as u8,
                    (y.rem_euclid(239)) as u8,
                    ((x + y).rem_euclid(227)) as u8,
                    255,
                ]);
            }
        }
        for (effect, strength, scale) in [
            (MosaicEffect::Pixelate, 1, 1.),
            (MosaicEffect::Pixelate, 100, 1.25),
            (MosaicEffect::Blur, 1, 1.),
            (MosaicEffect::Blur, 20, 1.25),
        ] {
            let mut object = Object {
                id: Default::default(),
                kind: Kind::MosaicBrush,
                bounds: Bounds::from_corners((-450., -450.), (450., 450.)),
                points: vec![(-450., -450.), (-450., 450.), (450., 450.)],
                style: Style::default(),
                extra: Extra::Mosaic(MosaicAnnotation::new(effect, strength, 12., scale)),
            };
            object.refresh_bounds();
            if let Extra::Mosaic(settings) = &mut object.extra {
                settings.id = 99;
            }
            let regions = mosaic::work_regions(&object, &[monitor]);
            assert!(regions.len() > 1);
            let area: i64 = regions
                .iter()
                .map(|(_, r)| (r.right as i64 - r.left as i64) * (r.bottom as i64 - r.top as i64))
                .sum();
            let whole = mosaic::sample_bounds(&object, [monitor]).unwrap();
            let full_area =
                (whole.right as i64 - whole.left as i64) * (whole.bottom as i64 - whole.top as i64);
            assert!(area < full_area, "empty cells were not eliminated");
            let mut job = job(90);
            job.session_mut().objects = vec![object.clone()];
            job.monitors = vec![(monitor, scale)];
            job.capture = Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: vec![CapturedRegion {
                    bounds: captured,
                    pixels: pixels.clone(),
                }],
            });
            let effects = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
            assert_eq!(effects.tiles.len(), regions.len());
            let actual = super::super::render_editing_with_effects(
                &job.session,
                monitor,
                scale,
                None,
                None,
                None,
                Some((&effects).into()),
            )
            .unwrap();
            let overlay = tiny_skia::Pixmap::new(
                (whole.right - whole.left) as u32,
                (whole.bottom - whole.top) as u32,
            )
            .unwrap();
            let reference_tile = mosaic::prepare(
                &overlay,
                &object,
                whole,
                &job.capture,
                &|| false,
                &mut mosaic::Scratch::default(),
            )
            .unwrap();
            let mut reference = tiny_skia::Pixmap::new(1040, 1040).unwrap();
            mosaic::blit(&mut reference, monitor, &reference_tile);
            let difference = actual
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(reference.data().as_chunks::<4>().0)
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| (i % 1040, i / 1040, *a, *b));
            assert!(
                difference.is_none(),
                "partition introduced a seam for {effect:?}/{strength}/{scale}: {difference:?}"
            );
            let mut current = tiny_skia::Pixmap::new(1040, 1040).unwrap();
            for tile in &effects.tiles {
                mosaic::blit_current_mask(&mut current, monitor, tile, &object);
            }
            let difference = current
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(reference.data().as_chunks::<4>().0)
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| (i % 1040, i / 1040, *a, *b));
            assert!(
                difference.is_none(),
                "draft feedback introduced internal cell caps: {difference:?}"
            );
            // The real capture request contains only padded path cells, rather
            // than the large rectangle used above as an independent reference.
            job.capture = Arc::new(ScreenCaptureFrame {
                revision: 2,
                serial: 2,
                regions: regions
                    .iter()
                    .map(|(_, bounds)| {
                        let mut cropped = Vec::new();
                        for y in bounds.top..bounds.bottom {
                            let start = ((y - captured.top) as usize * 1220
                                + (bounds.left - captured.left) as usize)
                                * 4;
                            let end = start + (bounds.right - bounds.left) as usize * 4;
                            cropped.extend_from_slice(&pixels[start..end]);
                        }
                        CapturedRegion {
                            bounds: *bounds,
                            pixels: cropped,
                        }
                    })
                    .collect(),
            });
            let split = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
            assert_eq!(split.tiles.len(), regions.len());
            let mut current = tiny_skia::Pixmap::new(1040, 1040).unwrap();
            for tile in &split.tiles {
                mosaic::blit_current_mask(&mut current, monitor, tile, &object);
            }
            let difference = current
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(reference.data().as_chunks::<4>().0)
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| (i % 1040, i / 1040, *a, *b));
            assert!(
                difference.is_none(),
                "split capture changed draft pixels: {difference:?}"
            );
            let completed = super::super::render_editing_with_effects(
                &job.session,
                monitor,
                scale,
                None,
                None,
                None,
                Some((&split).into()),
            )
            .unwrap();
            assert!(
                completed.data() == reference.data(),
                "split completed capture changed pixels"
            );
            if effect == MosaicEffect::Pixelate && strength == 1 {
                let mut partial = (*job.capture).clone();
                partial.regions.truncate(2);
                job.capture = Arc::new(partial);
                job.provisional_index = Some(0);
                let partial = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
                assert!(
                    !partial.complete,
                    "partial draft cannot authorize software handoff"
                );
                assert!(!partial.tiles.is_empty());
                assert!(partial.tiles.len() < split.tiles.len());
                let mut output = tiny_skia::Pixmap::new(1040, 1040).unwrap();
                for tile in &partial.tiles {
                    mosaic::blit_current_mask(&mut output, monitor, tile, &object);
                }
                let Extra::Mosaic(settings) = &object.extra else {
                    unreachable!()
                };
                let padding = settings.sample_padding().max(1);
                for (i, pixel) in output.data().as_chunks::<4>().0.iter().enumerate() {
                    if pixel[3] == 0 {
                        continue;
                    }
                    let x = monitor.left + (i % 1040) as i32;
                    let y = monitor.top + (i / 1040) as i32;
                    assert!(
                        job.capture.regions.iter().any(|r| {
                            x >= r.bounds.left + padding
                                && x < r.bounds.right - padding
                                && y >= r.bounds.top + padding
                                && y < r.bounds.bottom - padding
                        }),
                        "partial snapshot extrapolated an uncaptured pixel at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn cancellation_discards_entire_effect_frame() {
        assert!(process(&job(7), &|| true, &mut mosaic::Scratch::default()).is_none());
    }
    #[test]
    fn missing_capture_does_not_replace_last_good_effect_with_an_empty_frame() {
        let mut job = job(7);
        job.capture = Arc::new(ScreenCaptureFrame {
            revision: 2,
            serial: 2,
            regions: Vec::new(),
        });
        assert!(process(&job, &|| false, &mut mosaic::Scratch::default()).is_none());
        let Extra::Mosaic(settings) = &mut job.session_mut().objects[1].extra else {
            unreachable!()
        };
        settings.strength = 0;
        let frame = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        assert!(
            frame.complete,
            "zero strength needs no source pixels for handoff"
        );
        assert!(Arc::ptr_eq(&frame.snapshot, &job.session));
        assert!(frame.tiles.is_empty(), "zero strength needs no capture");
    }
    #[test]
    fn last_good_tile_tracks_move_and_resize_without_painting_the_old_location() {
        let mut job = job(7);
        let effects = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        let original = job.session.objects[1].clone();
        job.session_mut().objects[0].style.fill = false;
        job.session_mut().objects[0].style.outline = false;
        let monitor = Rect {
            left: -8,
            top: -8,
            right: 24,
            bottom: 16,
        };
        for bounds in [
            original.bounds.moved(8., 0.),
            Bounds::from_corners((4., -4.), (16., 8.)),
        ] {
            job.session_mut().objects[1].bounds = bounds;
            let view = View {
                remask: false,
                frame: &effects,
                preview: Some((1, &original)),
            };
            let output = super::super::render_editing_with_effects(
                &job.session,
                monitor,
                1.,
                None,
                None,
                None,
                Some(view),
            )
            .unwrap();
            let center = bounds.center();
            assert_eq!(
                output
                    .pixel((center.0 + 8.) as u32, (center.1 + 8.) as u32)
                    .unwrap()
                    .alpha(),
                255
            );
            assert_eq!(
                output.pixel(8, 8).unwrap().alpha(),
                0,
                "old location must be empty"
            );
            let output = super::super::render_editing_with_effects(
                &job.session,
                monitor,
                1.,
                None,
                None,
                None,
                Some((&effects).into()),
            )
            .unwrap();
            assert_eq!(
                output
                    .pixel((center.0 + 8.) as u32, (center.1 + 8.) as u32)
                    .unwrap()
                    .alpha(),
                0,
                "unrelated cached geometry must not be reused"
            );
        }
    }
    #[test]
    fn growing_draft_processes_captured_overlap_without_confirming_unsampled_pixels() {
        let mut job = job(7);
        job.session_mut().objects[1].bounds.right = 20.;
        assert!(
            process(&job, &|| false, &mut mosaic::Scratch::default()).is_none(),
            "committed objects require their full sampling range"
        );
        job.provisional_index = Some(1);
        let frame = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        let tile = &frame.tiles[0];
        assert_eq!(tile.bounds.right, 8);
        assert!(tile.pixels.pixel(10, 6).unwrap().alpha() > 0);
        assert_eq!(
            tile.pixels.pixel(13, 6).unwrap().alpha(),
            0,
            "filter padding is not promoted to valid draft output"
        );
    }

    #[test]
    fn provisional_texture_requires_explicit_draft_instance_validation() {
        let mut job = job(7);
        if let Extra::Mosaic(m) = &mut job.session_mut().objects[1].extra {
            m.id = 0;
        }
        let frame = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        job.session_mut().objects[0].style.fill = false;
        job.session_mut().objects[0].style.outline = false;
        let render = |view| {
            super::super::render_editing_with_effects(
                &job.session,
                job.capture.regions[0].bounds,
                1.,
                None,
                None,
                None,
                Some(view),
            )
            .unwrap()
        };
        assert_eq!(
            render((&frame).into()).pixel(8, 8).unwrap().alpha(),
            0,
            "a new zero-id draft at the same position cannot inherit a cancelled draft's frame"
        );
        let view = View {
            frame: &frame,
            preview: Some((1, &frame.tiles[0].object)),
            remask: true,
        };
        assert_eq!(render(view).pixel(8, 8).unwrap().alpha(), 255);
    }

    #[test]
    fn worker_rejects_old_geometry_results() {
        let mut worker = Worker::default();
        worker.submit(job(7));
        worker.submit(job(8));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while worker.latest(8).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(worker.latest(8).is_some());
        assert!(worker.latest(7).is_none());
    }
    #[test]
    fn geometry_updates_finish_in_the_same_gesture_but_new_context_discards_results() {
        let (started, starts) = std::sync::mpsc::channel();
        let (release, permits) = std::sync::mpsc::channel();
        let mut worker = Worker::start(move |job, cancelled| {
            started.send(job.key).unwrap();
            permits
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            (!cancelled()).then_some(Frame {
                key: job.key,
                context: job.context,
                serial: job.capture.serial,
                tiles: Vec::new(),
                snapshot: job.session.clone(),
                complete: job.provisional_index.is_none(),
            })
        });
        let timeout = std::time::Duration::from_secs(2);
        let mut first = job(7);
        first.context = 40;
        worker.submit(first);
        assert_eq!(starts.recv_timeout(timeout).unwrap(), 7);
        let mut next_pose = job(8);
        next_pose.context = 40;
        worker.submit(next_pose);
        release.send(()).unwrap();
        assert_eq!(starts.recv_timeout(timeout).unwrap(), 8);
        assert_eq!(
            worker.latest(40).unwrap().key,
            7,
            "completed pose supplies feedback while the next pose processes"
        );
        let mut new_gesture = job(9);
        new_gesture.context = 90;
        worker.submit(new_gesture);
        assert!(
            worker.latest(40).is_none(),
            "old gesture cannot supply callbacks after cancellation or reopening"
        );
        release.send(()).unwrap();
        assert_eq!(starts.recv_timeout(timeout).unwrap(), 9);
        assert!(
            worker.latest(90).is_none(),
            "pose from the cancelled context must not publish"
        );
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while worker.latest(90).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(worker.latest(90).unwrap().key, 9);
    }

    #[test]
    fn cached_effect_never_crosses_object_identity_or_survives_zero_strength() {
        let mut job = job(7);
        let effects = process(&job, &|| false, &mut mosaic::Scratch::default()).unwrap();
        job.session_mut().objects[0].style.fill = false;
        job.session_mut().objects[0].style.outline = false;
        let region = job.capture.regions[0].bounds;
        let render = |session: &Session| {
            super::super::render_editing_with_effects(
                session,
                region,
                1.,
                None,
                None,
                None,
                Some((&effects).into()),
            )
            .unwrap()
            .pixel(8, 8)
            .unwrap()
            .alpha()
        };
        assert_eq!(render(&job.session), 255);
        let Extra::Mosaic(m) = &mut job.session_mut().objects[1].extra else {
            unreachable!()
        };
        m.id = 2;
        assert_eq!(
            render(&job.session),
            0,
            "same geometry is not the same mask"
        );
        let Extra::Mosaic(m) = &mut job.session_mut().objects[1].extra else {
            unreachable!()
        };
        m.id = 1;
        m.strength = 0;
        assert_eq!(
            render(&job.session),
            0,
            "zero hides the cached effect immediately"
        );
    }
}
