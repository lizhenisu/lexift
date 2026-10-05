//! Clean lens content for effects above a native magnifier. Native annotation
//! windows are excluded from capture, so its source must be sampled explicitly.
use lexift_core::{
    domain::{
        annotation::{Extra, Object},
        geometry::Rect,
    },
    ports::screen_capture::ScreenCaptureFrame,
};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Transform,
};

/// The native lens is below the transparent annotation canvas. Clear its output
/// at the lens's document position so earlier privacy tiles cannot cover it.
pub(super) fn reveal_native<'a>(
    target: &mut Pixmap,
    object: &Object,
    mut earlier: impl Iterator<Item = &'a Object>,
    monitor: Rect,
) {
    use lexift_core::domain::annotation::{Bounds, Kind};
    let Extra::Magnifier {
        output, ellipse, ..
    } = object.extra
    else {
        return;
    };
    if !earlier.any(|o| {
        matches!(&o.extra, Extra::Mosaic(m) if m.strength > 0)
            && o.bounds.left < output.right
            && o.bounds.right > output.left
            && o.bounds.top < output.bottom
            && o.bounds.bottom > output.top
    }) {
        return;
    }
    let bounds = Bounds::from_corners(
        (output.left.floor(), output.top.floor()),
        (output.right.ceil(), output.bottom.ceil()),
    );
    if let Some(path) = super::path(
        bounds,
        if ellipse {
            Kind::Ellipse
        } else {
            Kind::Rectangle
        },
        0.,
        monitor,
    ) {
        target.fill_path(
            &path,
            &Paint {
                blend_mode: BlendMode::Clear,
                anti_alias: true,
                ..Paint::default()
            },
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

pub(crate) fn sources(objects: &[Object], roi: Rect) -> Vec<Rect> {
    objects
        .iter()
        .filter_map(|object| {
            let Extra::Magnifier {
                output,
                zoom,
                antialias,
                ..
            } = object.extra
            else {
                return None;
            };
            let output = Rect {
                left: output.left.floor() as i32,
                top: output.top.floor() as i32,
                right: output.right.ceil() as i32,
                bottom: output.bottom.ceil() as i32,
            };
            let visible = super::mosaic::intersect(roi, output)?;
            let source = Rect {
                left: object.bounds.left.floor() as i32,
                top: object.bounds.top.floor() as i32,
                right: object.bounds.right.ceil() as i32,
                bottom: object.bounds.bottom.ceil() as i32,
            };
            let zoom = zoom.clamp(1., 8.);
            let edge = i32::from(antialias);
            super::mosaic::intersect(
                source,
                Rect {
                    left: source.left + ((visible.left - output.left) as f32 / zoom).floor() as i32
                        - edge,
                    top: source.top + ((visible.top - output.top) as f32 / zoom).floor() as i32
                        - edge,
                    right: source.left
                        + ((visible.right - output.left) as f32 / zoom).ceil() as i32
                        + edge,
                    bottom: source.top
                        + ((visible.bottom - output.top) as f32 / zoom).ceil() as i32
                        + edge,
                },
            )
        })
        .collect()
}

/// Paint at the magnifier's document position, before its marks and border.
/// Missing source pixels postpone processing instead of publishing a false lens.
pub(super) fn draw(
    target: &mut Pixmap,
    object: &Object,
    monitor: Rect,
    capture: &ScreenCaptureFrame,
) -> Option<()> {
    let Extra::Magnifier {
        output,
        ellipse,
        antialias,
        zoom,
        ..
    } = object.extra
    else {
        return Some(());
    };
    let Some(source) = sources(std::slice::from_ref(object), monitor)
        .into_iter()
        .next()
    else {
        return Some(());
    };
    let region = capture.regions.iter().find(|r| {
        r.bounds.left <= source.left
            && r.bounds.top <= source.top
            && r.bounds.right >= source.right
            && r.bounds.bottom >= source.bottom
    })?;
    if (source.right as i64 - source.left as i64) * (source.bottom as i64 - source.top as i64)
        > 40_000_000
    {
        return None;
    }
    let mut sampled = Pixmap::new(
        (source.right - source.left) as u32,
        (source.bottom - source.top) as u32,
    )?;
    let width = sampled.width() as usize * 4;
    let stride = (region.bounds.right - region.bounds.left) as usize * 4;
    for (y, row) in sampled.data_mut().chunks_exact_mut(width).enumerate() {
        let start = (source.top - region.bounds.top) as usize * stride
            + y * stride
            + (source.left - region.bounds.left) as usize * 4;
        row.copy_from_slice(&region.pixels[start..start + width]);
    }
    let output_rect = Rect {
        left: output.left.floor() as i32,
        top: output.top.floor() as i32,
        right: output.right.ceil() as i32,
        bottom: output.bottom.ceil() as i32,
    };
    let visible = super::mosaic::intersect(output_rect, monitor)?;
    let width = (visible.right - visible.left) as u32;
    let height = (visible.bottom - visible.top) as u32;
    if width as u64 * height as u64 > 40_000_000 {
        return None;
    }
    let mut enlarged = Pixmap::new(width, height)?;
    enlarged.draw_pixmap(
        0,
        0,
        sampled.as_ref(),
        &PixmapPaint {
            quality: if antialias {
                FilterQuality::Bilinear
            } else {
                FilterQuality::Nearest
            },
            ..PixmapPaint::default()
        },
        Transform::from_row(
            zoom.clamp(1., 8.),
            0.,
            0.,
            zoom.clamp(1., 8.),
            (output_rect.left - visible.left) as f32
                + (source.left as f32 - object.bounds.left.floor()) * zoom.clamp(1., 8.),
            (output_rect.top - visible.top) as f32
                + (source.top as f32 - object.bounds.top.floor()) * zoom.clamp(1., 8.),
        ),
        None,
    );
    if ellipse {
        let mut mask = Mask::new(width, height)?;
        let path = PathBuilder::from_oval(tiny_skia::Rect::from_xywh(
            (output_rect.left - visible.left) as f32,
            (output_rect.top - visible.top) as f32,
            (output_rect.right - output_rect.left) as f32,
            (output_rect.bottom - output_rect.top) as f32,
        )?)?;
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        enlarged.apply_mask(&mask);
    }
    target.draw_pixmap(
        visible.left - monitor.left,
        visible.top - monitor.top,
        enlarged.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::annotation::{Bounds, Kind, MagnifierConnector, Style},
        ports::screen_capture::CapturedRegion,
    };

    #[test]
    fn lens_mapping_respects_zoom_shape_and_negative_source_coordinates() {
        let source = Rect {
            left: -20,
            top: -10,
            right: -12,
            bottom: -2,
        };
        let mut pixels = Vec::new();
        for _ in 0..8 {
            for x in 0..8 {
                pixels.extend_from_slice(if x < 4 {
                    &[210, 0, 0, 255]
                } else {
                    &[0, 0, 210, 255]
                });
            }
        }
        let capture = ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds: source,
                pixels,
            }],
        };
        for ellipse in [false, true] {
            let object = Object {
                id: Default::default(),
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((-20., -10.), (-12., -2.)),
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((0., 0.), (16., 16.)),
                    zoom: 2.,
                    ellipse,
                    antialias: false,
                    erase_annotations: false,
                    shadow: false,
                    connector: MagnifierConnector::None,
                },
            };
            let mut target = Pixmap::new(16, 16).unwrap();
            draw(
                &mut target,
                &object,
                Rect {
                    left: 0,
                    top: 0,
                    right: 16,
                    bottom: 16,
                },
                &capture,
            )
            .unwrap();
            assert_eq!(target.pixel(4, 8).unwrap().red(), 210);
            assert_eq!(target.pixel(12, 8).unwrap().blue(), 210);
            assert_eq!(
                target.pixel(0, 0).unwrap().alpha(),
                if ellipse { 0 } else { 255 }
            );
            let partial = Rect {
                left: 4,
                top: 4,
                right: 8,
                bottom: 12,
            };
            let requested = sources(std::slice::from_ref(&object), partial)[0];
            assert_eq!(
                (requested.right - requested.left) * (requested.bottom - requested.top),
                8,
                "only the inverse-mapped source intersection should be sampled"
            );
            let mut clipped = Pixmap::new(4, 8).unwrap();
            draw(&mut clipped, &object, partial, &capture).unwrap();
            for y in 0..8 {
                for x in 0..4 {
                    assert_eq!(
                        clipped.pixel(x, y).unwrap(),
                        target.pixel(x + 4, y + 4).unwrap()
                    );
                }
            }
        }
    }
}
