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
    pub hovered: Option<usize>,
    pub crop_handles: bool,
    pub crop_border: bool,
    pub erase: Option<(
        lexift_core::domain::annotation_erase::RoundedRectangle,
        bool,
    )>,
}
pub(super) fn paint(input: &Input, cancelled: &AtomicBool) -> Option<tiny_skia::Pixmap> {
    paint_cached(
        input,
        cancelled,
        &mut crate::annotation_scene::SceneCompiler::default(),
        true,
    )
}
fn paint_cached(
    input: &Input,
    cancelled: &AtomicBool,
    scene: &mut crate::annotation_scene::SceneCompiler,
    controls: bool,
) -> Option<tiny_skia::Pixmap> {
    let snapshot = &input.snapshot;
    let monitor = snapshot.monitor;
    let base = input.capture.regions.iter().find(|f| f.bounds == monitor)?;
    let mut pixmap = if !input.crop_border {
        let image = crate::annotation_export_render::compose_frozen(
            snapshot,
            input.capture.clone(),
            cancelled,
        )
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
    let width = pixmap.width() as usize;
    for (i, pixel) in pixmap
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let x = monitor.left + (i % width) as i32;
        let y = monitor.top + (i / width) as i32;
        if x < bounds.left || x >= bounds.right || y < bounds.top || y >= bounds.bottom {
            for (c, channel) in pixel.iter_mut().enumerate().take(3) {
                *channel = (f32::from(base.pixels[i * 4 + c]) * 0.45) as u8;
            }
            pixel[3] = 255;
        }
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
    Some(pixmap)
}
/// Pins keep controls in a separate transparent texture so finishing cannot bake them in.
pub(super) fn paint_frames(
    input: &Input,
    cancelled: &AtomicBool,
    scene: &mut crate::annotation_scene::SceneCompiler,
) -> Option<Vec<tiny_skia::Pixmap>> {
    let image = paint_cached(input, cancelled, scene, input.crop_border)?;
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
                let mut scenes = std::collections::HashMap::<
                    [i32; 4],
                    crate::annotation_scene::SceneCompiler,
                >::new();
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
                    let _ = slint::invoke_from_event_loop(move || {
                        super::frozen_editor::present(
                            job.generation,
                            job.session,
                            job.revision,
                            frames,
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
