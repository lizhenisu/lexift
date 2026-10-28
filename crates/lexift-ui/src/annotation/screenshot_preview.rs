//! Coalesced CPU preview jobs. Workers own immutable documents, never live views or OS capture.
use super::*;
use lexift_core::{
    domain::annotation_export::ExportSnapshot, ports::screen_capture::ScreenCaptureFrame,
};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
pub(super) struct Input {
    pub snapshot: ExportSnapshot,
    pub capture: Arc<ScreenCaptureFrame>,
    pub crop: Rect,
    pub label: Option<crate::screen_interaction::size_label::Feedback>,
    pub help: Option<crate::screen_interaction::screenshot_help::Feedback>,
    pub picker: Option<crate::screen_interaction::pixel_picker::Feedback>,
    pub hovered: Option<usize>,
    pub crop_handles: bool,
    pub crop_border: bool,
    pub erase: Option<(
        lexift_core::domain::annotation_erase::RoundedRectangle,
        bool,
    )>,
}
#[derive(Default)]
pub(super) struct Cache {
    scene: crate::annotation_scene::SceneCompiler,
    base: Option<Base>,
    image: Option<ImageBase>,
    #[cfg(test)]
    pub builds: usize,
}
struct Base {
    snapshot: ExportSnapshot,
    capture: Arc<ScreenCaptureFrame>,
    composed: tiny_skia::Pixmap,
    dark: Arc<tiny_skia::Pixmap>,
}
// One optional, recomputable content raster per worker; never an input/output size limit.
const MAX_CACHED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
struct ImageBase {
    snapshot: ExportSnapshot,
    capture: Arc<ScreenCaptureFrame>,
    pixels: Arc<tiny_skia::Pixmap>,
}
fn content_matches(a: &ExportSnapshot, b: &ExportSnapshot) -> bool {
    a.monitor == b.monitor
        && a.scale == b.scale
        && a.monitors == b.monitors
        && a.session.objects == b.session.objects
        && a.session.watermark == b.session.watermark
        && a.session.watermark_erased == b.session.watermark_erased
        && a.session.spotlight_opacity == b.session.spotlight_opacity
        && a.session.spotlight_erased == b.session.spotlight_erased
        && a.session.displays == b.session.displays
        && a.session.image_source == b.session.image_source
}
impl ImageBase {
    fn matches(&self, input: &Input) -> bool {
        Arc::ptr_eq(&self.capture, &input.capture)
            && content_matches(&self.snapshot, &input.snapshot)
            && self.snapshot.session.image_crop == input.snapshot.session.image_crop
            && self.snapshot.session.image_orientation == input.snapshot.session.image_orientation
            && self.snapshot.session.image_color == input.snapshot.session.image_color
            && self.snapshot.session.image_scale == input.snapshot.session.image_scale
    }
}
impl Base {
    fn matches(&self, input: &Input) -> bool {
        let a = &self.snapshot;
        let b = &input.snapshot;
        Arc::ptr_eq(&self.capture, &input.capture)
            && a.monitor == b.monitor
            && a.session.image_scale == b.session.image_scale
            && a.session.image_color == b.session.image_color
            && a.scale == b.scale
            && a.monitors == b.monitors
            && a.session.objects == b.session.objects
            && a.session.watermark == b.session.watermark
            && a.session.watermark_erased == b.session.watermark_erased
            && a.session.spotlight_opacity == b.session.spotlight_opacity
            && a.session.spotlight_erased == b.session.spotlight_erased
            && a.session.displays == b.session.displays
    }
}
pub(super) fn paint(input: &Input, cancelled: &AtomicBool) -> Option<tiny_skia::Pixmap> {
    paint_cached(input, cancelled, &mut Cache::default(), true)
}
fn paint_cached(
    input: &Input,
    cancelled: &AtomicBool,
    cache: &mut Cache,
    controls: bool,
) -> Option<tiny_skia::Pixmap> {
    if input.crop_border && cache.base.as_ref().is_none_or(|base| !base.matches(input)) {
        let composed = compose_base(input, cancelled, &mut cache.scene)?;
        let dark = if let Some(base) = cache.base.as_ref().filter(|base| {
            Arc::ptr_eq(&base.capture, &input.capture)
                && base.snapshot.monitor == input.snapshot.monitor
        }) {
            base.dark.clone()
        } else {
            let raw = input
                .capture
                .regions
                .iter()
                .find(|f| f.bounds == input.snapshot.monitor)?;
            let mut dark = tiny_skia::Pixmap::from_vec(
                raw.pixels.clone(),
                tiny_skia::IntSize::from_wh(composed.width(), composed.height())?,
            )?;
            for p in dark.data_mut().as_chunks_mut::<4>().0 {
                for c in &mut p[..3] {
                    *c = (f32::from(*c) * 0.45) as u8;
                }
                p[3] = 255;
            }
            Arc::new(dark)
        };
        cache.base = Some(Base {
            snapshot: input.snapshot.clone(),
            capture: input.capture.clone(),
            composed,
            dark,
        });
        #[cfg(test)]
        {
            cache.builds += 1;
        }
    }
    let crop_only = input.crop_border
        && input.snapshot.session.selected.is_none()
        && input.hovered.is_none()
        && input.erase.is_none();
    let mut pixmap = if crop_only {
        cache.base.as_ref()?.dark.as_ref().clone()
    } else if input.crop_border {
        cache.base.as_ref()?.composed.clone()
    } else {
        compose_base(input, cancelled, &mut cache.scene)?
    };
    let snapshot = &input.snapshot;
    let monitor = snapshot.monitor;
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    if !controls {
        return Some(pixmap);
    }
    crate::annotation_render::draw_interaction(
        &mut pixmap,
        &snapshot.session,
        monitor,
        snapshot.scale,
        input.hovered,
    );
    if let Some((geometry, handles)) = input.erase {
        crate::annotation_render::draw_erase_interaction(
            &mut pixmap,
            geometry,
            handles,
            monitor,
            snapshot.scale,
        );
    }
    let bounds = input.crop;
    if crop_only {
        copy_crop(
            &mut pixmap,
            &cache.base.as_ref()?.composed,
            bounds,
            monitor,
            cancelled,
        )?;
    } else if input.crop_border {
        let mut masked = cache.base.as_ref()?.dark.as_ref().clone();
        copy_crop(&mut masked, &pixmap, bounds, monitor, cancelled)?;
        pixmap = masked;
    }
    let geometry = Bounds {
        left: bounds.left as f32,
        top: bounds.top as f32,
        right: bounds.right as f32,
        bottom: bounds.bottom as f32,
    };
    if input.crop_border {
        crate::annotation_render::draw_erase_interaction(
            &mut pixmap,
            lexift_core::domain::annotation_erase::RoundedRectangle {
                bounds: geometry,
                rounding: 0.,
                creation_scale: 1.,
            },
            false,
            monitor,
            snapshot.scale,
        );
    }
    if input.crop_handles {
        for (x, y) in geometry.handles() {
            crate::annotation_render::draw_rectangle_handle(
                &mut pixmap,
                (x - monitor.left as f32, y - monitor.top as f32),
                false,
                snapshot.scale,
            );
        }
    }
    if let Some(help) = &input.help {
        help.paint(&mut pixmap, monitor);
    }
    if let Some(picker) = &input.picker {
        picker.paint(&mut pixmap, monitor);
    }
    if let Some(label) = &input.label {
        if label.crop != input.crop {
            return None;
        }
        label.paint(&mut pixmap, monitor);
    }
    Some(pixmap)
}

fn copy_crop(
    target: &mut tiny_skia::Pixmap,
    source: &tiny_skia::Pixmap,
    bounds: Rect,
    monitor: Rect,
    cancelled: &AtomicBool,
) -> Option<()> {
    let width = source.width() as usize;
    let left = (bounds.left - monitor.left).clamp(0, source.width() as i32) as usize;
    let right = (bounds.right - monitor.left).clamp(0, source.width() as i32) as usize;
    let top = (bounds.top - monitor.top).clamp(0, source.height() as i32) as usize;
    let bottom = (bounds.bottom - monitor.top).clamp(0, source.height() as i32) as usize;
    for y in top..bottom {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let start = (y * width + left) * 4;
        let end = (y * width + right) * 4;
        target.data_mut()[start..end].copy_from_slice(&source.data()[start..end]);
    }
    Some(())
}

fn compose_base(
    input: &Input,
    cancelled: &AtomicBool,
    scene: &mut crate::annotation_scene::SceneCompiler,
) -> Option<tiny_skia::Pixmap> {
    let snapshot = &input.snapshot;
    let monitor = snapshot.monitor;
    let base = input.capture.regions.iter().find(|f| {
        f.bounds == monitor || !input.crop_border
            && f.bounds.left <= monitor.left && f.bounds.top <= monitor.top
            && f.bounds.right >= monitor.right && f.bounds.bottom >= monitor.bottom
    }).or_else(|| {
        tracing::debug!(?monitor, capture_bounds=?input.capture.regions.iter().map(|r|r.bounds).collect::<Vec<_>>(), "Frozen preview source geometry mismatch");
        None
    })?;
    let pixmap = if !input.crop_border {
        let image = crate::annotation_export_render::compose_frozen(
            snapshot,
            input.capture.clone(),
            cancelled,
        )
        .map_err(|error| {
            tracing::debug!(%error, "Frozen preview composition failed");
            error
        })
        .ok()?;
        tiny_skia::Pixmap::from_vec(
            image.pixels,
            tiny_skia::IntSize::from_wh(
                (monitor.right - monitor.left) as u32,
                (monitor.bottom - monitor.top) as u32,
            )?,
        )?
    } else if snapshot.session.objects.iter().any(|o| {
        matches!(
            o.kind,
            Kind::MosaicArea | Kind::MosaicBrush | Kind::Magnifier
        )
    }) {
        let image =
            crate::annotation_export_render::compose(snapshot, input.capture.clone(), cancelled)
                .ok()?;
        tiny_skia::Pixmap::from_vec(
            image.pixels,
            tiny_skia::IntSize::from_wh(
                (monitor.right - monitor.left) as u32,
                (monitor.bottom - monitor.top) as u32,
            )?,
        )?
    } else {
        if super::font_service::document_needs_fonts(&snapshot.session)
            && !super::font_service::ready()
        {
            return None;
        }
        let mut frame = tiny_skia::Pixmap::from_vec(
            base.pixels.clone(),
            tiny_skia::IntSize::from_wh(
                (monitor.right - monitor.left) as u32,
                (monitor.bottom - monitor.top) as u32,
            )?,
        )?;
        let overlay = scene.render_document(&snapshot.session, monitor, snapshot.scale)?;
        frame.draw_pixmap(
            0,
            0,
            overlay.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
        frame
    };
    Some(pixmap)
}
/// Compatibility path for CPU fixtures. Runtime retains shared immutable content.
#[cfg(test)]
pub(super) fn paint_frames(
    input: &Input,
    cancelled: &AtomicBool,
    cache: &mut Cache,
) -> Option<Vec<tiny_skia::Pixmap>> {
    render_frames(input, cancelled, cache).map(|frames| {
        frames
            .into_iter()
            .map(|p| Arc::try_unwrap(p).unwrap_or_else(|p| p.as_ref().clone()))
            .collect()
    })
}
/// Content is reused independently of selection and gesture feedback. No live views are read.
fn render_frames(
    input: &Input,
    cancelled: &AtomicBool,
    cache: &mut Cache,
) -> Option<Vec<Arc<tiny_skia::Pixmap>>> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    if input.crop_border {
        cache.image = None;
        return Some(vec![Arc::new(paint_cached(input, cancelled, cache, true)?)]);
    }
    cache.base = None;
    let rebuilt = cache
        .image
        .as_ref()
        .is_none_or(|image| !image.matches(input));
    let compose_started = Instant::now();
    let mut compose_ms = 0.;
    let mut finish_ms = 0.;
    if rebuilt {
        // The installed Slint picture owns its copy. Retire the old worker cache before
        // allocating a replacement, rather than retaining two complete cached versions.
        cache.image = None;
        let raw = compose_base(input, cancelled, &mut cache.scene)?;
        compose_ms = compose_started.elapsed().as_secs_f64() * 1000.;
        let started = Instant::now();
        let pixels = Arc::new(finish_frame(raw, input, cancelled, true)?);
        finish_ms = started.elapsed().as_secs_f64() * 1000.;
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        cache.image = Some(ImageBase {
            snapshot: input.snapshot.clone(),
            capture: input.capture.clone(),
            pixels,
        });
    }
    let started = Instant::now();
    let snapshot = &input.snapshot;
    let mut controls = tiny_skia::Pixmap::new(
        (snapshot.monitor.right - snapshot.monitor.left) as u32,
        (snapshot.monitor.bottom - snapshot.monitor.top) as u32,
    )?;
    crate::annotation_render::draw_interaction(
        &mut controls,
        &snapshot.session,
        snapshot.monitor,
        snapshot.scale,
        input.hovered,
    );
    if let Some((geometry, handles)) = input.erase {
        crate::annotation_render::draw_erase_interaction(
            &mut controls,
            geometry,
            handles,
            snapshot.monitor,
            snapshot.scale,
        );
    }
    let controls = Arc::new(finish_frame(controls, input, cancelled, false)?);
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let content = &cache.image.as_ref()?.pixels;
    // Only content rebuilds are diagnosed, never each mouse/selection feedback update.
    if rebuilt {
        tracing::debug!(
            compose_ms,
            finish_ms,
            feedback_ms = started.elapsed().as_secs_f64() * 1000.,
            source_bytes = input
                .capture
                .regions
                .iter()
                .map(|r| r.pixels.len())
                .sum::<usize>(),
            content_bytes = content.data().len(),
            feedback_bytes = controls.data().len(),
            "Frozen image CPU stages complete (not presentation)"
        );
    }
    let frames = vec![content.clone(), controls];
    if content.data().len() > MAX_CACHED_IMAGE_BYTES {
        cache.image = None;
    }
    Some(frames)
}
fn finish_frame(
    pixmap: tiny_skia::Pixmap,
    input: &Input,
    cancelled: &AtomicBool,
    colors: bool,
) -> Option<tiny_skia::Pixmap> {
    let snapshot = &input.snapshot;
    let full = snapshot.monitor;
    let orientation = snapshot.session.image_orientation;
    let finish_started = Instant::now();
    let mut pixmap = if orientation == Default::default() {
        pixmap
    } else {
        let image = lexift_core::ports::screen_capture::CapturedRegion {
            bounds: full,
            pixels: pixmap.take(),
        };
        let image = orientation.pixels(&image, full, cancelled).ok()?;
        tiny_skia::Pixmap::from_vec(
            image.pixels,
            tiny_skia::IntSize::from_wh(
                (image.bounds.right - image.bounds.left) as u32,
                (image.bounds.bottom - image.bounds.top) as u32,
            )?,
        )?
    };
    let orientation_ms = finish_started.elapsed().as_secs_f64() * 1000.;
    let color_started = Instant::now();
    if colors {
        snapshot
            .session
            .image_color
            .apply(pixmap.data_mut(), cancelled)
            .ok()?;
    }
    let color_ms = color_started.elapsed().as_secs_f64() * 1000.;
    let resize_started = Instant::now();
    if snapshot.session.image_scale != Default::default() {
        let visible = orientation
            .rect(snapshot.session.image_crop.unwrap_or(full), full)
            .ok()?;
        let area = tiny_skia::IntRect::from_xywh(
            visible.left - full.left,
            visible.top - full.top,
            (visible.right - visible.left) as u32,
            (visible.bottom - visible.top) as u32,
        )?;
        // Full-range crops already own the correct pixels; avoid a second full allocation.
        if area.x() != 0
            || area.y() != 0
            || area.width() != pixmap.width()
            || area.height() != pixmap.height()
        {
            pixmap = pixmap.clone_rect(area)?;
        }
        let image = snapshot
            .session
            .image_scale
            .pixels(
                lexift_core::ports::screen_capture::CapturedRegion {
                    bounds: visible,
                    pixels: pixmap.take(),
                },
                cancelled,
            )
            .ok()?;
        let size = snapshot
            .session
            .image_scale
            .bounds_size(snapshot.session.image_crop.unwrap_or(full), orientation)
            .ok()?;
        if colors {
            tracing::debug!(
                orientation_ms,
                color_ms,
                resample_ms = resize_started.elapsed().as_secs_f64() * 1000.,
                "Frozen image transform stages"
            );
        }
        return tiny_skia::Pixmap::from_vec(
            image.pixels,
            tiny_skia::IntSize::from_wh(size[0], size[1])?,
        );
    }
    if colors {
        tracing::debug!(
            orientation_ms,
            color_ms,
            resample_ms = 0.,
            "Frozen image transform stages"
        );
    }
    Some(pixmap)
}
pub(super) struct Job {
    pub submitted: Instant,
    pub generation: u64,
    pub session: u64,
    pub revision: u64,
    pub inputs: Vec<Input>,
    pub canvases: Vec<u64>,
}
#[derive(Default)]
struct QueueState {
    pending: Option<Job>,
    running: Option<(u64, Arc<AtomicBool>)>,
    stopped: bool,
    minimum_revision: u64,
}
struct Queue {
    state: Mutex<QueueState>,
    wake: Condvar,
}
impl QueueState {
    fn retire_before(&mut self, revision: u64) {
        self.minimum_revision = self.minimum_revision.max(revision);
        if let Some((running, cancel)) = &self.running
            && *running < revision
        {
            cancel.store(true, Ordering::Release);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|job| job.revision < revision)
        {
            self.pending = None;
        }
    }
    fn stop(&mut self) {
        self.stopped = true;
        self.pending = None;
        if let Some((_, cancel)) = &self.running {
            cancel.store(true, Ordering::Release);
        }
    }
}
pub(super) struct Preview {
    queue: Arc<Queue>,
}
impl Preview {
    pub fn new() -> std::io::Result<Self> {
        let queue = Arc::new(Queue {
            state: Mutex::new(QueueState::default()),
            wake: Condvar::new(),
        });
        let worker = queue.clone();
        std::thread::Builder::new()
            .name("screenshot-preview".into())
            .spawn(move || {
                let mut scenes = std::collections::HashMap::<[i32; 4], Cache>::new();
                loop {
                    let (job, cancelled) = {
                        let mut state = worker.state.lock().unwrap_or_else(|p| p.into_inner());
                        while state.pending.is_none() && !state.stopped {
                            state = worker.wake.wait(state).unwrap_or_else(|p| p.into_inner());
                        }
                        if state.stopped {
                            break;
                        }
                        let job = state.pending.take().unwrap();
                        let cancelled = Arc::new(AtomicBool::new(false));
                        state.running = Some((job.revision, cancelled.clone()));
                        (job, cancelled)
                    };
                    let queue_ms = job.submitted.elapsed().as_secs_f64() * 1000.;
                    let cpu_started = Instant::now();
                    let frames = job
                        .inputs
                        .iter()
                        .map(|input| {
                            let b = input.snapshot.monitor;
                            let scene = scenes
                                .entry([b.left, b.top, b.right, b.bottom])
                                .or_default();
                            render_frames(input, &cancelled, scene)
                        })
                        .collect::<Option<Vec<_>>>()
                        .map(|frames| frames.into_iter().flatten().collect());
                    {
                        let mut state = worker.state.lock().unwrap_or_else(|p| p.into_inner());
                        state.running = None;
                        if state.stopped {
                            break;
                        }
                    }
                    if cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    if cpu_started.elapsed() >= Duration::from_millis(100) {
                        tracing::debug!(
                            session = job.session,
                            revision = job.revision,
                            queue_ms,
                            cpu_ms = cpu_started.elapsed().as_secs_f64() * 1000.,
                            "Frozen preview slow task finished"
                        );
                    }
                    let labels = job
                        .canvases
                        .iter()
                        .copied()
                        .zip(job.inputs.iter().map(|input| {
                            input
                                .label
                                .as_ref()
                                .filter(|_| input.crop_handles)
                                .map(|l| super::screenshot::size_input::Hit {
                                    crop: input.crop,
                                    bounds: l.bounds,
                                })
                        }))
                        .collect();
                    let _ = slint::invoke_from_event_loop(move || {
                        super::frozen_editor::present_shared(
                            job.generation,
                            job.session,
                            job.revision,
                            frames,
                            labels,
                        )
                    });
                }
            })?;
        Ok(Self { queue })
    }
    pub fn submit(&self, job: Job) {
        let mut state = self.queue.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.stopped && job.revision >= state.minimum_revision {
            state.pending = Some(job);
        }
        drop(state);
        self.queue.wake.notify_one();
    }
    /// Retire explicitly cancelled/replaced candidates, not every hover update.
    /// Letting ordinary in-flight feedback finish guarantees progress under continuous input.
    pub fn retire_before(&self, revision: u64) {
        self.queue
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retire_before(revision);
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.queue
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stop();
        self.queue.wake.notify_one();
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    #[test]
    #[ignore = "CPU preview timing fixture; run explicitly on the same machine"]
    fn crop_feedback_benchmark() {
        feedback_tests::prepare_font_fixture(1.25, false);
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 720,
        };
        let mut input = Input {
            snapshot: ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor,
                scale: 1.25,
                monitors: vec![(monitor, 1.25)],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            },
            capture: Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial: 1,
                regions: vec![lexift_core::ports::screen_capture::CapturedRegion {
                    bounds: monitor,
                    pixels: [120, 160, 180, 255].repeat(1280 * 720),
                }],
            }),
            crop: Rect {
                left: 100,
                top: 100,
                right: 500,
                bottom: 400,
            },
            label: None,
            help: None,
            picker: None,
            hovered: None,
            crop_handles: true,
            crop_border: true,
            erase: None,
        };
        let mut scene = Cache::default();
        let cancelled = AtomicBool::new(false);
        let mut times = Vec::new();
        for n in 0..50 {
            input.crop.left = 100 + n * 3;
            input.crop.right = 500 + n * 3;
            input.label = crate::screen_interaction::size_label::layout(
                input.crop,
                &[(monitor, 1.25)],
                Some(monitor),
            );
            let start = Instant::now();
            std::hint::black_box(paint_frames(&input, &cancelled, &mut scene).unwrap());
            if n >= 10 {
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        times.sort_by(f64::total_cmp);
        println!(
            "CROP_FRAME_TIMING samples={} median_ms={:.3} p95_ms={:.3} max_ms={:.3}",
            times.len(),
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() - 1]
        );
    }
}

#[cfg(test)]
#[path = "screenshot_feedback_tests.rs"]
mod feedback_tests;

#[cfg(test)]
#[path = "large_image_performance_tests.rs"]
mod large_image_performance_tests;
