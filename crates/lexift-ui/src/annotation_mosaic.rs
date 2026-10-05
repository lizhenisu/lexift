//! Effect sampling and round privacy masks in physical desktop coordinates.
use crate::annotation_path::COVERAGE_CELL;
pub(crate) use crate::annotation_path::intersect;
use lexift_core::{
    domain::{
        annotation::{Bounds, Extra, Kind, MosaicEffect, Object},
        geometry::Rect,
    },
    ports::screen_capture::ScreenCaptureFrame,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Clip the visible mask before adding filter padding. A large off-screen
/// object must not allocate an equally large pixel frame or filter buffer.
pub(crate) fn sample_bounds(
    object: &Object,
    monitors: impl IntoIterator<Item = Rect>,
) -> Option<Rect> {
    let Extra::Mosaic(settings) = &object.extra else {
        return None;
    };
    if settings.strength == 0 {
        return None;
    }
    let bounds = Rect {
        left: object.bounds.left.floor() as i32,
        top: object.bounds.top.floor() as i32,
        right: object.bounds.right.ceil() as i32,
        bottom: object.bounds.bottom.ceil() as i32,
    };
    let visible = monitors
        .into_iter()
        .filter_map(|monitor| intersect(bounds, monitor))
        .reduce(|a, b| Rect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })?;
    let p = settings.sample_padding().max(1);
    Some(Rect {
        left: visible.left.saturating_sub(p),
        top: visible.top.saturating_sub(p),
        right: visible.right.saturating_add(p),
        bottom: visible.bottom.saturating_add(p),
    })
}

/// Partition long brush paths into disjoint output cells with independent filter
/// margins. Empty cells inside a winding path's bounding box are never processed.
pub(crate) fn work_regions(object: &Object, monitors: &[Rect]) -> Vec<(Rect, Rect)> {
    let Some(sample) = sample_bounds(object, monitors.iter().copied()) else {
        return Vec::new();
    };
    let Extra::Mosaic(settings) = &object.extra else {
        return Vec::new();
    };
    let padding = settings.sample_padding().max(1);
    let visible = Rect {
        left: sample.left + padding,
        top: sample.top + padding,
        right: sample.right - padding,
        bottom: sample.bottom - padding,
    };
    const CELL: i32 = COVERAGE_CELL;
    if object.kind != Kind::MosaicBrush
        || (visible.right - visible.left <= CELL && visible.bottom - visible.top <= CELL)
    {
        return vec![(visible, sample)];
    }
    let radius = settings.diameter * 0.5 + 1.;
    let mut cells = std::collections::BTreeSet::new();
    let Some(&first) = object.points.first() else {
        return Vec::new();
    };
    for (a, b) in
        std::iter::once((first, first)).chain(object.points.windows(2).map(|p| (p[0], p[1])))
    {
        let envelope = Rect {
            left: (a.0.min(b.0) - radius).floor() as i32,
            top: (a.1.min(b.1) - radius).floor() as i32,
            right: (a.0.max(b.0) + radius).ceil() as i32,
            bottom: (a.1.max(b.1) + radius).ceil() as i32,
        };
        for monitor in monitors {
            let Some(envelope) = intersect(envelope, *monitor).and_then(|r| intersect(r, visible))
            else {
                continue;
            };
            for cy in envelope.top.div_euclid(CELL)..=(envelope.bottom - 1).div_euclid(CELL) {
                for cx in envelope.left.div_euclid(CELL)..=(envelope.right - 1).div_euclid(CELL) {
                    let cell = Rect {
                        left: cx * CELL,
                        top: cy * CELL,
                        right: (cx + 1) * CELL,
                        bottom: (cy + 1) * CELL,
                    };
                    let Some(body) = intersect(cell, visible).and_then(|r| intersect(r, *monitor))
                    else {
                        continue;
                    };
                    let expanded = Bounds {
                        left: body.left as f32 - radius,
                        top: body.top as f32 - radius,
                        right: body.right as f32 + radius,
                        bottom: body.bottom as f32 + radius,
                    };
                    if clip_segment(a, b, expanded).is_some() {
                        cells.insert((body.top, body.left, body.bottom, body.right));
                    }
                }
            }
        }
    }
    cells
        .into_iter()
        .map(|(top, left, bottom, right)| {
            let body = Rect {
                left,
                top,
                right,
                bottom,
            };
            (
                body,
                Rect {
                    left: left.saturating_sub(padding),
                    top: top.saturating_sub(padding),
                    right: right.saturating_add(padding),
                    bottom: bottom.saturating_add(padding),
                },
            )
        })
        .collect()
}

#[cfg(test)]
pub(super) fn draw(
    overlay: &mut Pixmap,
    object: &Object,
    monitor: Rect,
    frame: &ScreenCaptureFrame,
) {
    if let Some(tile) = prepare(
        overlay,
        object,
        monitor,
        frame,
        &|| false,
        &mut Scratch::default(),
    ) {
        blit(overlay, monitor, &tile);
    }
}

#[derive(Default)]
pub(super) struct Scratch {
    sampled: Vec<u8>,
    filtered: Vec<u8>,
    intermediate: Vec<f32>,
}

pub(crate) struct Tile {
    pub index: usize,
    pub object: Object,
    pub bounds: Rect,
    pub pixels: Pixmap,
    filtered: Pixmap,
    sampled_mask: Rect,
    feedback_bounds: Rect,
    pub visible_monitors: Vec<Rect>,
    pub raster_key: u64,
}

impl Tile {
    /// Hash immutable sampled output once on the worker, including data used by
    /// remasking a growing draft. Capture sequence numbers are not visual state.
    pub(super) fn finalize_raster_key(&mut self) {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for value in [
            self.object.bounds.left,
            self.object.bounds.top,
            self.object.bounds.right,
            self.object.bounds.bottom,
        ] {
            value.to_bits().hash(&mut hash);
        }
        for rect in std::iter::once(&self.bounds)
            .chain([&self.sampled_mask, &self.feedback_bounds])
            .chain(self.visible_monitors.iter())
        {
            [rect.left, rect.top, rect.right, rect.bottom].hash(&mut hash);
        }
        self.pixels.data().hash(&mut hash);
        self.filtered.data().hash(&mut hash);
        self.raster_key = hash.finish();
    }
}

pub(super) fn blit(overlay: &mut Pixmap, monitor: Rect, tile: &Tile) {
    overlay.draw_pixmap(
        tile.bounds.left - monitor.left,
        tile.bounds.top - monitor.top,
        tile.pixels.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Keep the completed mask visible while a newer sample is processing. The
/// caller validates the original object against the active placement gesture.
pub(super) fn blit_transformed(
    overlay: &mut Pixmap,
    monitor: Rect,
    tile: &Tile,
    current: lexift_core::domain::annotation::Bounds,
) {
    let old = tile.object.bounds;
    if old.width() <= 0. || old.height() <= 0. {
        return;
    }
    let sx = current.width() / old.width();
    let sy = current.height() / old.height();
    let tx = current.left + (tile.bounds.left as f32 - old.left) * sx - monitor.left as f32;
    let ty = current.top + (tile.bounds.top as f32 - old.top) * sy - monitor.top as f32;
    if ![sx, sy, tx, ty].into_iter().all(f32::is_finite) {
        return;
    }
    overlay.draw_pixmap(
        0,
        0,
        tile.pixels.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::from_row(sx, 0., 0., sy, tx, ty),
        None,
    );
}

pub(super) fn prepare(
    overlay: &Pixmap,
    object: &Object,
    monitor: Rect,
    frame: &ScreenCaptureFrame,
    cancelled: &dyn Fn() -> bool,
    scratch: &mut Scratch,
) -> Option<Tile> {
    let Extra::Mosaic(settings) = &object.extra else {
        return None;
    };
    if settings.strength == 0 {
        return None;
    }
    // Capture can contain a larger ROI from another object or an earlier pose.
    // Filter only this mask's visible bounds plus its required filter padding.
    let source = frame.regions.iter().find(|r| {
        r.bounds.left <= monitor.left
            && r.bounds.top <= monitor.top
            && r.bounds.right >= monitor.right
            && r.bounds.bottom >= monitor.bottom
    })?;
    let area = monitor;
    let w = (area.right - area.left) as usize;
    let h = (area.bottom - area.top) as usize;
    if overlay.width() as usize != w || overlay.height() as usize != h {
        return None;
    }
    let source_width = (source.bounds.right - source.bounds.left) as usize;
    scratch.sampled.clear();
    scratch.sampled.reserve(w * h * 4);
    for y in area.top..area.bottom {
        if cancelled() {
            return None;
        }
        let start = ((y - source.bounds.top) as usize * source_width
            + (area.left - source.bounds.left) as usize)
            * 4;
        scratch
            .sampled
            .extend_from_slice(source.pixels.get(start..start + w * 4)?);
    }
    let sampled = &mut scratch.sampled;
    // The current overlay contains exactly the marks below this object. Control
    // handles are drawn afterwards, so they can never feed back into the effect.
    for y in 0..h {
        if cancelled() {
            return None;
        }
        for x in 0..w {
            let target = (y * w + x) * 4;
            let a = overlay.data()[target + 3] as u32;
            for c in 0..4 {
                sampled[target + c] = (overlay.data()[target + c] as u32
                    + (sampled[target + c] as u32 * (255 - a) + 127) / 255)
                    .min(255) as u8;
            }
        }
    }
    match settings.effect {
        MosaicEffect::Pixelate => pixelate_cancel(
            sampled,
            area,
            settings.block_size() as i32,
            cancelled,
            &mut scratch.filtered,
        )?,
        MosaicEffect::Blur => gaussian_cancel(
            sampled,
            w,
            h,
            settings.blur_sigma(),
            cancelled,
            &mut scratch.filtered,
            &mut scratch.intermediate,
        )?,
    };
    let filtered = &scratch.filtered;
    let padding = settings.sample_padding().max(1);
    let sampled_mask = Rect {
        left: area.left.saturating_add(padding),
        top: area.top.saturating_add(padding),
        right: area.right.saturating_sub(padding),
        bottom: area.bottom.saturating_sub(padding),
    };
    if sampled_mask.right <= sampled_mask.left || sampled_mask.bottom <= sampled_mask.top {
        return None;
    }
    if cancelled() {
        return None;
    }
    let mut mask = Pixmap::new(w as u32, h as u32)?;
    let mut white = Paint::default();
    white.set_color_rgba8(255, 255, 255, 255);
    let origin = area;
    paint_mask(&mut mask, object, origin, &white);
    let mut output = Pixmap::new(w as u32, h as u32)?;
    for y in sampled_mask.top..sampled_mask.bottom {
        if cancelled() {
            return None;
        }
        for x in sampled_mask.left..sampled_mask.right {
            let src = ((y - origin.top) as usize * w + (x - origin.left) as usize) * 4;
            let coverage = mask.data()[src + 3] as u32;
            if coverage == 0 {
                continue;
            }
            for c in 0..4 {
                output.data_mut()[src + c] =
                    ((filtered[src + c] as u32 * coverage + 127) / 255) as u8;
            }
        }
    }
    let mut texture = Pixmap::new(w as u32, h as u32)?;
    texture.data_mut().copy_from_slice(filtered);
    Some(Tile {
        index: 0,
        object: object.clone(),
        bounds: area,
        pixels: output,
        filtered: texture,
        sampled_mask,
        feedback_bounds: Rect {
            left: source.bounds.left.saturating_add(padding),
            top: source.bounds.top.saturating_add(padding),
            right: source.bounds.right.saturating_sub(padding),
            bottom: source.bounds.bottom.saturating_sub(padding),
        },
        visible_monitors: Vec::new(),
        raster_key: 0,
    })
}

/// Keep filter margins in the texture but publish each mask pixel only once.
pub(super) fn clip_output(tile: &mut Tile, body: Rect) -> Option<()> {
    tile.sampled_mask = intersect(tile.sampled_mask, body)?;
    let width = tile.pixels.width() as usize;
    for (y, row) in tile
        .pixels
        .data_mut()
        .chunks_exact_mut(width * 4)
        .enumerate()
    {
        let global_y = tile.bounds.top + y as i32;
        if global_y < body.top || global_y >= body.bottom {
            row.fill(0);
            continue;
        }
        let left = (body.left - tile.bounds.left).clamp(0, width as i32) as usize * 4;
        let right = (body.right - tile.bounds.left).clamp(0, width as i32) as usize * 4;
        row[..left].fill(0);
        row[right..].fill(0);
    }
    Some(())
}

/// Internal processing edges are not missing pixels. Use the valid bounds of
/// the complete capture snapshot for draft caps; output remains clipped to each
/// tile's own sampled pixels, so gaps in the snapshot are never extrapolated.
pub(super) fn set_snapshot_feedback(
    tile: &mut Tile,
    frame: &ScreenCaptureFrame,
    object: &Object,
    monitors: &[Rect],
) {
    let Some(requested) = sample_bounds(object, monitors.iter().copied()) else {
        return;
    };
    let Extra::Mosaic(settings) = &object.extra else {
        return;
    };
    let padding = settings.sample_padding().max(1);
    if let Some(bounds) = frame
        .regions
        .iter()
        .filter_map(|r| {
            let valid = Rect {
                left: r.bounds.left.saturating_add(padding),
                top: r.bounds.top.saturating_add(padding),
                right: r.bounds.right.saturating_sub(padding),
                bottom: r.bounds.bottom.saturating_sub(padding),
            };
            intersect(valid, requested)
        })
        .reduce(|a, b| Rect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })
    {
        tile.feedback_bounds = bounds;
    }
}

/// Apply a completed sample to the latest draft path, clipped to pixels that
/// were actually captured. Never extrapolate an unsampled privacy region.
pub(super) fn blit_current_mask(overlay: &mut Pixmap, monitor: Rect, tile: &Tile, object: &Object) {
    let Some(shared) = intersect(monitor, tile.sampled_mask) else {
        return;
    };
    let Some(mut output) = Pixmap::new(
        (shared.right - shared.left) as u32,
        (shared.bottom - shared.top) as u32,
    ) else {
        return;
    };
    let paint = Paint {
        shader: tiny_skia::Pattern::new(
            tile.filtered.as_ref(),
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Nearest,
            1.,
            Transform::from_translate(
                (tile.bounds.left - shared.left) as f32,
                (tile.bounds.top - shared.top) as f32,
            ),
        ),
        ..Paint::default()
    };
    let visible = if tile.visible_monitors.is_empty() {
        std::slice::from_ref(&monitor)
    } else {
        &tile.visible_monitors
    };
    paint_visible_mask(
        &mut output,
        object,
        shared,
        tile.feedback_bounds,
        visible,
        &paint,
    );
    overlay.draw_pixmap(
        shared.left - monitor.left,
        shared.top - monitor.top,
        output.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Pending feedback ends at the sampled geometry with a complete cap/corner.
/// This changes only its temporary mask, never the saved path or effect pixels.
fn paint_visible_mask(
    target: &mut Pixmap,
    object: &Object,
    origin: Rect,
    available: Rect,
    visible: &[Rect],
    paint: &Paint<'_>,
) {
    let Extra::Mosaic(settings) = &object.extra else {
        return;
    };
    // Only sampling frontiers shorten feedback. Physical display boundaries
    // crop naturally, and the same monitor set is used by every canvas so no
    // extra cap/corner is introduced at a screen seam.
    let left = visible.iter().any(|m| {
        m.left < available.left
            && m.right >= available.left
            && m.top < available.bottom
            && m.bottom > available.top
    });
    let right = visible.iter().any(|m| {
        m.right > available.right
            && m.left <= available.right
            && m.top < available.bottom
            && m.bottom > available.top
    });
    let top = visible.iter().any(|m| {
        m.top < available.top
            && m.bottom >= available.top
            && m.left < available.right
            && m.right > available.left
    });
    let bottom = visible.iter().any(|m| {
        m.bottom > available.bottom
            && m.top <= available.bottom
            && m.left < available.right
            && m.right > available.left
    });
    if object.kind != Kind::MosaicBrush {
        let mut preview = object.clone();
        if left {
            preview.bounds.left = preview.bounds.left.max(available.left as f32);
        }
        if top {
            preview.bounds.top = preview.bounds.top.max(available.top as f32);
        }
        if right {
            preview.bounds.right = preview.bounds.right.min(available.right as f32);
        }
        if bottom {
            preview.bounds.bottom = preview.bounds.bottom.min(available.bottom as f32);
        }
        if preview.bounds.width() > 0. && preview.bounds.height() > 0. {
            paint_mask(target, &preview, origin, paint);
        }
        return;
    }
    let radius = settings.diameter / 2.;
    let center_bounds = Bounds {
        left: available.left as f32 + if left { radius } else { -radius },
        top: available.top as f32 + if top { radius } else { -radius },
        right: available.right as f32 - if right { radius } else { -radius },
        bottom: available.bottom as f32 - if bottom { radius } else { -radius },
    };
    if center_bounds.left > center_bounds.right || center_bounds.top > center_bounds.bottom {
        return;
    }
    let inside = |p: &(f32, f32)| {
        p.0 >= center_bounds.left
            && p.0 <= center_bounds.right
            && p.1 >= center_bounds.top
            && p.1 <= center_bounds.bottom
    };
    if object.points.iter().all(inside) {
        paint_mask(target, object, origin, paint);
        return;
    }
    let mut path = PathBuilder::new();
    let mut previous_end = None;
    let mut dot = None;
    let mut has_line = false;
    for pair in object.points.windows(2) {
        let Some((a, b)) = clip_segment(pair[0], pair[1], center_bounds) else {
            previous_end = None;
            continue;
        };
        if a == b {
            dot = Some(a);
            continue;
        }
        if previous_end != Some(a) {
            path.move_to(a.0 - origin.left as f32, a.1 - origin.top as f32);
        }
        path.line_to(b.0 - origin.left as f32, b.1 - origin.top as f32);
        previous_end = Some(b);
        has_line = true;
    }
    if has_line {
        if let Some(path) = path.finish() {
            target.stroke_path(
                &path,
                paint,
                &Stroke {
                    width: settings.diameter,
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    } else if let Some(p) = dot {
        path.push_circle(p.0 - origin.left as f32, p.1 - origin.top as f32, radius);
        if let Some(path) = path.finish() {
            target.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// Liang–Barsky clipping also handles zero-width center ranges (a single fully
/// sampled brush cap). Separate re-entry pieces never bridge unsampled pixels.
fn clip_segment(a: (f32, f32), b: (f32, f32), bounds: Bounds) -> Option<((f32, f32), (f32, f32))> {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let mut enter = 0f32;
    let mut leave = 1f32;
    for (p, q) in [
        (-dx, a.0 - bounds.left),
        (dx, bounds.right - a.0),
        (-dy, a.1 - bounds.top),
        (dy, bounds.bottom - a.1),
    ] {
        if p == 0. {
            if q < 0. {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0. {
                enter = enter.max(t);
            } else {
                leave = leave.min(t);
            }
        }
        if enter > leave {
            return None;
        }
    }
    Some((
        (a.0 + enter * dx, a.1 + enter * dy),
        (a.0 + leave * dx, a.1 + leave * dy),
    ))
}

#[cfg(test)]
fn paint_available_mask(
    target: &mut Pixmap,
    object: &Object,
    origin: Rect,
    available: Rect,
    paint: &Paint<'_>,
) {
    paint_visible_mask(
        target,
        object,
        origin,
        available,
        std::slice::from_ref(&origin),
        paint,
    );
}

pub(crate) fn paint_mask(target: &mut Pixmap, object: &Object, origin: Rect, paint: &Paint<'_>) {
    // Pending feedback may replace points before its saved bounds are updated.
    let mut bounds = object.bounds;
    if object.kind == Kind::MosaicBrush
        && let Extra::Mosaic(settings) = &object.extra
        && let Some(&first) = object.points.first()
    {
        bounds = Bounds::from_corners(first, first);
        for &(x, y) in &object.points {
            bounds.left = bounds.left.min(x);
            bounds.right = bounds.right.max(x);
            bounds.top = bounds.top.min(y);
            bounds.bottom = bounds.bottom.max(y);
        }
        let radius = settings.diameter * 0.5;
        bounds.left -= radius;
        bounds.right += radius;
        bounds.top -= radius;
        bounds.bottom += radius;
    }
    let geometry = Rect {
        left: (bounds.left - 1.).floor() as i32,
        top: (bounds.top - 1.).floor() as i32,
        right: (bounds.right + 1.).ceil() as i32,
        bottom: (bounds.bottom + 1.).ceil() as i32,
    };
    crate::annotation_path::paint_coverage(
        target,
        origin,
        geometry,
        paint,
        |target, origin, white| paint_mask_contour(target, object, origin, white),
    );
}

fn paint_mask_contour(target: &mut Pixmap, object: &Object, origin: Rect, paint: &Paint<'_>) {
    let Extra::Mosaic(settings) = &object.extra else {
        return;
    };
    if object.kind == Kind::MosaicBrush {
        if let Some(first) = object.points.first() {
            let mut path = PathBuilder::new();
            if object.points.len() == 1 {
                path.push_circle(first.0, first.1, settings.diameter / 2.);
                if let Some(path) = path.finish().and_then(|path| {
                    crate::annotation_path::normalize(path, Transform::identity(), origin)
                }) {
                    target.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
                }
            } else {
                path.move_to(first.0, first.1);
                for p in &object.points[1..] {
                    path.line_to(p.0, p.1);
                }
                if let Some(path) = path
                    .finish()
                    .and_then(|path| {
                        path.stroke(
                            &Stroke {
                                width: settings.diameter,
                                line_cap: tiny_skia::LineCap::Round,
                                line_join: tiny_skia::LineJoin::Round,
                                ..Stroke::default()
                            },
                            1.,
                        )
                    })
                    .and_then(|path| {
                        crate::annotation_path::normalize(path, Transform::identity(), origin)
                    })
                {
                    target.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
                }
            }
        }
    } else if let Some(path) = super::path(
        object.bounds,
        Kind::Rectangle,
        object.style.rounding * settings.creation_scale,
        Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    )
    .and_then(|path| crate::annotation_path::normalize(path, Transform::identity(), origin))
    {
        target.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

#[cfg(test)]
fn pixelate(pixels: &[u8], bounds: Rect, block: i32) -> Vec<u8> {
    let mut result = Vec::new();
    pixelate_cancel(pixels, bounds, block, &|| false, &mut result).unwrap();
    result
}
fn pixelate_cancel(
    pixels: &[u8],
    bounds: Rect,
    block: i32,
    cancelled: &dyn Fn() -> bool,
    result: &mut Vec<u8>,
) -> Option<()> {
    if block <= 1 {
        result.clear();
        result.extend_from_slice(pixels);
        return Some(());
    }
    let width = (bounds.right - bounds.left) as usize;
    result.resize(pixels.len(), 0);
    for cy in bounds.top.div_euclid(block)..=(bounds.bottom - 1).div_euclid(block) {
        if cancelled() {
            return None;
        }
        for cx in bounds.left.div_euclid(block)..=(bounds.right - 1).div_euclid(block) {
            let cell = Rect {
                left: (cx * block).max(bounds.left),
                top: (cy * block).max(bounds.top),
                right: ((cx + 1) * block).min(bounds.right),
                bottom: ((cy + 1) * block).min(bounds.bottom),
            };
            let mut sum = [0u64; 4];
            let count = ((cell.right - cell.left) * (cell.bottom - cell.top)) as u64;
            for y in cell.top..cell.bottom {
                for x in cell.left..cell.right {
                    let i = ((y - bounds.top) as usize * width + (x - bounds.left) as usize) * 4;
                    for c in 0..4 {
                        sum[c] += pixels[i + c] as u64;
                    }
                }
            }
            let color = sum.map(|s| ((s + count / 2) / count) as u8);
            for y in cell.top..cell.bottom {
                for x in cell.left..cell.right {
                    let i = ((y - bounds.top) as usize * width + (x - bounds.left) as usize) * 4;
                    result[i..i + 4].copy_from_slice(&color);
                }
            }
        }
    }
    Some(())
}

#[cfg(test)]
fn gaussian(pixels: &[u8], width: usize, height: usize, sigma: f32) -> Vec<u8> {
    let mut result = Vec::new();
    gaussian_cancel(
        pixels,
        width,
        height,
        sigma,
        &|| false,
        &mut result,
        &mut Vec::new(),
    )
    .unwrap();
    result
}
fn gaussian_cancel(
    pixels: &[u8],
    width: usize,
    height: usize,
    sigma: f32,
    cancelled: &dyn Fn() -> bool,
    result: &mut Vec<u8>,
    intermediate: &mut Vec<f32>,
) -> Option<()> {
    if sigma <= 0. {
        result.clear();
        result.extend_from_slice(pixels);
        return Some(());
    }
    let radius = (sigma * 3.).ceil() as isize;
    let mut weights: Vec<f32> = (-radius..=radius)
        .map(|x| (-(x * x) as f32 / (2. * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    for weight in &mut weights {
        *weight /= total;
    }
    let row_len = width * 4;
    intermediate.resize(pixels.len() + row_len, 0.);
    let (horizontal, row) = intermediate.split_at_mut(pixels.len());
    result.resize(pixels.len(), 0);
    // Accumulate four independent lanes in the same tap order. Sharing the
    // clamped sample index lets LLVM vectorize RGBA without changing the kernel
    // or substituting a box-filter approximation at large radii.
    for y in 0..height {
        if cancelled() {
            return None;
        }
        for x in 0..width {
            let mut sums = [0.; 4];
            for (offset, weight) in (-radius..=radius).zip(&weights) {
                let px = (x as isize + offset).clamp(0, width as isize - 1) as usize;
                let source = (y * width + px) * 4;
                for c in 0..4 {
                    sums[c] += pixels[source + c] as f32 * weight;
                }
            }
            let target = (y * width + x) * 4;
            horizontal[target..target + 4].copy_from_slice(&sums);
        }
    }
    // Traverse complete rows for each vertical tap, keeping reads contiguous
    // instead of revisiting widely separated cache lines for every pixel. The
    // extra accumulation row shares the reusable intermediate allocation.
    for y in 0..height {
        if cancelled() {
            return None;
        }
        row.fill(0.);
        for (offset, weight) in (-radius..=radius).zip(&weights) {
            let py = (y as isize + offset).clamp(0, height as isize - 1) as usize;
            let source = &horizontal[py * row_len..(py + 1) * row_len];
            for (sum, value) in row.iter_mut().zip(source) {
                *sum += value * weight;
            }
        }
        for (target, value) in result[y * row_len..(y + 1) * row_len]
            .iter_mut()
            .zip(row.iter())
        {
            *target = value.round().clamp(0., 255.) as u8;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::annotation::{Bounds, MosaicAnnotation, Style},
        ports::screen_capture::CapturedRegion,
    };
    // Independent scalar reference retained for byte-exact regression and local
    // CPU timing. It is not compiled into the application.
    fn gaussian_scalar_reference(
        pixels: &[u8],
        width: usize,
        height: usize,
        sigma: f32,
    ) -> Vec<u8> {
        let mut result = Vec::new();
        let mut intermediate = Vec::new();
        gaussian_scalar_into(pixels, width, height, sigma, &mut result, &mut intermediate);
        result
    }
    fn gaussian_scalar_into(
        pixels: &[u8],
        width: usize,
        height: usize,
        sigma: f32,
        result: &mut Vec<u8>,
        intermediate: &mut Vec<f32>,
    ) {
        if sigma <= 0. {
            result.clear();
            result.extend_from_slice(pixels);
            return;
        }
        let radius = (sigma * 3.).ceil() as isize;
        let mut weights: Vec<f32> = (-radius..=radius)
            .map(|x| (-(x * x) as f32 / (2. * sigma * sigma)).exp())
            .collect();
        let total: f32 = weights.iter().sum();
        for weight in &mut weights {
            *weight /= total;
        }
        intermediate.resize(pixels.len(), 0.);
        intermediate.fill(0.);
        result.resize(pixels.len(), 0);
        for y in 0..height {
            for x in 0..width {
                for c in 0..4 {
                    let i = (y * width + x) * 4 + c;
                    for (offset, weight) in (-radius..=radius).zip(&weights) {
                        let px = (x as isize + offset).clamp(0, width as isize - 1) as usize;
                        intermediate[i] += pixels[(y * width + px) * 4 + c] as f32 * weight;
                    }
                }
            }
        }
        for y in 0..height {
            for x in 0..width {
                for c in 0..4 {
                    let i = (y * width + x) * 4 + c;
                    let mut value = 0.;
                    for (offset, weight) in (-radius..=radius).zip(&weights) {
                        let py = (y as isize + offset).clamp(0, height as isize - 1) as usize;
                        value += intermediate[(py * width + x) * 4 + c] * weight;
                    }
                    result[i] = value.round().clamp(0., 255.) as u8;
                }
            }
        }
    }

    #[test]
    fn gaussian_rgba_lanes_match_scalar_reference_including_edges_and_reused_buffers() {
        let mut filtered = Vec::new();
        let mut intermediate = Vec::new();
        for (width, height) in [(1, 1), (1, 17), (17, 1), (7, 9), (63, 47)] {
            let pixels: Vec<u8> = (0..width * height * 4)
                .map(|i| ((i * 73 + i / 7 * 11) % 256) as u8)
                .collect();
            for sigma in [0., 0.5, 1., 3.3, 20., 25.] {
                gaussian_cancel(
                    &pixels,
                    width,
                    height,
                    sigma,
                    &|| false,
                    &mut filtered,
                    &mut intermediate,
                )
                .unwrap();
                assert_eq!(
                    filtered,
                    gaussian_scalar_reference(&pixels, width, height, sigma),
                    "{width}x{height}, sigma={sigma}"
                );
            }
        }
    }

    #[test]
    #[ignore = "opt-in local release CPU timing"]
    fn gaussian_release_cpu_timing() {
        let (width, height) = (480, 300);
        let pixels: Vec<u8> = (0..width * height * 4)
            .map(|i| ((i * 73 + i / 7 * 11) % 256) as u8)
            .collect();
        let mut filtered = Vec::new();
        let mut intermediate = Vec::new();
        let mut scalar_filtered = Vec::new();
        let mut scalar_intermediate = Vec::new();
        let start = std::time::Instant::now();
        for _ in 0..3 {
            gaussian_scalar_into(
                std::hint::black_box(&pixels),
                width,
                height,
                25.,
                &mut scalar_filtered,
                &mut scalar_intermediate,
            );
            std::hint::black_box(&scalar_filtered);
        }
        let scalar = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..3 {
            gaussian_cancel(
                std::hint::black_box(&pixels),
                width,
                height,
                25.,
                &|| false,
                &mut filtered,
                &mut intermediate,
            )
            .unwrap();
            std::hint::black_box(&filtered);
        }
        eprintln!(
            "Gaussian 480x300 sigma25, 3 frames: scalar={scalar:?}, rgba={:?}",
            start.elapsed()
        );
    }

    #[test]
    fn round_brush_masks_only_its_footprint_and_samples_lower_marks() {
        let monitor = Rect {
            left: -4,
            top: -4,
            right: 4,
            bottom: 4,
        };
        let source = CapturedRegion {
            bounds: monitor,
            pixels: [0, 100, 0, 255].repeat(64),
        };
        let frame = ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![source],
        };
        let object = Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-1., -1.), (1., 1.)),
            points: vec![(0., 0.)],
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 2., 1.)),
        };
        let mut overlay = Pixmap::new(8, 8).unwrap();
        draw(&mut overlay, &object, monitor, &frame);
        assert_eq!(&overlay.data()[0..4], &[0, 0, 0, 0]);
        let center = (4 * 8 + 4) * 4;
        assert!(overlay.data()[center + 1] > 0);
        overlay.fill(tiny_skia::Color::from_rgba8(200, 0, 0, 255));
        draw(&mut overlay, &object, monitor, &frame);
        assert_eq!(&overlay.data()[center..center + 4], &[200, 0, 0, 255]);
    }
    #[test]
    fn cropped_filter_preserves_visible_pixels_and_avoids_larger_capture_buffers() {
        let full = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let frame = ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds: full,
                pixels: (full.top..full.bottom)
                    .flat_map(|y| {
                        (full.left..full.right).flat_map(move |x| {
                            [
                                (x * 7 + y * 3).rem_euclid(256) as u8,
                                (x * 3 - y * 11).rem_euclid(256) as u8,
                                90,
                                255,
                            ]
                        })
                    })
                    .collect(),
            }],
        };
        for effect in [MosaicEffect::Pixelate, MosaicEffect::Blur] {
            for scale in [1., 1.25] {
                let object = Object {
                    id: Default::default(),
                    kind: Kind::MosaicArea,
                    bounds: Bounds::from_corners((-12.5, -9.5), (8.5, 7.5)),
                    points: vec![],
                    style: Style {
                        rounding: 3.,
                        ..Style::default()
                    },
                    extra: Extra::Mosaic(MosaicAnnotation::new(effect, 35, 12., scale)),
                };
                let roi = sample_bounds(&object, [full]).unwrap();
                let mut scratch = Scratch::default();
                let cropped = prepare(
                    &Pixmap::new((roi.right - roi.left) as u32, (roi.bottom - roi.top) as u32)
                        .unwrap(),
                    &object,
                    roi,
                    &frame,
                    &|| false,
                    &mut scratch,
                )
                .unwrap();
                assert_eq!(cropped.bounds, roi);
                assert_eq!(scratch.sampled.len(), cropped.pixels.data().len());
                assert!(scratch.sampled.len() < frame.regions[0].pixels.len() / 3);
                let reference = prepare(
                    &Pixmap::new(200, 200).unwrap(),
                    &object,
                    full,
                    &frame,
                    &|| false,
                    &mut Scratch::default(),
                )
                .unwrap();
                for y in roi.top..roi.bottom {
                    for x in roi.left..roi.right {
                        assert_eq!(
                            cropped
                                .pixels
                                .pixel((x - roi.left) as u32, (y - roi.top) as u32),
                            reference
                                .pixels
                                .pixel((x - full.left) as u32, (y - full.top) as u32),
                            "{effect:?} {scale} at {x},{y}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn draft_feedback_uses_current_path_without_extrapolating_capture() {
        let roi = Rect {
            left: -8,
            top: -8,
            right: 8,
            bottom: 8,
        };
        let frame = ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds: roi,
                pixels: [10, 20, 30, 255].repeat(256),
            }],
        };
        let mut object = Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-1., -1.), (1., 1.)),
            points: vec![(0., 0.)],
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 2., 1.)),
        };
        let tile = prepare(
            &Pixmap::new(16, 16).unwrap(),
            &object,
            roi,
            &frame,
            &|| false,
            &mut Scratch::default(),
        )
        .unwrap();
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        object.points = vec![(-5., 0.)];
        let mut output = Pixmap::new(32, 32).unwrap();
        blit_current_mask(&mut output, monitor, &tile, &object);
        assert!(
            output.pixel(11, 16).unwrap().alpha() >= 200,
            "one-pixel-radius caps retain antialias coverage"
        );
        assert_eq!(
            output.pixel(16, 16).unwrap().alpha(),
            0,
            "discarded draft path is not baked into the texture"
        );
        object.points = vec![(0., 0.), (12., 0.)];
        let mut output = Pixmap::new(32, 32).unwrap();
        blit_current_mask(&mut output, monitor, &tile, &object);
        assert!(
            output.pixel(21, 16).unwrap().alpha() > 0,
            "new path within sampled bounds uses a complete antialiased cap"
        );
        assert_eq!(
            output.pixel(28, 16).unwrap().alpha(),
            0,
            "uncaptured pixels must not be filled by edge extrapolation"
        );
    }

    #[test]
    fn partial_feedback_keeps_caps_and_corners_round_without_changing_saved_geometry() {
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        let available = Rect {
            left: -6,
            top: -6,
            right: 6,
            bottom: 6,
        };
        let mut white = Paint::default();
        white.set_color_rgba8(255, 255, 255, 255);
        for scale in [1., 1.25] {
            let mut object = Object {
                id: Default::default(),
                kind: Kind::MosaicBrush,
                bounds: Bounds::from_corners((-10., -10.), (12., 8.)),
                points: vec![(0., 0.), (12., 0.)],
                style: Style {
                    rounding: 3.,
                    ..Style::default()
                },
                extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 4., scale)),
            };
            let saved = object.clone();
            let mut output = Pixmap::new(32, 32).unwrap();
            paint_available_mask(&mut output, &object, monitor, available, &white);
            assert_eq!(output.pixel(19, 16).unwrap().alpha(), 255);
            assert!(
                output.pixel(21, 17).unwrap().alpha() < 180,
                "a clipped pending brush still ends in a circular cap"
            );
            assert_eq!(object, saved);
            object.kind = Kind::MosaicArea;
            let mut output = Pixmap::new(32, 32).unwrap();
            paint_available_mask(&mut output, &object, monitor, available, &white);
            assert_eq!(output.pixel(16, 16).unwrap().alpha(), 255);
            assert!(
                output.pixel(21, 21).unwrap().alpha() < 128,
                "a pending region keeps its lower-right corner round"
            );
            for y in -16..16 {
                for x in -16..16 {
                    if x < available.left
                        || x >= available.right
                        || y < available.top
                        || y >= available.bottom
                    {
                        assert_eq!(
                            output
                                .pixel((x + 16) as u32, (y + 16) as u32)
                                .unwrap()
                                .alpha(),
                            0
                        );
                    }
                }
            }
            let full = Rect {
                left: -20,
                top: -20,
                right: 20,
                bottom: 20,
            };
            for kind in [Kind::MosaicArea, Kind::MosaicBrush] {
                object.kind = kind;
                let mut actual = Pixmap::new(32, 32).unwrap();
                let mut reference = Pixmap::new(32, 32).unwrap();
                paint_available_mask(&mut actual, &object, monitor, full, &white);
                paint_mask(&mut reference, &object, monitor, &white);
                assert_eq!(
                    actual.data(),
                    reference.data(),
                    "fully sampled geometry must retain byte-identical rendering"
                );
            }
        }
    }

    #[test]
    fn clipped_feedback_does_not_join_separate_reentry_pieces() {
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        let available = Rect {
            left: -6,
            top: -6,
            right: 6,
            bottom: 6,
        };
        let object = Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-6., -2.), (12., 12.)),
            points: vec![(-4., 0.), (10., 0.), (10., 10.), (0., 0.)],
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 4., 1.)),
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        let mut output = Pixmap::new(32, 32).unwrap();
        paint_available_mask(&mut output, &object, monitor, available, &paint);
        assert!(
            output.pixel(21, 18).unwrap().alpha() < 128,
            "re-entry must not add a vertical connector"
        );
        assert_eq!(output.pixel(16, 16).unwrap().alpha(), 255);
        let point = Bounds::from_corners((0., 0.), (0., 0.));
        assert_eq!(
            clip_segment((0., 0.), (10., 0.), point),
            Some(((0., 0.), (0., 0.)))
        );
        assert!(clip_segment((-10., 2.), (10., 2.), point).is_none());
    }

    #[test]
    fn partial_feedback_preserves_natural_display_edges_and_screen_seams() {
        let viewport = Rect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        let available = Rect {
            left: 0,
            top: 0,
            right: 6,
            bottom: 6,
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        let mut object = Object {
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-10., -10.), (12., 12.)),
            points: vec![(0., 0.), (12., 0.)],
            style: Style {
                rounding: 3.,
                ..Style::default()
            },
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 4., 1.)),
        };
        for kind in [Kind::MosaicBrush, Kind::MosaicArea] {
            object.kind = kind;
            let mut output = Pixmap::new(32, 32).unwrap();
            paint_visible_mask(
                &mut output,
                &object,
                viewport,
                available,
                &[viewport],
                &paint,
            );
            assert_eq!(
                output.pixel(0, 0).unwrap().alpha(),
                255,
                "a display boundary naturally crops a cap or corner; it is not a missing-sample frontier"
            );
        }
        let full = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        let left = Rect { right: 0, ..full };
        let right = Rect { left: 0, ..full };
        let available = Rect {
            left: -2,
            top: -10,
            right: 2,
            bottom: 10,
        };
        for kind in [Kind::MosaicArea, Kind::MosaicBrush] {
            object.kind = kind;
            object.points = vec![(-12., 0.), (12., 0.)];
            let mut expected = Pixmap::new(32, 32).unwrap();
            paint_visible_mask(
                &mut expected,
                &object,
                full,
                available,
                &[left, right],
                &paint,
            );
            for monitor in [left, right] {
                let mut actual = Pixmap::new(16, 32).unwrap();
                paint_visible_mask(
                    &mut actual,
                    &object,
                    monitor,
                    available,
                    &[left, right],
                    &paint,
                );
                for y in 0..32 {
                    for x in 0..16 {
                        let full_x = (monitor.left - full.left) as u32 + x;
                        let a = actual.pixel(x, y).unwrap().alpha();
                        let b = expected.pixel(full_x, y).unwrap().alpha();
                        assert!(
                            a.abs_diff(b) <= 1,
                            "no extra cap/corner is allowed at the monitor seam"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn grid_is_anchored_to_negative_desktop_coordinates() {
        let pixels: Vec<_> = (0..8).flat_map(|x| [x, 0, 0, 255]).collect();
        let bounds = Rect {
            left: -4,
            top: 0,
            right: 4,
            bottom: 1,
        };
        let result = pixelate(&pixels, bounds, 4);
        assert_eq!(result[0], 2);
        assert_eq!(result[12], 2);
        assert_eq!(result[16], 6);
        assert_eq!(result[28], 6);
    }
    #[test]
    fn offscreen_mask_allocates_only_visible_bounds_and_filter_padding() {
        let settings = MosaicAnnotation::new(MosaicEffect::Blur, 100, 12., 1.25);
        let padding = settings.sample_padding();
        let mut object = Object {
            id: Default::default(),
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((-100000., -100000.), (100000., 100000.)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::Mosaic(settings),
        };
        let monitor = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(
            sample_bounds(&object, [monitor]),
            Some(Rect {
                left: monitor.left - padding,
                top: monitor.top - padding,
                right: monitor.right + padding,
                bottom: monitor.bottom + padding,
            })
        );
        object.bounds = Bounds::from_corners((100., 100.), (200., 200.));
        assert!(sample_bounds(&object, [monitor]).is_none());
    }
    #[test]
    fn gaussian_is_symmetric_and_zero_strength_is_exact() {
        let mut pixels = vec![0; 9 * 9 * 4];
        for p in pixels.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        pixels[(4 * 9 + 4) * 4] = 255;
        assert_eq!(gaussian(&pixels, 9, 9, 0.), pixels);
        let blurred = gaussian(&pixels, 9, 9, 1.);
        assert!(blurred[(4 * 9 + 4) * 4] < 255);
        assert_eq!(blurred[(4 * 9 + 3) * 4], blurred[(4 * 9 + 5) * 4]);
        assert_eq!(blurred[(3 * 9 + 4) * 4], blurred[(5 * 9 + 4) * 4]);
    }
}
