//! Stateless software composition shared by preview and background export.
use crate::annotation_render::{self, mosaic_worker};
use lexift_core::{
    Error, Result,
    domain::{annotation_export::ExportSnapshot as Snapshot, geometry::Rect},
    ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub struct SoftwareExportRenderer;
impl lexift_core::ports::annotation_export::AnnotationExportRenderPort for SoftwareExportRenderer {
    fn regions(&self, snapshot: &Snapshot) -> Vec<Rect> {
        regions(snapshot)
    }
    fn compose(
        &self,
        snapshot: &Snapshot,
        capture: Arc<ScreenCaptureFrame>,
        cancelled: &AtomicBool,
    ) -> Result<CapturedRegion> {
        compose(snapshot, capture, cancelled)
    }
}
pub(crate) fn regions(snapshot: &Snapshot) -> Vec<Rect> {
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
/// Compose a frozen document using its source DPI partitions, irrespective of view zoom.
pub(crate) fn compose_frozen(
    snapshot: &Snapshot,
    capture: Arc<ScreenCaptureFrame>,
    cancelled: &AtomicBool,
) -> Result<CapturedRegion> {
    let mut rendered = Vec::new();
    for (monitor, scale) in lexift_core::domain::screen_snapshot::render_partitions(
        snapshot.monitor,
        &snapshot.monitors,
        snapshot.scale,
    ) {
        let mut part = snapshot.clone();
        part.monitor = monitor;
        part.scale = scale;
        rendered.push(compose(&part, capture.clone(), cancelled)?);
    }
    lexift_core::domain::screen_snapshot::crop_frames(&rendered, snapshot.monitor)
}

pub(crate) fn compose(
    snapshot: &Snapshot,
    capture: Arc<ScreenCaptureFrame>,
    cancelled: &AtomicBool,
) -> Result<CapturedRegion> {
    if super::font_service::document_needs_fonts(&snapshot.session) && !super::font_service::ready()
    {
        return Err(Error::new("annotation fonts are not ready"));
    }
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
