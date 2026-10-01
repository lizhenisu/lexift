//! The badge and caption share their glyph layout.
use lexift_core::domain::{annotation::*, geometry::Rect};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
pub(crate) fn badge_text(object: &Object) -> Option<Object> {
    let Extra::Sequence(s) = &object.extra else {
        return None;
    };
    let mut text = s.caption.clone();
    text.content = sequence_label(s.value, s.format);
    text.font_size = s.size * 1.25;
    text.outline = None;
    text.background = None;
    text.rotation = 0.;
    text.linked_arrow = None;
    text.linked_arrow_control = None;
    text.bold = false;
    text.italic = false;
    let measured = crate::annotation_text::text_bounds((0., 0.), &text, s.scale);
    let center = object.bounds.center();
    let mut bounds = Bounds::from_corners(
        (
            center.0 - measured.width() / 2.,
            center.1 - measured.height() / 2.,
        ),
        (
            center.0 + measured.width() / 2.,
            center.1 + measured.height() / 2.,
        ),
    );
    if let Some(ink) = crate::annotation_text::text_ink_bounds(&text, s.scale) {
        let ink_center = ink.center();
        bounds = bounds.moved(
            center.0 - (bounds.left + ink_center.0),
            center.1 - (bounds.top + ink_center.1),
        );
    }
    let filled = matches!(
        s.shape,
        SequenceShape::Circle | SequenceShape::CircleWhiteBorder | SequenceShape::Square
    );
    let c = object.style.color;
    let bright = 0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2]);
    let color = if filled {
        if bright > 155. {
            [28, 28, 28]
        } else {
            [255, 255, 255]
        }
    } else {
        c
    };
    Some(Object {
        kind: Kind::Text,
        bounds,
        style: Style {
            color,
            ..object.style
        },
        points: Vec::new(),
        extra: Extra::Text(text),
    })
}
pub(crate) fn layout(object: &mut Object) {
    let center = object.bounds.center();
    let Some(number) = badge_text(object) else {
        return;
    };
    let Extra::Sequence(s) = &mut object.extra else {
        return;
    };
    let diameter = (s.size * 2.25 * s.scale).max(number.bounds.width() + s.size * 0.5 * s.scale);
    object.bounds = Bounds::from_corners(
        (center.0 - diameter / 2., center.1 - diameter / 2.),
        (center.0 + diameter / 2., center.1 + diameter / 2.),
    );
    s.caption.font_size = s.size * 1.25 * [0.6, 0.8, 1., 1.25, 1.5][s.caption_ratio.min(4)];
    let b = crate::annotation_text::text_bounds((0., 0.), &s.caption, s.scale);
    let at = (
        object.bounds.right + diameter * 0.15,
        center.1 - b.height() / 2.,
    );
    s.caption_bounds = Bounds::from_corners(at, (at.0 + b.width(), at.1 + b.height()));
}
pub(crate) fn draw(pixmap: &mut Pixmap, object: &Object, monitor: Rect) {
    let Extra::Sequence(s) = &object.extra else {
        return;
    };
    let b = object.bounds;
    let x = b.left - monitor.left as f32;
    let y = b.top - monitor.top as f32;
    let path = if matches!(
        s.shape,
        SequenceShape::Circle | SequenceShape::CircleWhiteBorder | SequenceShape::Outline
    ) {
        PathBuilder::from_circle(x + b.width() / 2., y + b.height() / 2., b.width() / 2.)
    } else {
        let mut p = PathBuilder::new();
        let r = (s.size * 0.15 * s.scale).min(b.width() / 2.);
        p.move_to(x + r, y);
        p.line_to(x + b.width() - r, y);
        p.quad_to(x + b.width(), y, x + b.width(), y + r);
        p.line_to(x + b.width(), y + b.height() - r);
        p.quad_to(
            x + b.width(),
            y + b.height(),
            x + b.width() - r,
            y + b.height(),
        );
        p.line_to(x + r, y + b.height());
        p.quad_to(x, y + b.height(), x, y + b.height() - r);
        p.line_to(x, y + r);
        p.quad_to(x, y, x + r, y);
        p.close();
        p.finish()
    };
    let mut paint = Paint::default();
    let c = object.style.color;
    paint.set_color_rgba8(c[0], c[1], c[2], 255);
    if let Some(path) = path {
        if !matches!(s.shape, SequenceShape::Plain | SequenceShape::Outline) {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if matches!(
            s.shape,
            SequenceShape::Outline | SequenceShape::CircleWhiteBorder
        ) {
            if s.shape == SequenceShape::CircleWhiteBorder {
                paint.set_color_rgba8(255, 255, 255, 255);
            }
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: (s.size * 0.07 * s.scale).max(s.scale),
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    if let Some(number) = badge_text(object) {
        crate::annotation_text::draw_text(pixmap, &number, monitor, s.scale);
    }
    if !s.caption.content.is_empty() {
        if let Some(mut caption) = object.sequence_caption() {
            if let Extra::Text(t) = &mut caption.extra {
                t.linked_arrow = None;
                t.linked_arrow_control = None;
            }
            crate::annotation_text::draw_text(pixmap, &caption, monitor, s.scale);
        }
        if s.guide {
            let mut p = PathBuilder::new();
            let bottom = b.bottom.max(s.caption_bounds.bottom);
            p.move_to(b.right - monitor.left as f32, b.bottom - monitor.top as f32);
            p.line_to(b.right - monitor.left as f32, bottom - monitor.top as f32);
            p.line_to(
                s.caption_bounds.right - monitor.left as f32,
                bottom - monitor.top as f32,
            );
            if let Some(path) = p.finish() {
                paint.set_color_rgba8(c[0], c[1], c[2], 255);
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: (s.size * 0.07 * s.scale).max(s.scale),
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn badge(scale: f32) -> Object {
        Object {
            kind: Kind::Sequence,
            bounds: Bounds::from_corners((-100., 80.), (-100., 80.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Sequence(SequenceAnnotation {
                value: 1,
                format: SequenceFormat::Decimal,
                shape: SequenceShape::Square,
                size: 16.,
                scale,
                caption: TextAnnotation {
                    content: "说明文字\n第二行 ABC".into(),
                    font_family: "Microsoft YaHei".into(),
                    font_size: 20.,
                    bold: false,
                    italic: false,
                    outline: None,
                    background: None,
                    rotation: 0.,
                    linked_arrow: None,
                    linked_arrow_control: None,
                    layout_scale: 1.,
                },
                caption_bounds: Bounds::from_corners((0., 0.), (0., 0.)),
                caption_color: [217, 71, 43],
                caption_custom_color: false,
                caption_ratio: 2,
                guide: true,
                endpoint: Endpoint::FilledArrow,
            }),
        }
    }

    #[test]
    fn visible_number_ink_is_centered_without_changing_size_or_dpi() {
        let labels = [
            (9, SequenceFormat::Decimal),
            (5, SequenceFormat::Decimal),
            (1, SequenceFormat::Chinese),
            (1, SequenceFormat::Roman),
            (6, SequenceFormat::LowerAlpha),
            (8, SequenceFormat::UpperAlpha),
            (10, SequenceFormat::UpperAlpha),
            (12, SequenceFormat::Chinese),
            (12, SequenceFormat::Decimal),
            (27, SequenceFormat::UpperAlpha),
            (49, SequenceFormat::Roman),
        ];
        for dpi in [1., 1.25] {
            for family in ["Microsoft YaHei", "Segoe UI"] {
                for size in [16., 64., 240.] {
                    for shape in [
                        SequenceShape::Circle,
                        SequenceShape::CircleWhiteBorder,
                        SequenceShape::Outline,
                        SequenceShape::Plain,
                        SequenceShape::Square,
                    ] {
                        for (value, format) in labels {
                            let mut object = badge(dpi);
                            if let Extra::Sequence(s) = &mut object.extra {
                                s.value = value;
                                s.format = format;
                                s.size = size;
                                s.shape = shape;
                                s.caption.font_family = family.into();
                            }
                            layout(&mut object);
                            let number = badge_text(&object).unwrap();
                            let Extra::Text(text) = &number.extra else {
                                unreachable!()
                            };
                            let full = crate::annotation_text::text_bounds((0., 0.), text, dpi);
                            assert!((number.bounds.width() - full.width()).abs() < 0.001);
                            assert!((number.bounds.height() - full.height()).abs() < 0.001);
                            assert!(
                                (crate::annotation_text::object_scale(&number) - dpi).abs() < 0.001
                            );
                            let ink = crate::annotation_text::text_ink_bounds(text, dpi)
                                .unwrap()
                                .moved(number.bounds.left, number.bounds.top);
                            assert!((ink.center().0 - object.bounds.center().0).abs() < 0.001);
                            assert!((ink.center().1 - object.bounds.center().1).abs() < 0.001);
                            let monitor = Rect {
                                left: object.bounds.left.floor() as i32 - 4,
                                top: object.bounds.top.floor() as i32 - 4,
                                right: object.bounds.right.ceil() as i32 + 4,
                                bottom: object.bounds.bottom.ceil() as i32 + 4,
                            };
                            let width = (monitor.right - monitor.left) as u32;
                            let mut pixmap =
                                Pixmap::new(width, (monitor.bottom - monitor.top) as u32).unwrap();
                            crate::annotation_text::draw_text(&mut pixmap, &number, monitor, dpi);
                            let (mut min_x, mut min_y, mut max_x, mut max_y) =
                                (u32::MAX, u32::MAX, 0, 0);
                            for (i, p) in pixmap.pixels().iter().enumerate() {
                                if p.alpha() > 0 {
                                    let (x, y) = (i as u32 % width, i as u32 / width);
                                    min_x = min_x.min(x);
                                    min_y = min_y.min(y);
                                    max_x = max_x.max(x);
                                    max_y = max_y.max(y);
                                }
                            }
                            assert_ne!(min_x, u32::MAX);
                            let center = object.bounds.center();
                            let actual = (
                                monitor.left as f32 + (min_x + max_x + 1) as f32 / 2.,
                                monitor.top as f32 + (min_y + max_y + 1) as f32 / 2.,
                            );
                            assert!(
                                (actual.0 - center.0).abs() <= 1.
                                    && (actual.1 - center.1).abs() <= 1.,
                                "{family} {value:?} {format:?} {size} {dpi}: {actual:?} != {center:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn growing_labels_keep_the_click_center_and_fit_the_badge() {
        for scale in [1., 1.25] {
            for format in [
                SequenceFormat::Decimal,
                SequenceFormat::Roman,
                SequenceFormat::LowerAlpha,
                SequenceFormat::UpperAlpha,
                SequenceFormat::Chinese,
            ] {
                let mut object = badge(scale);
                for value in [1, 27, 999] {
                    if let Extra::Sequence(s) = &mut object.extra {
                        s.value = value;
                        s.format = format;
                    }
                    layout(&mut object);
                    assert_eq!(object.bounds.center(), (-100., 80.));
                    let number = badge_text(&object).unwrap();
                    assert!(number.bounds.width() < object.bounds.width());
                    assert!(number.bounds.height() < object.bounds.height());
                    let caption = object.sequence_caption().unwrap();
                    assert!(caption.bounds.left > object.bounds.right);
                    assert!((caption.bounds.center().1 - 80.).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn each_shape_paints_on_a_negative_monitor_without_losing_caption() {
        let monitor = Rect {
            left: -200,
            top: 0,
            right: 500,
            bottom: 300,
        };
        for scale in [1., 1.25] {
            for shape in [
                SequenceShape::Circle,
                SequenceShape::CircleWhiteBorder,
                SequenceShape::Outline,
                SequenceShape::Plain,
                SequenceShape::Square,
            ] {
                let mut object = badge(scale);
                if let Extra::Sequence(s) = &mut object.extra {
                    s.shape = shape;
                }
                layout(&mut object);
                let mut pixmap = Pixmap::new(700, 300).unwrap();
                draw(&mut pixmap, &object, monitor);
                assert!(pixmap.pixels().iter().any(|p| p.alpha() != 0));
                let caption = object.sequence_caption().unwrap().bounds;
                let x = (caption.left - monitor.left as f32) as usize;
                assert!(
                    pixmap
                        .pixels()
                        .chunks(700)
                        .any(|row| row[x..].iter().any(|p| p.alpha() != 0))
                );
            }
        }
    }
}
