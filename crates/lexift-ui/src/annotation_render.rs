//! Rasterizes the transient annotation document into a transparent monitor surface.
use lexift_core::domain::{
    annotation::{Bounds, Kind, Object, Session},
    geometry::Rect,
};
#[cfg(test)]
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{
    BlendMode, FillRule, Paint, Path, PathBuilder, Pixmap, Rect as SkRect, Stroke, StrokeDash,
    Transform,
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
        draw_object(&mut pixmap, object, monitor, scale);
    }
    // Hover feedback belongs to the object boundary, while edit handles belong only
    // to the selected object. Keep the outline above the translucent fill.
    for index in [hovered, session.selected].into_iter().flatten() {
        if let Some(object) = session
            .objects
            .get(index)
            .filter(|o| o.kind == Kind::HighlightRectangle)
            && let Some(shape) = path(
                object.bounds,
                object.kind,
                object.style.rounding * scale,
                monitor,
            )
        {
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
            let half = 4. * scale;
            let Some(r) = SkRect::from_xywh(
                x - monitor.left as f32 - half,
                y - monitor.top as f32 - half,
                half * 2.,
                half * 2.,
            ) else {
                continue;
            };
            if handle >= 8 {
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
}
