//! One frozen document and one independent capture per download. Workers own
//! their buffers; UI polling and cancellation belong to the annotation session.
use super::{REGISTRY, Registry};
use crate::annotation_render::{self, mosaic_worker};
use lexift_core::{
    Error, Result,
    domain::{
        annotation::{Extra, Session},
        geometry::Rect,
    },
    ports::{
        annotation_save::{AnnotationSavePort, PngSaveRequest, SaveOutcome},
        magnifier::WindowToken,
        screen_capture::{
            CapturedRegion, ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
        },
    },
};
use slint::ComponentHandle;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

type CaptureFactory = fn() -> Option<Arc<dyn ScreenCapturePort>>;
#[derive(Default)]
pub(super) struct State {
    cancelled: Option<Arc<AtomicBool>>,
    result: Option<Receiver<Result<SaveOutcome>>>,
    timer: slint::Timer,
    editor: Option<u64>,
    request: u64,
    origin: Option<super::focus::Origin>,
    monitor: Option<Rect>,
}
impl State {
    pub fn cancel(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
        self.timer.stop();
        self.result = None;
        self.editor = None;
        self.monitor = None;
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) fn available(r: &Registry) -> bool {
    r.lifecycle.annotation_export_capture.is_some()
        && r.lifecycle
            .annotation_save
            .as_ref()
            .is_some_and(|p| p.is_available())
        && r.lifecycle
            .annotation_screen_capture
            .as_ref()
            .is_some_and(|p| p.is_available())
}

struct Snapshot {
    session: Arc<Session>,
    monitor: Rect,
    scale: f32,
    monitors: Vec<(Rect, f32)>,
    excluded: Vec<WindowToken>,
    owner: Option<WindowToken>,
    name: String,
    title: String,
}

/// The toolbar center selects a complete display, never its work area.
fn display_at(center: (i64, i64), displays: &[(Rect, f32)]) -> Option<(Rect, f32)> {
    displays.iter().copied().find(|(r, _)| {
        center.0 >= i64::from(r.left)
            && center.0 < i64::from(r.right)
            && center.1 >= i64::from(r.top)
            && center.1 < i64::from(r.bottom)
    })
}

fn snapshot(r: &Registry) -> Option<Snapshot> {
    let main = r.main.get()?;
    let position = main.window().position();
    let size = main.window().size();
    let monitors: Vec<_> = r
        .canvases
        .iter()
        .map(|c| (c.bounds, c.window.window().scale_factor()))
        .collect();
    let (monitor, scale) = display_at(
        (
            i64::from(position.x) + i64::from(size.width) / 2,
            i64::from(position.y) + i64::from(size.height) / 2,
        ),
        &monitors,
    )?;
    // Copy visible state, not the undo stack or parameter bindings.
    let mut session = Session::new();
    session.objects = r.session.objects.clone();
    session.displays = r.session.displays.clone();
    session.watermark = r.session.watermark.clone();
    session.watermark_erased = r.session.watermark_erased.clone();
    session.spotlight_erased = r.session.spotlight_erased.clone();
    session.spotlight_opacity = r.session.spotlight_opacity;
    if let Some((index, mut object)) = super::edited_document_object(r) {
        // Native editors omit their glyphs from the canvas, but their HWND is
        // excluded from export. Restore visible draft glyphs in the snapshot.
        if let Some(editor) = r.editor.window() {
            let content = super::editor_preview(editor).0;
            match &mut object.extra {
                Extra::Text(text) => {
                    text.content = content;
                    text.outline = r.editor.edit()?.original.outline.clone();
                }
                Extra::Sequence(sequence) => {
                    sequence.caption.content = content;
                    sequence.caption.outline = r.editor.edit()?.original.outline.clone();
                }
                _ => {}
            }
        }
        if let Some(index) = index {
            session.objects[index] = object;
        } else {
            session.objects.push(object);
        }
    }
    if let Some(draft) = super::gestures::display_draft(&r.gesture, &r.pending_drawing) {
        session.objects.push(draft.clone());
    }
    identify_draft_effects(&mut session)?;
    Some(Snapshot {
        session: Arc::new(session),
        monitor,
        scale,
        monitors,
        excluded: super::capture_exclusions(r),
        owner: (r.lifecycle.annotation_window_token)(main.window()),
        name: format!(
            "Lexift-{}.png",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ),
        title: crate::i18n::tr("Save annotation screenshot"),
    })
}

/// Live drafts have no completed effect identity. The frozen export needs its
/// own identity for tile matching, without assigning an object in the real document.
fn identify_draft_effects(session: &mut Session) -> Option<()> {
    let mut used: std::collections::HashSet<_> = session
        .objects
        .iter()
        .filter_map(|o| {
            if let Extra::Mosaic(m) = &o.extra {
                Some(m.id)
            } else {
                None
            }
        })
        .collect();
    let mut next = 1u64;
    for object in &mut session.objects {
        if let Extra::Mosaic(m) = &mut object.extra
            && m.id == 0
        {
            while used.contains(&next) {
                next = next.checked_add(1)?;
            }
            m.id = next;
            used.insert(next);
        }
    }
    Some(())
}

pub(super) fn start() {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.annotation_ready && r.export.cancelled.is_none() && available(r))
        else {
            return;
        };
        let Some(snapshot) = snapshot(r) else { return };
        let monitor = snapshot.monitor;
        let factory = r.lifecycle.annotation_export_capture.unwrap();
        let save = r.lifecycle.annotation_save.as_ref().unwrap().clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let cancel = cancelled.clone();
        let spawned = std::thread::Builder::new()
            .name("annotation-export".into())
            .spawn(move || {
                let result = export(snapshot, factory, save.as_ref(), cancel);
                let _ = tx.send(result);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "Could not start annotation export");
            super::mosaic::notify_on(r, "Could not save screenshot. Try again.", Some(monitor));
            r.queue_render();
            return;
        }
        r.export.request = super::window::next_revision();
        r.focus.cancel();
        r.export.origin = Some(super::focus_origin(&r.lifecycle));
        r.export.editor = r.editor.edit().map(|edit| edit.id);
        r.export.monitor = Some(monitor);
        r.export.cancelled = Some(cancelled);
        r.export.result = Some(rx);
        let generation = r.generation;
        let request = r.export.request;
        r.main.get().unwrap().set_can_download(false);
        r.export.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(25),
            move || poll(generation, request),
        );
    });
}

fn poll(generation: u64, request: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.export.request == request
                && r.export.cancelled.is_some()
        }) else {
            return;
        };
        let result = match r.export.result.as_ref().unwrap().try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(Error::new("Annotation export worker stopped"))
            }
        };
        let editor = r.export.editor;
        let monitor = r.export.monitor;
        let origin = r.export.origin.unwrap_or(super::focus::Origin::Unobserved);
        r.export.cancel();
        if let Some(main) = r.main.get() {
            main.set_can_download(available(r));
        }
        if let Err(error) = result {
            tracing::warn!(%error, "Annotation export failed");
            super::mosaic::notify_on(r, "Could not save screenshot. Try again.", monitor);
            r.queue_render();
        }
        if editor.is_some() && r.editor.edit().map(|e| e.id) == editor {
            super::restore_editor_focus_from(r, super::focus::Activation::UserRequested, origin);
        }
    });
}

fn regions(snapshot: &Snapshot) -> Vec<Rect> {
    let mut regions = vec![snapshot.monitor];
    let monitors: Vec<_> = snapshot.monitors.iter().map(|(r, _)| *r).collect();
    for object in &snapshot.session.objects {
        for (_, roi) in annotation_render::mosaic::work_regions(object, &monitors) {
            regions.push(roi);
        }
    }
    let sources: Vec<_> = regions
        .iter()
        .flat_map(|&roi| annotation_render::mosaic_lens::sources(&snapshot.session.objects, roi))
        .collect();
    regions.extend(sources);
    regions.sort_unstable_by_key(|r| (r.left, r.top, r.right, r.bottom));
    regions.dedup();
    // A containing sample already supplies a smaller ROI's pixels.
    let all = regions.clone();
    regions.retain(|r| !all.iter().any(|outer| outer != r && contains(*outer, *r)));
    regions
}
fn contains(a: Rect, b: Rect) -> bool {
    a.left <= b.left && a.top <= b.top && a.right >= b.right && a.bottom >= b.bottom
}

/// The new instance cannot publish a prior request's cache. Unchanged pixels
/// still produce its first frame; no serial-increment test is needed.
fn capture(
    factory: CaptureFactory,
    regions: Vec<Rect>,
    excluded_windows: Vec<WindowToken>,
    cancelled: &AtomicBool,
) -> Result<Option<Arc<ScreenCaptureFrame>>> {
    capture_with_timeout(
        factory,
        regions,
        excluded_windows,
        cancelled,
        Duration::from_secs(5),
    )
}

fn capture_with_timeout(
    factory: CaptureFactory,
    regions: Vec<Rect>,
    excluded_windows: Vec<WindowToken>,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Option<Arc<ScreenCaptureFrame>>> {
    let deadline = Instant::now() + timeout;
    let port = factory().ok_or_else(|| Error::new("Screen capture unavailable"))?;
    if !port.is_available() {
        return Err(Error::new("Screen capture unavailable"));
    }
    let request = ScreenCaptureRequest {
        revision: 1,
        regions,
        paused_regions: Vec::new(),
        excluded_windows,
    };
    port.request(request.clone())?;
    let mut last_error = None;
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Ok(None);
        }
        match port.latest() {
            Ok(Some(frame))
                if frame.revision == request.revision
                    && request.regions.iter().all(|r| {
                        frame
                            .regions
                            .iter()
                            .any(|source| contains(source.bounds, *r))
                    }) =>
            {
                break Ok(Some(frame));
            }
            Err(error) => last_error = Some(error),
            _ => {}
        }
        if Instant::now() >= deadline {
            break Err(last_error.unwrap_or_else(|| Error::new("Screen capture timed out")));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    port.clear();
    result
}

fn export(
    snapshot: Snapshot,
    factory: CaptureFactory,
    save: &dyn AnnotationSavePort,
    cancelled: Arc<AtomicBool>,
) -> Result<SaveOutcome> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    let Some(capture) = capture(
        factory,
        regions(&snapshot),
        snapshot.excluded.clone(),
        &cancelled,
    )?
    else {
        return Ok(SaveOutcome::Cancelled);
    };
    if cancelled.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    let image = compose(&snapshot, capture, &cancelled)?;
    if cancelled.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    save.save_png(
        PngSaveRequest {
            image,
            owner: snapshot.owner,
            default_name: snapshot.name,
            title: snapshot.title,
        },
        cancelled,
    )
}

fn compose(
    snapshot: &Snapshot,
    capture: Arc<ScreenCaptureFrame>,
    cancelled: &AtomicBool,
) -> Result<CapturedRegion> {
    let cancelled = || cancelled.load(Ordering::Acquire);
    let job = mosaic_worker::Job {
        key: 1,
        context: 1,
        session: snapshot.session.clone(),
        provisional_index: None,
        monitors: snapshot.monitors.clone(),
        capture: capture.clone(),
    };
    let effects = mosaic_worker::process_once(&job, &cancelled)
        .ok_or_else(|| Error::new("Incomplete annotation effects"))?;
    let overlay = annotation_render::render_export(
        &snapshot.session,
        snapshot.monitor,
        snapshot.scale,
        (&effects).into(),
        &capture,
    )
    .ok_or_else(|| Error::new("Incomplete annotation image"))?;
    let region = capture
        .regions
        .iter()
        .find(|r| contains(r.bounds, snapshot.monitor))
        .ok_or_else(|| Error::new("Missing screen pixels"))?;
    let width = (snapshot.monitor.right - snapshot.monitor.left) as u32;
    let height = (snapshot.monitor.bottom - snapshot.monitor.top) as u32;
    let mut output =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| Error::new("Image is too large"))?;
    let stride = (region.bounds.right - region.bounds.left) as usize * 4;
    for (y, row) in output
        .data_mut()
        .chunks_exact_mut(width as usize * 4)
        .enumerate()
    {
        let start = (snapshot.monitor.top - region.bounds.top) as usize * stride
            + y * stride
            + (snapshot.monitor.left - region.bounds.left) as usize * 4;
        row.copy_from_slice(
            region
                .pixels
                .get(start..start + row.len())
                .ok_or_else(|| Error::new("Incomplete screen pixels"))?,
        );
    }
    output.draw_pixmap(
        0,
        0,
        overlay.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    Ok(CapturedRegion {
        bounds: snapshot.monitor,
        pixels: output.take(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::{
        annotation::{
            Bounds, Kind, MagnifierConnector, MosaicAnnotation, MosaicEffect, Object, Style,
        },
        annotation_erase::{EraseRegion, ErasedCoverage},
    };
    fn fixture() -> Snapshot {
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        Snapshot {
            session: Arc::new(Session::new()),
            monitor,
            scale: 1.,
            monitors: vec![(monitor, 1.)],
            excluded: vec![WindowToken(100)],
            owner: None,
            name: "test.png".into(),
            title: String::new(),
        }
    }
    fn rectangle(a: (f32, f32), b: (f32, f32), color: [u8; 3]) -> Object {
        Object {
            id: Default::default(),
            erased: Vec::new(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners(a, b),
            style: Style {
                fill: true,
                outline: false,
                rounding: 0.,
                color,
                ..Style::default()
            },
            points: Vec::new(),
            extra: Extra::None,
        }
    }
    fn frame(bounds: Rect, color: [u8; 4]) -> Arc<ScreenCaptureFrame> {
        Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds,
                pixels: color
                    .repeat(((bounds.right - bounds.left) * (bounds.bottom - bounds.top)) as usize),
            }],
        })
    }
    fn pixel(image: &CapturedRegion, x: i32, y: i32) -> &[u8] {
        let i = ((y - image.bounds.top) * (image.bounds.right - image.bounds.left) + x
            - image.bounds.left) as usize
            * 4;
        &image.pixels[i..i + 4]
    }
    #[test]
    fn export_composes_frozen_pixels_and_erasures_without_selection_handles() {
        for scale in [1., 1.25] {
            let mut snapshot = fixture();
            snapshot.scale = scale;
            let session = Arc::get_mut(&mut snapshot.session).unwrap();
            let mut rect = rectangle((-8., -8.), (8., 8.), [210, 0, 0]);
            rect.erased.push(ErasedCoverage::new(EraseRegion::Rectangle(
                Bounds::from_corners((-4., -4.), (4., 4.)),
            )));
            session.objects.push(rect);
            session.selected = Some(0);
            let output = compose(
                &snapshot,
                frame(snapshot.monitor, [0, 120, 0, 255]),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(pixel(&output, 0, 0), [0, 120, 0, 255]);
            assert_eq!(pixel(&output, 6, 6), [210, 0, 0, 255]);
            assert_eq!(
                pixel(&output, -8, -8),
                [210, 0, 0, 255],
                "selection controls must not enter export"
            );
            assert_eq!(pixel(&output, 12, 12), [0, 120, 0, 255]);
            assert!(!snapshot.session.can_undo());
        }
    }
    #[test]
    fn export_effects_use_this_capture_and_keep_later_marks_clear() {
        for effect in [MosaicEffect::Pixelate, MosaicEffect::Blur] {
            let mut snapshot = fixture();
            let session = Arc::get_mut(&mut snapshot.session).unwrap();
            let mut area = rectangle((-8., -8.), (8., 8.), [0, 0, 0]);
            area.kind = Kind::MosaicArea;
            let mut settings = MosaicAnnotation::new(effect, 50, 12., 1.);
            settings.id = 1;
            area.extra = Extra::Mosaic(settings);
            session.objects.push(area);
            session
                .objects
                .push(rectangle((2., 2.), (6., 6.), [0, 0, 230]));
            let sampling = Rect {
                left: -100,
                top: -100,
                right: 100,
                bottom: 100,
            };
            let output = compose(
                &snapshot,
                frame(sampling, [170, 30, 20, 255]),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(pixel(&output, 0, 0), [170, 30, 20, 255]);
            assert_eq!(pixel(&output, 4, 4), [0, 0, 230, 255]);
        }
    }

    #[test]
    fn uncommitted_mosaic_draft_uses_export_identity_without_mutating_the_draft() {
        let mut snapshot = fixture();
        let mut draft = rectangle((-8., -8.), (8., 8.), [0, 0, 0]);
        draft.kind = Kind::MosaicArea;
        draft.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 100, 12., 1.));
        let session = Arc::get_mut(&mut snapshot.session).unwrap();
        session.objects.push(draft.clone());
        identify_draft_effects(session).unwrap();
        assert!(matches!(draft.extra,Extra::Mosaic(m) if m.id==0));
        assert!(!session.can_undo());
        let sampling = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let mut capture = frame(sampling, [10, 30, 90, 255]);
        let frame = Arc::get_mut(&mut capture).unwrap();
        // Distinct adjacent colors must actually be pixelated, not just reveal
        // the unchanged base through a missing draft tile.
        for y in 0..200usize {
            for x in 0..200usize {
                if x % 2 == 0 {
                    frame.regions[0].pixels[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4]
                        .copy_from_slice(&[200, 40, 20, 255]);
                }
            }
        }
        let output = compose(&snapshot, capture, &AtomicBool::new(false)).unwrap();
        let color = pixel(&output, 0, 0);
        assert!(
            color[0] > 30 && color[0] < 180,
            "uncommitted mosaic was omitted: {color:?}"
        );
    }
    #[test]
    fn magnifier_samples_cross_display_pixels_and_includes_annotations() {
        let mut snapshot = fixture();
        let session = Arc::get_mut(&mut snapshot.session).unwrap();
        session
            .objects
            .push(rectangle((-40., 0.), (-36., 4.), [230, 0, 0]));
        let mut lens = rectangle((-40., 0.), (-36., 4.), [0, 0, 0]);
        lens.kind = Kind::Magnifier;
        lens.style.outline = false;
        lens.style.width = 0.;
        lens.extra = Extra::Magnifier {
            output: Bounds::from_corners((0., 0.), (8., 8.)),
            zoom: 2.,
            ellipse: false,
            antialias: false,
            erase_annotations: false,
            shadow: false,
            connector: MagnifierConnector::None,
        };
        session.objects.push(lens);
        assert!(
            regions(&snapshot)
                .iter()
                .any(|r| r.left <= -40 && r.right >= -36)
        );
        let sampling = Rect {
            left: -50,
            top: -30,
            right: 30,
            bottom: 30,
        };
        let output = compose(
            &snapshot,
            frame(sampling, [0, 90, 0, 255]),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(pixel(&output, 4, 4), [230, 0, 0, 255]);
        if let Extra::Magnifier {
            erase_annotations, ..
        } = &mut Arc::get_mut(&mut snapshot.session).unwrap().objects[1].extra
        {
            *erase_annotations = true;
        }
        let output = compose(
            &snapshot,
            frame(sampling, [0, 90, 0, 255]),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(pixel(&output, 4, 4), [0, 90, 0, 255]);
    }
    #[test]
    fn toolbar_center_uses_full_display_and_negative_coordinates() {
        let displays = [
            (
                Rect {
                    left: -1920,
                    top: 0,
                    right: 0,
                    bottom: 1080,
                },
                1.,
            ),
            (
                Rect {
                    left: 0,
                    top: 0,
                    right: 2560,
                    bottom: 1440,
                },
                1.25,
            ),
        ];
        assert_eq!(display_at((-1, 1050), &displays), Some(displays[0]));
        assert_eq!(display_at((0, 1400), &displays), Some(displays[1]));
        assert!(display_at((3000, 0), &displays).is_none());
    }
    #[test]
    fn cancelled_export_never_opens_save_dialog() {
        struct Save;
        impl AnnotationSavePort for Save {
            fn is_available(&self) -> bool {
                true
            }
            fn save_png(&self, _: PngSaveRequest, _: Arc<AtomicBool>) -> Result<SaveOutcome> {
                panic!("cancelled job opened dialog")
            }
        }
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                panic!("cancelled job read frame")
            }
            fn clear(&self) {}
        }
        let result = export(
            fixture(),
            || Some(Arc::new(Capture)),
            &Save,
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        assert_eq!(result, SaveOutcome::Cancelled);
    }
    #[test]
    fn single_capture_waits_for_request_and_all_sources_not_a_serial_change() {
        use std::sync::atomic::AtomicUsize;
        struct Capture {
            calls: AtomicUsize,
        }
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, request: ScreenCaptureRequest) -> Result<()> {
                assert_eq!(request.regions, vec![fixture().monitor]);
                assert_eq!(request.excluded_windows, vec![WindowToken(100)]);
                assert!(request.paused_regions.is_empty());
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                let calls = self.calls.fetch_add(1, Ordering::Relaxed);
                let mut result = ScreenCaptureFrame {
                    revision: 1,
                    serial: 41,
                    regions: frame(fixture().monitor, [20, 30, 40, 255]).regions.clone(),
                };
                if calls == 0 {
                    result.revision = 0;
                }
                if calls == 1 {
                    result.regions.clear();
                }
                Ok(Some(Arc::new(result)))
            }
            fn clear(&self) {
                assert_eq!(self.calls.load(Ordering::Relaxed), 3);
            }
        }
        let frame = capture(
            || {
                Some(Arc::new(Capture {
                    calls: AtomicUsize::new(0),
                }))
            },
            vec![fixture().monitor],
            vec![WindowToken(100)],
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            frame.serial, 41,
            "unchanged serial must not stall a new independent capture"
        );
    }

    #[test]
    fn capture_timeout_releases_the_job_without_opening_a_dialog() {
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> Result<()> {
                Ok(())
            }
            fn latest(&self) -> Result<Option<Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        let result = capture_with_timeout(
            || Some(Arc::new(Capture)),
            vec![fixture().monitor],
            Vec::new(),
            &AtomicBool::new(false),
            Duration::from_millis(1),
        );
        assert!(result.is_err_and(|e| e.to_string().contains("timed out")));
    }

    #[test]
    fn snapshot_preserves_text_draft_and_does_not_commit_it() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::rc::Rc;
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(
                &self,
            ) -> std::result::Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        super::super::init(
            |_| super::super::PassiveWindowPreparation::Ready,
            super::super::WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(crate::AnnotationToolbar::new().unwrap());
            r.main
                .get()
                .unwrap()
                .window()
                .set_size(slint::PhysicalSize::new(650, 54));
            let monitor = Rect {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 768,
            };
            r.canvases.push(super::super::CanvasLayer::new(
                crate::AnnotationCanvas::new().unwrap(),
                monitor,
            ));
            let window = crate::AnnotationTextInput::new().unwrap();
            window.set_value("draft 中文".into());
            window.invoke_place_selection(2, 5);
            let text = lexift_core::domain::annotation::TextAnnotation {
                content: String::new(),
                font_family: "Segoe UI".into(),
                font_size: 22.,
                bold: false,
                italic: false,
                outline: None,
                background: None,
                rotation: 0.,
                linked_arrow: None,
                layout_scale: 1.,
                linked_arrow_control: None,
            };
            let edit = super::super::TextEdit::new(
                88,
                None,
                (100., 100.),
                1.,
                text,
                Bounds::from_corners((100., 100.), (260., 140.)),
                String::new(),
            );
            r.editor
                .open(window, edit, Some(super::super::parameters::defaults(8)));
            let frozen = snapshot(r).unwrap();
            assert_eq!(frozen.session.objects.len(), 1);
            assert!(
                matches!(&frozen.session.objects[0].extra,Extra::Text(t) if t.content=="draft 中文")
            );
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
            assert_eq!(r.editor.edit().unwrap().id, 88);
            assert_eq!(r.editor.window().unwrap().get_cursor_offset(), 5);
            assert_eq!(r.editor.window().unwrap().get_anchor_offset(), 2);
            r.editor.window().unwrap().set_value("new content".into());
            assert!(
                matches!(&frozen.session.objects[0].extra,Extra::Text(t) if t.content=="draft 中文")
            );
            r.export.cancelled = Some(Arc::new(AtomicBool::new(false)));
            let cancelled = r.export.cancelled.as_ref().unwrap().clone();
            r.export.cancel();
            assert!(cancelled.load(Ordering::Acquire));
            r.close();
        });
        // A delayed completion from a retired request must leave the next job busy.
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(SaveOutcome::Saved)).unwrap();
            r.export.cancelled = Some(Arc::new(AtomicBool::new(false)));
            r.export.request = 42;
            r.export.result = Some(rx);
            r.generation
        });
        poll(generation, 41);
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().export.cancelled.is_some()));
        poll(generation, 42);
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().export.cancelled.is_none()));
    }
}
