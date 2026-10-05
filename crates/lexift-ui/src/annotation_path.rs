//! Desktop contour normalization shared by text and tiled effect masks.
use lexift_core::domain::geometry::Rect;
use tiny_skia::{Path, PathBuilder, PathSegment, Transform};
pub(crate) const COVERAGE_CELL: i32 = 256;
const CELL_GUARD: i32 = 4;
const CELL_SIZE: u32 = (COVERAGE_CELL + CELL_GUARD * 2) as u32;

fn target_bounds(target: &tiny_skia::Pixmap, origin: Rect) -> Rect {
    Rect {
        left: origin.left,
        top: origin.top,
        right: (i64::from(origin.left) + i64::from(target.width())).min(i64::from(i32::MAX)) as i32,
        bottom: (i64::from(origin.top) + i64::from(target.height())).min(i64::from(i32::MAX))
            as i32,
    }
}

/// Both colored overlays and alpha masks must use the same desktop clipping
/// origins. The guard is outside the cropped cell and never blends twice.
fn visit_cells(visible: Rect, mut visit: impl FnMut(Rect, Rect)) {
    for cy in visible.top.div_euclid(COVERAGE_CELL)..=(visible.bottom - 1).div_euclid(COVERAGE_CELL)
    {
        for cx in
            visible.left.div_euclid(COVERAGE_CELL)..=(visible.right - 1).div_euclid(COVERAGE_CELL)
        {
            let left = i64::from(cx) * i64::from(COVERAGE_CELL);
            let top = i64::from(cy) * i64::from(COVERAGE_CELL);
            let cell = Rect {
                left: left as i32,
                top: top as i32,
                right: (left + i64::from(COVERAGE_CELL)).min(i64::from(i32::MAX)) as i32,
                bottom: (top + i64::from(COVERAGE_CELL)).min(i64::from(i32::MAX)) as i32,
            };
            let Some(piece) = intersect(cell, visible) else {
                continue;
            };
            let Some(left) = cell.left.checked_sub(CELL_GUARD) else {
                continue;
            };
            let Some(top) = cell.top.checked_sub(CELL_GUARD) else {
                continue;
            };
            visit(Rect { left, top, ..cell }, piece);
        }
    }
}

/// Draw colored feedback at canonical desktop origins, cropping before blending.
/// Empty space outside `geometry` never allocates a monitor-sized temporary.
pub(crate) fn draw_cells(
    target: &mut tiny_skia::Pixmap,
    origin: Rect,
    geometry: Rect,
    mut draw: impl FnMut(&mut tiny_skia::Pixmap, Rect),
) {
    let roi = target_bounds(target, origin);
    let Some(visible) = intersect(roi, geometry) else {
        return;
    };
    let Some(mut tile) = tiny_skia::Pixmap::new(CELL_SIZE, CELL_SIZE) else {
        return;
    };
    visit_cells(visible, |cell, piece| {
        tile.fill(tiny_skia::Color::TRANSPARENT);
        draw(&mut tile, cell);
        let width = target.width() as usize;
        for y in piece.top..piece.bottom {
            for x in piece.left..piece.right {
                let source =
                    ((y - cell.top) as usize * CELL_SIZE as usize + (x - cell.left) as usize) * 4;
                let destination = ((y - roi.top) as usize * width + (x - roi.left) as usize) * 4;
                let rgba = &tile.data()[source..source + 4];
                if rgba[3] == 0 {
                    continue;
                }
                let pixel = &mut target.data_mut()[destination..destination + 4];
                let remain = 255 - u32::from(rgba[3]);
                for channel in 0..4 {
                    pixel[channel] = (u32::from(rgba[channel])
                        + (u32::from(pixel[channel]) * remain + 127) / 255)
                        .min(255) as u8;
                }
            }
        }
    });
}

pub(crate) fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// Apply a desktop transform and normalize curves before integer viewport
/// translation. All coordinates and approximation tolerances are physical pixels.
pub(crate) fn normalize(path: Path, transform: Transform, monitor: Rect) -> Option<Path> {
    let path = path.transform(transform)?;
    let point = |p: tiny_skia::Point| {
        (
            (p.x * 256.).round() / 256. - monitor.left as f32,
            (p.y * 256.).round() / 256. - monitor.top as f32,
        )
    };
    let mut builder = PathBuilder::new();
    let mut previous = tiny_skia::Point::from_xy(0., 0.);
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                let (x, y) = point(p);
                builder.move_to(x, y);
                previous = p;
            }
            PathSegment::LineTo(p) => {
                let (x, y) = point(p);
                builder.line_to(x, y);
                previous = p;
            }
            PathSegment::QuadTo(p, q) => {
                let first = tiny_skia::Point::from_xy(
                    previous.x + (p.x - previous.x) * 2. / 3.,
                    previous.y + (p.y - previous.y) * 2. / 3.,
                );
                let second = tiny_skia::Point::from_xy(
                    q.x + (p.x - q.x) * 2. / 3.,
                    q.y + (p.y - q.y) * 2. / 3.,
                );
                flatten_curve([previous, first, second, q], 0, &mut |p| {
                    let (x, y) = point(p);
                    builder.line_to(x, y);
                });
                previous = q;
            }
            PathSegment::CubicTo(p, q, r) => {
                flatten_curve([previous, p, q, r], 0, &mut |p| {
                    let (x, y) = point(p);
                    builder.line_to(x, y);
                });
                previous = r;
            }
            PathSegment::Close => builder.close(),
        }
    }
    builder.finish()
}

/// Flatten before viewport clipping: translated cubic clipping can choose
/// different edge coverage. The 1/64px tolerance is independent of tile origin.
fn flatten_curve(
    points: [tiny_skia::Point; 4],
    depth: u8,
    emit: &mut impl FnMut(tiny_skia::Point),
) {
    let [a, b, c, d] = points;
    let dx = f64::from(d.x - a.x);
    let dy = f64::from(d.y - a.y);
    let length = dx * dx + dy * dy;
    let distance = |p: tiny_skia::Point| {
        let x = f64::from(p.x - a.x);
        let y = f64::from(p.y - a.y);
        if length < 1e-12 {
            x * x + y * y
        } else {
            let t = ((x * dx + y * dy) / length).clamp(0., 1.);
            (x - t * dx).powi(2) + (y - t * dy).powi(2)
        }
    };
    if depth >= 16 || distance(b).max(distance(c)) <= (1f64 / 64.).powi(2) {
        emit(d);
        return;
    }
    let midpoint = |p: tiny_skia::Point, q: tiny_skia::Point| {
        tiny_skia::Point::from_xy((p.x + q.x) * 0.5, (p.y + q.y) * 0.5)
    };
    let ab = midpoint(a, b);
    let bc = midpoint(b, c);
    let cd = midpoint(c, d);
    let abc = midpoint(ab, bc);
    let bcd = midpoint(bc, cd);
    let middle = midpoint(abc, bcd);
    flatten_curve([a, ab, abc, middle], depth + 1, emit);
    flatten_curve([middle, bcd, cd, d], depth + 1, emit);
}

/// Paint coverage on canonical desktop cells, then crop to the requested ROI.
pub(crate) fn paint_coverage(
    target: &mut tiny_skia::Pixmap,
    origin: Rect,
    geometry: Rect,
    paint: &tiny_skia::Paint,
    mut draw: impl FnMut(&mut tiny_skia::Pixmap, Rect, &tiny_skia::Paint),
) {
    // Canonical desktop cells give the fixed-point scan converter identical
    // clipping origins in whole-frame and partial-frame calls. Crop afterwards.
    let Some(mut coverage) = tiny_skia::Pixmap::new(CELL_SIZE, CELL_SIZE) else {
        return;
    };
    let mut white = tiny_skia::Paint::default();
    white.set_color_rgba8(255, 255, 255, 255);
    white.anti_alias = true;
    let Some(mut mask) = tiny_skia::Mask::new(target.width(), target.height()) else {
        return;
    };
    let roi = target_bounds(target, origin);
    let Some(visible) = intersect(roi, geometry) else {
        return;
    };
    visit_cells(visible, |cell, piece| {
        coverage.fill(tiny_skia::Color::TRANSPARENT);
        draw(&mut coverage, cell, &white);
        for y in piece.top..piece.bottom {
            for x in piece.left..piece.right {
                let source =
                    ((y - cell.top) as usize * CELL_SIZE as usize + (x - cell.left) as usize) * 4
                        + 3;
                let output =
                    (y - roi.top) as usize * target.width() as usize + (x - roi.left) as usize;
                mask.data_mut()[output] = coverage.data()[source];
            }
        }
    });
    if let Some(rect) =
        tiny_skia::Rect::from_xywh(0., 0., target.width() as f32, target.height() as f32)
    {
        target.fill_rect(rect, paint, Transform::identity(), Some(&mask));
    }
}
