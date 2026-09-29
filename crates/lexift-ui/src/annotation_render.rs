//! Rasterizes the transient annotation document into a transparent monitor surface.
use lexift_core::domain::{
    annotation::{Bounds, Endpoint, Extra, Kind, MagnifierConnector, Object, Session},
    geometry::Rect,
};
#[cfg(test)]
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Rect as SkRect, Stroke, StrokeDash, Transform,
};

fn path(bounds: Bounds, kind: Kind, rounding: f32, monitor: Rect) -> Option<Path> {
    let (l, t, r, b) = (
        bounds.left - monitor.left as f32,
        bounds.top - monitor.top as f32,
        bounds.right - monitor.left as f32,
        bounds.bottom - monitor.top as f32,
    );
    let rect = SkRect::from_ltrb(l, t, r, b)?;
    if matches!(kind, Kind::Ellipse | Kind::SpotlightEllipse) {
        return PathBuilder::from_oval(rect);
    }
    let radius = rounding
        .min(bounds.width() / 2.)
        .min(bounds.height() / 2.)
        .max(0.);
    if radius < 0.5 {
        return Some(PathBuilder::from_rect(rect));
    }
    let k = 0.552_284_8 * radius;
    let mut p = PathBuilder::new();
    p.move_to(l + radius, t);
    p.line_to(r - radius, t);
    p.cubic_to(r - radius + k, t, r, t + radius - k, r, t + radius);
    p.line_to(r, b - radius);
    p.cubic_to(r, b - radius + k, r - radius + k, b, r - radius, b);
    p.line_to(l + radius, b);
    p.cubic_to(l + radius - k, b, l, b - radius + k, l, b - radius);
    p.line_to(l, t + radius);
    p.cubic_to(l, t + radius - k, l + radius - k, t, l + radius, t);
    p.close();
    p.finish()
}

fn color_paint(color: [u8; 3]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], 255);
    p.anti_alias = true;
    p
}

fn draw_brush(pixmap: &mut Pixmap, object: &Object, monitor: Rect, scale: f32) {
    let Some(&(x, y)) = object.points.first() else {
        return;
    };
    let mut paint = color_paint(object.style.color);
    if object.kind == Kind::HighlightLine {
        let [r, g, b] = object.style.color;
        paint.set_color_rgba8(r, g, b, 77);
    }
    let width = object.style.width * scale;
    if object.points.iter().all(|p| (p.0 - x).hypot(p.1 - y) < 0.1) {
        if let Some(circle) =
            PathBuilder::from_circle(x - monitor.left as f32, y - monitor.top as f32, width / 2.)
        {
            pixmap.fill_path(
                &circle,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }
    let mut builder = PathBuilder::new();
    builder.move_to(x - monitor.left as f32, y - monitor.top as f32);
    for &(x, y) in &object.points[1..] {
        builder.line_to(x - monitor.left as f32, y - monitor.top as f32);
    }
    let Some(path) = builder.finish() else { return };
    let dash = if object.kind == Kind::HighlightLine {
        0
    } else {
        object.style.dash
    };
    let intervals = match dash {
        1 => Some(vec![3. * width, 2. * width]),
        2 => Some(vec![0.05 * width, 2. * width]),
        3 => Some(vec![3. * width, 2. * width, 0.05 * width, 2. * width]),
        4 => Some(vec![
            3. * width,
            2. * width,
            0.05 * width,
            2. * width,
            0.05 * width,
            2. * width,
        ]),
        _ => None,
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            dash: intervals.and_then(|v| StrokeDash::new(v, 0.)),
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
}

fn draw_object(pixmap: &mut Pixmap, object: &Object, monitor: Rect, scale: f32) {
    if matches!(object.kind, Kind::Arrow | Kind::Polyline) {
        draw_line_annotation(pixmap, object, monitor, scale);
        return;
    }
    if object.kind == Kind::Magnifier {
        return;
    }
    if matches!(object.kind, Kind::Pencil | Kind::HighlightLine) {
        draw_brush(pixmap, object, monitor, scale);
        return;
    }
    let Some(path) = path(
        object.bounds,
        object.kind,
        object.style.rounding * scale,
        monitor,
    ) else {
        return;
    };
    let mut paint = color_paint(object.style.color);
    if object.kind == Kind::HighlightRectangle {
        let [r, g, b] = object.style.color;
        paint.set_color_rgba8(r, g, b, 77);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        return;
    }
    if object.style.fill && !object.kind.is_spotlight() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if object.style.outline && (!object.style.fill || object.kind.is_spotlight()) {
        let width = (object.style.width * scale).max(1.);
        let intervals = match object.style.dash {
            1 => Some(vec![3. * width, 2. * width]),
            2 => Some(vec![width, width]),
            3 => Some(vec![3. * width, 2. * width, width, 2. * width]),
            4 => Some(vec![
                3. * width,
                2. * width,
                width,
                2. * width,
                width,
                2. * width,
            ]),
            _ => None,
        };
        let stroke = Stroke {
            width,
            dash: intervals.and_then(|v| StrokeDash::new(v, 0.)),
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn annotate_point(point: (f32, f32), monitor: Rect) -> (f32, f32) {
    (point.0 - monitor.left as f32, point.1 - monitor.top as f32)
}

fn draw_endpoint(
    pixmap: &mut Pixmap,
    point: (f32, f32),
    direction: (f32, f32),
    endpoint: Endpoint,
    width: f32,
    color: [u8; 3],
    monitor: Rect,
) {
    if endpoint == Endpoint::None {
        return;
    }
    let (x, y) = annotate_point(point, monitor);
    let length = direction.0.hypot(direction.1).max(0.001);
    let (ux, uy) = (direction.0 / length, direction.1 / length);
    let (vx, vy) = (-uy, ux);
    let size = (width * 3.).clamp(9., 38.);
    let mut builder = PathBuilder::new();
    let (fill, close) = match endpoint {
        Endpoint::Circle | Endpoint::OpenCircle => {
            if let Some(circle) = PathBuilder::from_circle(x, y, size * 0.35) {
                let paint = color_paint(color);
                if endpoint == Endpoint::Circle {
                    pixmap.fill_path(
                        &circle,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                } else {
                    pixmap.stroke_path(
                        &circle,
                        &paint,
                        &Stroke {
                            width: width.max(1.),
                            ..Stroke::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
            return;
        }
        Endpoint::Bar => {
            builder.move_to(x + vx * size * 0.5, y + vy * size * 0.5);
            builder.line_to(x - vx * size * 0.5, y - vy * size * 0.5);
            (false, false)
        }
        Endpoint::Diamond | Endpoint::OpenDiamond => {
            builder.move_to(x + ux * size * 0.5, y + uy * size * 0.5);
            builder.line_to(x + vx * size * 0.3, y + vy * size * 0.3);
            builder.line_to(x - ux * size * 0.5, y - uy * size * 0.5);
            builder.line_to(x - vx * size * 0.3, y - vy * size * 0.3);
            (endpoint == Endpoint::Diamond, true)
        }
        _ => {
            let back = (x - ux * size, y - uy * size);
            builder.move_to(back.0 + vx * size * 0.45, back.1 + vy * size * 0.45);
            builder.line_to(x, y);
            builder.line_to(back.0 - vx * size * 0.45, back.1 - vy * size * 0.45);
            (endpoint == Endpoint::FilledArrow, true)
        }
    };
    if close {
        builder.close();
    }
    if let Some(path) = builder.finish() {
        let paint = color_paint(color);
        if fill {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        } else {
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: width.max(1.),
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
}

fn draw_line_annotation(pixmap: &mut Pixmap, object: &Object, monitor: Rect, scale: f32) {
    let points = object.stroke_points();
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::new();
    let (x, y) = annotate_point(points[0], monitor);
    builder.move_to(x, y);
    for &point in &points[1..] {
        let (x, y) = annotate_point(point, monitor);
        builder.line_to(x, y);
    }
    if object.style.fill && object.kind == Kind::Polyline && points.len() >= 3 {
        builder.close();
    }
    let Some(path) = builder.finish() else { return };
    let paint = color_paint(object.style.color);
    if object.style.fill && object.kind == Kind::Polyline {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        let width = (object.style.width * scale).max(1.);
        let intervals = match object.style.dash {
            1 => Some(vec![3. * width, 2. * width]),
            2 => Some(vec![width, width]),
            3 => Some(vec![3. * width, 2. * width, width, 2. * width]),
            4 => Some(vec![
                3. * width,
                2. * width,
                width,
                2. * width,
                width,
                2. * width,
            ]),
            _ => None,
        };
        pixmap.stroke_path(
            &path,
            &paint,
            &Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: intervals.and_then(|intervals| StrokeDash::new(intervals, 0.)),
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
    let (start, end, head) = match object.extra {
        Extra::Arrow {
            start, end, head, ..
        } => (start, end, head),
        Extra::Polyline {
            start, end, head, ..
        } => (start, end, head),
        _ => (Endpoint::None, Endpoint::None, false),
    };
    let size = (object.style.width * scale).max(1.);
    let a = points[0];
    let b = points[1];
    if head {
        draw_endpoint(
            pixmap,
            a,
            (a.0 - b.0, a.1 - b.1),
            start,
            size,
            object.style.color,
            monitor,
        );
    }
    let n = points.len();
    let a = points[n - 1];
    let b = points[n - 2];
    if head {
        draw_endpoint(
            pixmap,
            a,
            (a.0 - b.0, a.1 - b.1),
            if end == Endpoint::None {
                Endpoint::FilledArrow
            } else {
                end
            },
            size,
            object.style.color,
            monitor,
        );
    }
}

/// Intersects the center-to-center ray with a rectangular or elliptical frame.
fn frame_anchor(bounds: Bounds, ellipse: bool, toward: (f32, f32)) -> (f32, f32) {
    let center = (
        (bounds.left + bounds.right) / 2.,
        (bounds.top + bounds.bottom) / 2.,
    );
    let (dx, dy) = (toward.0 - center.0, toward.1 - center.1);
    let (rx, ry) = (
        (bounds.width() / 2.).max(0.5),
        (bounds.height() / 2.).max(0.5),
    );
    if dx.abs() + dy.abs() < f32::EPSILON {
        return center;
    }
    let distance = if ellipse {
        1. / ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt()
    } else {
        (rx / dx.abs().max(f32::EPSILON)).min(ry / dy.abs().max(f32::EPSILON))
    };
    (center.0 + dx * distance, center.1 + dy * distance)
}

fn inside_frame(bounds: Bounds, ellipse: bool, point: (f32, f32)) -> bool {
    let center = (
        (bounds.left + bounds.right) / 2.,
        (bounds.top + bounds.bottom) / 2.,
    );
    let x = (point.0 - center.0) / (bounds.width() / 2.).max(0.5);
    let y = (point.1 - center.1) / (bounds.height() / 2.).max(0.5);
    if ellipse {
        x * x + y * y <= 1. + f32::EPSILON
    } else {
        x.abs() <= 1. + f32::EPSILON && y.abs() <= 1. + f32::EPSILON
    }
}

/// The connector must stop at the output frame and disappear when its source is covered.
fn magnifier_connector_segment(
    source: Bounds,
    output: Bounds,
    ellipse: bool,
    connector: MagnifierConnector,
) -> Option<((f32, f32), (f32, f32))> {
    if connector == MagnifierConnector::None {
        return None;
    }
    let source_center = (
        (source.left + source.right) / 2.,
        (source.top + source.bottom) / 2.,
    );
    if inside_frame(output, ellipse, source_center) {
        return None;
    }
    let output_center = (
        (output.left + output.right) / 2.,
        (output.top + output.bottom) / 2.,
    );
    let start = if connector == MagnifierConnector::Frame {
        frame_anchor(source, ellipse, output_center)
    } else {
        source_center
    };
    if inside_frame(output, ellipse, start) {
        return None;
    }
    Some((start, frame_anchor(output, ellipse, source_center)))
}

/// Composites only Lexift marks. The native magnifier always excludes every
/// annotation HWND, preventing its own guide and handles from being sampled.
fn draw_magnified_marks(
    pixmap: &mut Pixmap,
    session: &Session,
    draft: Option<&Object>,
    object: &Object,
    monitor: Rect,
) {
    let Extra::Magnifier {
        output,
        ellipse,
        antialias,
        erase_annotations,
        ..
    } = object.extra
    else {
        return;
    };
    if erase_annotations
        || output.right <= monitor.left as f32
        || output.left >= monitor.right as f32
        || output.bottom <= monitor.top as f32
        || output.top >= monitor.bottom as f32
    {
        return;
    }
    let source = object.bounds;
    let sample = Rect {
        left: source.left.floor() as i32,
        top: source.top.floor() as i32,
        right: source.right.ceil() as i32,
        bottom: source.bottom.ceil() as i32,
    };
    let width = (output.right.ceil() - output.left.floor()) as u32;
    let height = (output.bottom.ceil() - output.top.floor()) as u32;
    if width == 0 || height == 0 || width as u64 * height as u64 > 40_000_000 {
        return;
    }
    let mut marks = Session::new();
    marks.spotlight_opacity = session.spotlight_opacity;
    marks.objects = session
        .objects
        .iter()
        .filter(|o| o.kind != Kind::Magnifier)
        .cloned()
        .collect();
    let draft = draft.filter(|o| o.kind != Kind::Magnifier);
    if marks.objects.is_empty() && draft.is_none() {
        return;
    }
    let Some(sampled) = render(&marks, sample, 1., draft, None) else {
        return;
    };
    let Some(mut enlarged) = Pixmap::new(width, height) else {
        return;
    };
    let paint = PixmapPaint {
        quality: if antialias {
            FilterQuality::Bilinear
        } else {
            FilterQuality::Nearest
        },
        ..PixmapPaint::default()
    };
    enlarged.draw_pixmap(
        0,
        0,
        sampled.as_ref(),
        &paint,
        Transform::from_scale(
            width as f32 / sampled.width() as f32,
            height as f32 / sampled.height() as f32,
        ),
        None,
    );
    if ellipse
        && let (Some(mut mask), Some(rect)) = (
            Mask::new(width, height),
            SkRect::from_xywh(0., 0., width as f32, height as f32),
        )
        && let Some(oval) = PathBuilder::from_oval(rect)
    {
        mask.fill_path(&oval, FillRule::Winding, true, Transform::identity());
        enlarged.apply_mask(&mask);
    }
    pixmap.draw_pixmap(
        output.left.floor() as i32 - monitor.left,
        output.top.floor() as i32 - monitor.top,
        enlarged.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

fn draw_magnifier_overlay(pixmap: &mut Pixmap, object: &Object, monitor: Rect, scale: f32) {
    let Extra::Magnifier {
        output,
        ellipse,
        shadow,
        connector,
        ..
    } = object.extra
    else {
        return;
    };
    let paint = color_paint(object.style.color);
    let width = (object.style.width * scale).max(1.);
    let source = path(
        object.bounds,
        if ellipse {
            Kind::Ellipse
        } else {
            Kind::Rectangle
        },
        0.,
        monitor,
    );
    let destination = path(
        output,
        if ellipse {
            Kind::Ellipse
        } else {
            Kind::Rectangle
        },
        0.,
        monitor,
    );
    if let Some(source) = source {
        if connector == MagnifierConnector::Frame {
            pixmap.stroke_path(
                &source,
                &paint,
                &Stroke {
                    width,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        } else {
            let dash = StrokeDash::new(vec![2. * scale, 2. * scale], 0.);
            let mut halo = color_paint([255, 255, 255]);
            halo.set_color_rgba8(255, 255, 255, 190);
            pixmap.stroke_path(
                &source,
                &halo,
                &Stroke {
                    width: (2. * scale).max(1.),
                    dash: dash.clone(),
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
            let mut guide = color_paint([38, 42, 48]);
            guide.set_color_rgba8(38, 42, 48, 220);
            pixmap.stroke_path(
                &source,
                &guide,
                &Stroke {
                    width: scale.max(1.),
                    dash,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    if shadow {
        draw_magnifier_shadow(pixmap, output, ellipse, monitor, scale);
    }
    if let Some((start, end)) =
        magnifier_connector_segment(object.bounds, output, ellipse, connector)
        && let Some(line) = {
            let mut b = PathBuilder::new();
            let (x, y) = annotate_point(start, monitor);
            b.move_to(x, y);
            let (x, y) = annotate_point(end, monitor);
            b.line_to(x, y);
            b.finish()
        }
    {
        pixmap.stroke_path(
            &line,
            &paint,
            &Stroke {
                width,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
        if connector == MagnifierConnector::Dot
            && let Some(dot) = PathBuilder::from_circle(
                start.0 - monitor.left as f32,
                start.1 - monitor.top as f32,
                width.max(4.) * 1.3,
            )
        {
            pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    if let Some(destination) = destination {
        pixmap.stroke_path(
            &destination,
            &paint,
            &Stroke {
                width,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// Blurs a local shape mask rather than a widened stroke, so the shadow is
/// soft around the whole frame without covering the live content inside it.
fn draw_magnifier_shadow(
    pixmap: &mut Pixmap,
    output: Bounds,
    ellipse: bool,
    monitor: Rect,
    scale: f32,
) {
    let radius = (5. * scale).round().clamp(1., 20.) as usize;
    let offset_x = (2. * scale).round() as i32;
    let offset_y = (3. * scale).round() as i32;
    let padding = (radius * 2) as i32 + offset_x.max(offset_y);
    let left = output.left.floor() as i32 - padding;
    let top = output.top.floor() as i32 - padding;
    let right = output.right.ceil() as i32 + padding;
    let bottom = output.bottom.ceil() as i32 + padding;
    let width = (right - left) as u32;
    let height = (bottom - top) as u32;
    if width == 0 || height == 0 || width as u64 * height as u64 > 10_000_000 {
        return;
    }
    let local = Rect {
        left,
        top,
        right,
        bottom,
    };
    let kind = if ellipse {
        Kind::Ellipse
    } else {
        Kind::Rectangle
    };
    let shifted = Bounds {
        left: output.left + offset_x as f32,
        top: output.top + offset_y as f32,
        right: output.right + offset_x as f32,
        bottom: output.bottom + offset_y as f32,
    };
    let (Some(shifted_path), Some(interior_path)) = (
        path(shifted, kind, 0., local),
        path(output, kind, 0., local),
    ) else {
        return;
    };
    let (Some(mut shadow_mask), Some(mut interior_mask), Some(mut shadow_layer)) = (
        Mask::new(width, height),
        Mask::new(width, height),
        Pixmap::new(width, height),
    ) else {
        return;
    };
    shadow_mask.fill_path(
        &shifted_path,
        FillRule::Winding,
        true,
        Transform::identity(),
    );
    blur_alpha_mask(
        shadow_mask.data_mut(),
        width as usize,
        height as usize,
        radius,
    );
    interior_mask.fill_path(
        &interior_path,
        FillRule::Winding,
        true,
        Transform::identity(),
    );
    for (shadow, inside) in shadow_mask
        .data_mut()
        .iter_mut()
        .zip(interior_mask.data().iter())
    {
        *shadow = ((*shadow as u16 * (255 - *inside as u16)) / 255) as u8;
    }
    let mut black = color_paint([0, 0, 0]);
    black.set_color_rgba8(0, 0, 0, 100);
    if let Some(rect) = SkRect::from_xywh(0., 0., width as f32, height as f32) {
        shadow_layer.fill_rect(rect, &black, Transform::identity(), Some(&shadow_mask));
        pixmap.draw_pixmap(
            left - monitor.left,
            top - monitor.top,
            shadow_layer.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

fn blur_alpha_mask(alpha: &mut [u8], width: usize, height: usize, radius: usize) {
    let mut horizontal = vec![0; alpha.len()];
    let mut prefix = vec![0u32; width.max(height) + 1];
    for y in 0..height {
        prefix[0] = 0;
        for x in 0..width {
            prefix[x + 1] = prefix[x] + alpha[y * width + x] as u32;
        }
        for x in 0..width {
            let start = x.saturating_sub(radius);
            let end = (x + radius + 1).min(width);
            horizontal[y * width + x] =
                ((prefix[end] - prefix[start]) / (end - start) as u32) as u8;
        }
    }
    for x in 0..width {
        prefix[0] = 0;
        for y in 0..height {
            prefix[y + 1] = prefix[y] + horizontal[y * width + x] as u32;
        }
        for y in 0..height {
            let start = y.saturating_sub(radius);
            let end = (y + radius + 1).min(height);
            alpha[y * width + x] = ((prefix[end] - prefix[start]) / (end - start) as u32) as u8;
        }
    }
}

pub(crate) fn render(
    session: &Session,
    monitor: Rect,
    scale: f32,
    in_progress: Option<&Object>,
    hovered: Option<usize>,
) -> Option<Pixmap> {
    let width = u32::try_from(monitor.right - monitor.left).ok()?;
    let height = u32::try_from(monitor.bottom - monitor.top).ok()?;
    if width == 0 || height == 0 || width as u64 * height as u64 > 40_000_000 {
        return None;
    }
    let mut pixmap = Pixmap::new(width, height)?;
    let spotlight = session
        .objects
        .iter()
        .chain(in_progress)
        .filter(|o| o.kind.is_spotlight())
        .collect::<Vec<_>>();
    if !spotlight.is_empty() {
        let mut mask = Pixmap::new(width, height)?;
        let mut shadow = Paint::default();
        shadow.set_color_rgba8(
            0,
            0,
            0,
            (session.spotlight_opacity.clamp(0., 1.) * 255.) as u8,
        );
        mask.fill_rect(
            SkRect::from_xywh(0., 0., width as f32, height as f32)?,
            &shadow,
            Transform::identity(),
            None,
        );
        let clear = Paint {
            blend_mode: BlendMode::Clear,
            anti_alias: true,
            ..Paint::default()
        };
        for object in &spotlight {
            if let Some(shape) = path(
                object.bounds,
                object.kind,
                object.style.rounding * scale,
                monitor,
            ) {
                mask.fill_path(
                    &shape,
                    &clear,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        pixmap.data_mut().copy_from_slice(mask.data());
    }
    for object in session.objects.iter().chain(in_progress) {
        if object.kind == Kind::Magnifier {
            draw_magnified_marks(&mut pixmap, session, in_progress, object, monitor);
            draw_magnifier_overlay(&mut pixmap, object, monitor, scale);
        } else {
            draw_object(&mut pixmap, object, monitor, scale);
        }
    }
    // Hover feedback belongs to the object boundary, while edit handles belong only
    // to the selected object. Keep the outline above the translucent fill.
    for (index, is_hover) in [(hovered, true), (session.selected, false)] {
        let Some(index) = index else { continue };
        if let Some(object) = session.objects.get(index).filter(|o| {
            if is_hover {
                matches!(
                    o.kind,
                    Kind::Rectangle | Kind::Ellipse | Kind::HighlightRectangle
                )
            } else {
                o.kind == Kind::HighlightRectangle
            }
        }) && let Some(shape) = path(
            object.bounds,
            object.kind,
            object.style.rounding * scale,
            monitor,
        ) {
            pixmap.stroke_path(
                &shape,
                &color_paint([40, 129, 255]),
                &Stroke {
                    width: (2. * scale).max(1.),
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    if let Some(index) = session
        .selected
        .and_then(|i| session.objects.get(i).map(|_| i))
    {
        let selected = &session.objects[index];
        for (handle, (x, y)) in selected.edit_handles(scale).into_iter().enumerate() {
            if selected.kind == Kind::Arrow && handle == 2 {
                let center = (x - monitor.left as f32, y - monitor.top as f32);
                // Stroke-only rings keep the arrow itself visible through the handle.
                for (radius, color, width) in [
                    (8., [255, 255, 255], 3.),
                    (8., [28, 28, 28], 1.2),
                    (5., [255, 255, 255], 2.4),
                    (5., [28, 28, 28], 1.),
                ] {
                    if let Some(circle) =
                        PathBuilder::from_circle(center.0, center.1, radius * scale)
                    {
                        pixmap.stroke_path(
                            &circle,
                            &color_paint(color),
                            &Stroke {
                                width: width * scale,
                                ..Stroke::default()
                            },
                            Transform::identity(),
                            None,
                        );
                    }
                }
                continue;
            }
            let half = 4. * scale;
            let Some(r) = SkRect::from_xywh(
                x - monitor.left as f32 - half,
                y - monitor.top as f32 - half,
                half * 2.,
                half * 2.,
            ) else {
                continue;
            };
            if handle >= 8 && selected.kind == Kind::HighlightRectangle {
                if let Some(circle) =
                    PathBuilder::from_circle(x - monitor.left as f32, y - monitor.top as f32, half)
                {
                    pixmap.fill_path(
                        &circle,
                        &color_paint([255, 255, 255]),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                    pixmap.stroke_path(
                        &circle,
                        &color_paint([40, 129, 255]),
                        &Stroke {
                            width: scale,
                            ..Stroke::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
                continue;
            }
            let mut white = Paint::default();
            white.set_color_rgba8(255, 255, 255, 255);
            pixmap.fill_rect(r, &white, Transform::identity(), None);
            let mut blue = color_paint(if selected.kind == Kind::HighlightLine {
                [218, 22, 192]
            } else {
                [40, 129, 255]
            });
            blue.anti_alias = false;
            pixmap.stroke_path(
                &PathBuilder::from_rect(r),
                &blue,
                &Stroke {
                    width: scale,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    Some(pixmap)
}

#[cfg(test)]
pub(crate) fn preview_image(pixmap: &Pixmap) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(pixmap.width(), pixmap.height());
    buffer.make_mut_bytes().copy_from_slice(pixmap.data());
    Image::from_rgba8_premultiplied(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_hover_follows_rectangle_and_ellipse_without_edit_handles() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        for kind in [Kind::Rectangle, Kind::Ellipse] {
            let mut session = Session::new();
            session.add(Object {
                kind,
                points: Vec::new(),
                bounds: Bounds::from_corners((20., 20.), (80., 70.)),
                style: lexift_core::domain::annotation::Style {
                    color: [217, 71, 43],
                    width: 12.,
                    rounding: 16.,
                    ..Default::default()
                },
                extra: Default::default(),
            });
            session.selected = None;
            let ordinary = render(&session, monitor, 1., None, None).unwrap();
            let hover = render(&session, monitor, 1., None, Some(0)).unwrap();
            let edge = if kind == Kind::Ellipse {
                (50, 20)
            } else {
                (20, 45)
            };
            assert!(
                ordinary.pixel(edge.0, edge.1).unwrap().red()
                    > ordinary.pixel(edge.0, edge.1).unwrap().blue()
            );
            assert!(
                hover.pixel(edge.0, edge.1).unwrap().blue()
                    > hover.pixel(edge.0, edge.1).unwrap().red()
            );
            assert_eq!(hover.pixel(10, 10).unwrap().alpha(), 0);
        }
    }

    #[test]
    fn highlighter_hover_adds_only_a_blue_outline() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let mut session = Session::new();
        session.add(Object {
            kind: Kind::HighlightRectangle,
            points: Vec::new(),
            bounds: Bounds::from_corners((20., 20.), (80., 70.)),
            style: Default::default(),
            extra: Default::default(),
        });
        session.selected = None;
        let ordinary = render(&session, monitor, 1., None, None).unwrap();
        let hover = render(&session, monitor, 1., None, Some(0)).unwrap();
        assert_eq!(ordinary.pixel(20, 40).unwrap().alpha(), 77);
        let blue = hover.pixel(20, 40).unwrap();
        assert!(blue.blue() > blue.red());
        // The resize square is only present after selection.
        assert_eq!(hover.pixel(16, 16).unwrap().alpha(), 0);
        session.selected = Some(0);
        let selected = render(&session, monitor, 1., None, None).unwrap();
        assert!(selected.pixel(16, 16).unwrap().alpha() > 0);
    }
    #[test]
    fn brush_dashes_round_caps_and_alpha() {
        let monitor = Rect {
            left: -50,
            top: -50,
            right: 250,
            bottom: 100,
        };
        let mut object = Object {
            kind: Kind::Pencil,
            points: vec![(0., 0.), (200., 0.)],
            bounds: Bounds::from_corners((0., 0.), (200., 0.)),
            style: lexift_core::domain::annotation::Style::default(),
            extra: Default::default(),
        };
        object.style.width = 10.;
        let mut counts = Vec::new();
        for dash in 0..5 {
            object.style.dash = dash;
            let mut s = Session::new();
            s.add(object.clone());
            s.selected = None;
            let p = render(&s, monitor, 1., None, None).unwrap();
            counts.push(
                p.data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] > 0)
                    .count(),
            );
            assert!(
                p.pixel(47, 50).unwrap().alpha() > 0,
                "round cap extends before endpoint"
            );
        }
        assert!(counts[1..].iter().all(|c| *c < counts[0]));
        object.kind = Kind::HighlightLine;
        let mut s = Session::new();
        s.add(object.clone());
        s.selected = None;
        let p = render(&s, monitor, 1., None, None).unwrap();
        assert_eq!(p.pixel(100, 50).unwrap().alpha(), 77);
        s.add(object);
        s.selected = None;
        let p = render(&s, monitor, 1., None, None).unwrap();
        assert!(p.pixel(100, 50).unwrap().alpha() > 77);
    }
    #[test]
    fn renders_negative_origin_and_multiple_spotlights() {
        let monitor = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let mut session = Session::new();
        for left in [-80., 20.] {
            session.add(Object {
                points: Vec::new(),
                bounds: Bounds::from_corners((left, -40.), (left + 30., 0.)),
                kind: Kind::SpotlightEllipse,
                style: Default::default(),
                extra: Default::default(),
            });
        }
        let pixels = render(&session, monitor, 1., None, None).unwrap();
        assert_eq!(pixels.width(), 200);
        let pixel = |x: usize, y: usize| pixels.data()[(y * 200 + x) * 4 + 3];
        assert_eq!(pixel(35, 80), 0, "first spotlight reveals the live desktop");
        assert_eq!(
            pixel(135, 80),
            0,
            "second spotlight reveals the live desktop"
        );
        assert!(pixel(100, 100) > 20, "outside both regions is dimmed");
    }

    #[test]
    fn arrow_and_filled_polyline_render_on_negative_origin_monitor() {
        let monitor = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let mut session = Session::new();
        session.add(Object {
            bounds: Bounds::from_corners((-80., -70.), (40., -70.)),
            kind: Kind::Arrow,
            points: vec![(-80., -70.), (40., -70.)],
            style: lexift_core::domain::annotation::Style {
                width: 5.,
                dash: 1,
                ..Default::default()
            },
            extra: Extra::Arrow {
                curved: false,
                head: true,
                start: Endpoint::None,
                end: Endpoint::FilledArrow,
            },
        });
        session.add(Object {
            bounds: Bounds::from_corners((-50., -20.), (50., 60.)),
            kind: Kind::Polyline,
            points: vec![(-50., -20.), (50., -20.), (0., 60.)],
            style: lexift_core::domain::annotation::Style {
                fill: true,
                ..Default::default()
            },
            extra: Extra::Polyline {
                curved: false,
                head: false,
                start: Endpoint::None,
                end: Endpoint::None,
            },
        });
        session.selected = None;
        let pixels = render(&session, monitor, 1., None, None).unwrap();
        assert!(
            pixels.pixel(95, 120).unwrap().alpha() > 0,
            "polygon interior is filled"
        );
        assert!(
            pixels.pixel(135, 30).unwrap().alpha() > 0,
            "arrow head reaches endpoint"
        );
        assert_eq!(pixels.pixel(5, 5).unwrap().alpha(), 0);
    }

    #[test]
    fn arrow_midpoint_ring_follows_visible_line_and_keeps_its_center_open() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 120,
            bottom: 100,
        };
        for (points, curved, center) in [
            (vec![(10., 30.), (90., 30.)], false, (50, 30)),
            (vec![(10., 30.), (90., 30.), (50., 70.)], true, (50, 50)),
        ] {
            let mut session = Session::new();
            session.add(Object {
                bounds: Bounds::from_corners(points[0], points[1]),
                kind: Kind::Arrow,
                points,
                style: lexift_core::domain::annotation::Style {
                    color: [220, 30, 10],
                    width: 5.,
                    ..Default::default()
                },
                extra: Extra::Arrow {
                    curved,
                    head: true,
                    start: Endpoint::None,
                    end: Endpoint::FilledArrow,
                },
            });
            let selected = render(&session, monitor, 1., None, None).unwrap();
            let center_pixel = selected.pixel(center.0, center.1).unwrap();
            assert!(center_pixel.red() > 150 && center_pixel.green() < 100);
            assert!(selected.pixel(center.0, center.1 - 8).unwrap().alpha() > 0);
            session.selected = None;
            let plain = render(&session, monitor, 1., None, None).unwrap();
            assert_eq!(plain.pixel(center.0, center.1 - 8).unwrap().alpha(), 0);
        }
    }

    #[test]
    fn magnifier_connector_anchors_respect_rectangle_and_ellipse() {
        let b = Bounds::from_corners((-40., -20.), (40., 20.));
        assert_eq!(frame_anchor(b, false, (100., 0.)), (40., 0.));
        assert_eq!(frame_anchor(b, true, (100., 0.)), (40., 0.));
        let diagonal = frame_anchor(b, true, (40., 20.));
        assert!((diagonal.0 - 28.284).abs() < 0.01);
        assert!((diagonal.1 - 14.142).abs() < 0.01);
        assert_eq!(frame_anchor(b, false, (0., 0.)), (0., 0.));
    }

    #[test]
    fn magnifier_connector_stays_outside_output_frame() {
        let source = Bounds::from_corners((0., 20.), (60., 80.));
        for ellipse in [false, true] {
            for connector in [
                MagnifierConnector::Plain,
                MagnifierConnector::Dot,
                MagnifierConnector::Frame,
                MagnifierConnector::None,
            ] {
                let separated = Bounds::from_corners((100., 10.), (200., 90.));
                assert_eq!(
                    magnifier_connector_segment(source, separated, ellipse, connector).is_some(),
                    connector != MagnifierConnector::None
                );
                let covered = Bounds::from_corners((20., 10.), (120., 90.));
                assert!(magnifier_connector_segment(source, covered, ellipse, connector).is_none());
                let touching = Bounds::from_corners((30., 10.), (130., 90.));
                assert!(
                    magnifier_connector_segment(source, touching, ellipse, connector).is_none()
                );
                let overlapping = Bounds::from_corners((40., 10.), (140., 90.));
                if matches!(
                    connector,
                    MagnifierConnector::Plain | MagnifierConnector::Dot
                ) {
                    let (_, end) =
                        magnifier_connector_segment(source, overlapping, ellipse, connector)
                            .unwrap();
                    assert!((end.0 - 40.).abs() < 0.01);
                } else {
                    assert!(
                        magnifier_connector_segment(source, overlapping, ellipse, connector)
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn magnifier_erase_removes_only_enlarged_annotations_and_none_hides_line() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 110,
            bottom: 70,
        };
        let source = Bounds::from_corners((10., 10.), (30., 30.));
        let output = Bounds::from_corners((50., 10.), (90., 50.));
        let mut session = Session::new();
        session.add(Object {
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((15., 15.), (25., 25.)),
            points: vec![],
            style: lexift_core::domain::annotation::Style {
                color: [0, 220, 40],
                fill: true,
                ..Default::default()
            },
            extra: Extra::None,
        });
        session.add(Object {
            kind: Kind::Magnifier,
            bounds: source,
            points: vec![],
            style: lexift_core::domain::annotation::Style {
                color: [220, 20, 0],
                width: 2.,
                ..Default::default()
            },
            extra: Extra::Magnifier {
                output,
                zoom: 2.,
                ellipse: false,
                connector: MagnifierConnector::None,
                erase_annotations: false,
                antialias: true,
                shadow: false,
            },
        });
        session.selected = None;
        let shown = render(&session, monitor, 1., None, None).unwrap();
        assert!(shown.pixel(70, 30).unwrap().green() > 180);
        assert_eq!(
            shown.pixel(40, 24).unwrap().alpha(),
            0,
            "none draws no connector"
        );
        if let Extra::Magnifier {
            erase_annotations, ..
        } = &mut session.objects[1].extra
        {
            *erase_annotations = true;
        }
        let erased = render(&session, monitor, 1., None, None).unwrap();
        assert_eq!(erased.pixel(70, 30).unwrap().alpha(), 0);
        assert!(
            erased.pixel(20, 20).unwrap().green() > 180,
            "source marks stay visible"
        );
    }

    #[test]
    fn magnifier_shadow_is_soft_and_outside_the_content_for_both_shapes() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 220,
            bottom: 150,
        };
        for ellipse in [false, true] {
            let mut session = Session::new();
            session.add(Object {
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((10., 20.), (50., 60.)),
                points: vec![],
                style: lexift_core::domain::annotation::Style {
                    color: [230, 40, 20],
                    width: 2.,
                    ..Default::default()
                },
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((80., 20.), (180., 100.)),
                    zoom: 2.,
                    ellipse,
                    connector: MagnifierConnector::None,
                    erase_annotations: true,
                    antialias: true,
                    shadow: true,
                },
            });
            session.selected = None;
            let with_shadow = render(&session, monitor, 1., None, None).unwrap();
            let near = with_shadow.pixel(130, 103).unwrap().alpha();
            let far = with_shadow.pixel(130, 115).unwrap().alpha();
            assert!(near > far, "the shadow fades away from the bottom edge");
            assert_eq!(with_shadow.pixel(130, 60).unwrap().alpha(), 0);

            if let Extra::Magnifier {
                shadow, antialias, ..
            } = &mut session.objects[0].extra
            {
                *shadow = false;
                *antialias = false;
            }
            let without_shadow = render(&session, monitor, 1., None, None).unwrap();
            assert_eq!(without_shadow.pixel(130, 103).unwrap().alpha(), 0);
            if let Extra::Magnifier { antialias, .. } = &mut session.objects[0].extra {
                *antialias = true;
            }
            let antialias_on = render(&session, monitor, 1., None, None).unwrap();
            assert_eq!(antialias_on.data(), without_shadow.data());
        }
    }
}
