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
    #[cfg(test)]
    pub builds: usize,
}
struct Base {
    snapshot: ExportSnapshot,
    capture: Arc<ScreenCaptureFrame>,
    composed: tiny_skia::Pixmap,
    dark: Arc<tiny_skia::Pixmap>,
}
impl Base {
    fn matches(&self, input: &Input) -> bool {
        let a = &self.snapshot;
        let b = &input.snapshot;
        Arc::ptr_eq(&self.capture, &input.capture)
            && a.monitor == b.monitor
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
/// Pins keep controls in a separate transparent texture so finishing cannot bake them in.
pub(super) fn paint_frames(
    input: &Input,
    cancelled: &AtomicBool,
    cache: &mut Cache,
) -> Option<Vec<tiny_skia::Pixmap>> {
    let image = paint_cached(input, cancelled, cache, input.crop_border)?;
    if input.crop_border {
        return Some(vec![image]);
    }
    let snapshot = &input.snapshot;
    let mut controls = tiny_skia::Pixmap::new(image.width(), image.height())?;
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
    Some(vec![image, controls])
}
pub(super) struct Job {
    pub generation: u64,
    pub session: u64,
    pub revision: u64,
    pub inputs: Vec<Input>,
    pub canvases: Vec<u64>,
}
struct Queue {
    job: Mutex<Option<Job>>,
    wake: Condvar,
    cancelled: AtomicBool,
}
pub(super) struct Preview {
    queue: Arc<Queue>,
}
impl Preview {
    pub fn new() -> std::io::Result<Self> {
        let queue = Arc::new(Queue {
            job: Mutex::new(None),
            wake: Condvar::new(),
            cancelled: AtomicBool::new(false),
        });
        let worker = queue.clone();
        std::thread::Builder::new()
            .name("screenshot-preview".into())
            .spawn(move || {
                let mut scenes = std::collections::HashMap::<[i32; 4], Cache>::new();
                loop {
                    let job = {
                        let mut slot = worker.job.lock().unwrap_or_else(|p| p.into_inner());
                        while slot.is_none() && !worker.cancelled.load(Ordering::Acquire) {
                            slot = worker.wake.wait(slot).unwrap_or_else(|p| p.into_inner());
                        }
                        if worker.cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        slot.take().unwrap()
                    };
                    let frames = job
                        .inputs
                        .iter()
                        .map(|input| {
                            let b = input.snapshot.monitor;
                            let scene = scenes
                                .entry([b.left, b.top, b.right, b.bottom])
                                .or_default();
                            paint_frames(input, &worker.cancelled, scene)
                        })
                        .collect::<Option<Vec<_>>>()
                        .map(|frames| frames.into_iter().flatten().collect());
                    if worker.cancelled.load(Ordering::Acquire) {
                        break;
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
                        super::frozen_editor::present(
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
        *self.queue.job.lock().unwrap_or_else(|p| p.into_inner()) = Some(job);
        self.queue.wake.notify_one();
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.queue.cancelled.store(true, Ordering::Release);
        self.queue
            .job
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
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
