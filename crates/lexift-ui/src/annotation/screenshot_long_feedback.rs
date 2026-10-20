//! Native layered feedback rasterization; no desktop reads or independent render tasks.
use lexift_core::domain::{geometry::Rect, long_capture::Preview};
use tiny_skia::{Paint, Pixmap, Rect as PixelRect, Transform};
fn fill(p: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: slint::Color) {
    if let Some(rect) = PixelRect::from_xywh(x, y, w, h) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(c.red(), c.green(), c.blue(), c.alpha());
        paint.anti_alias = false;
        p.fill_rect(rect, &paint, Transform::identity(), None);
    }
}
fn border(p: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, t: f32, c: slint::Color) {
    fill(p, x, y, w, t, c);
    fill(p, x, y + h - t, w, t, c);
    fill(p, x, y, t, h, c);
    fill(p, x + w - t, y, t, h, c);
}
pub(super) fn range(
    screen: Rect,
    source: Rect,
    dpi: f32,
    primary: slint::Color,
    mask: slint::Color,
) -> Option<Pixmap> {
    let width = (screen.right - screen.left) as u32;
    let height = (screen.bottom - screen.top) as u32;
    let mut p = Pixmap::new(width, height)?;
    let x = (source.left - screen.left) as f32;
    let y = (source.top - screen.top) as f32;
    let w = (source.right - source.left) as f32;
    let h = (source.bottom - source.top) as f32;
    fill(&mut p, 0., 0., width as f32, y, mask);
    fill(&mut p, 0., y + h, width as f32, height as f32 - y - h, mask);
    fill(&mut p, 0., y, x, h, mask);
    fill(&mut p, x + w, y, width as f32 - x - w, h, mask);
    border(&mut p, x, y, w, h, (2. * dpi).round().max(1.), primary);
    Some(p)
}
pub(super) fn preview(
    preview: &Preview,
    dpi: f32,
    primary: slint::Color,
    current: slint::Color,
) -> Option<Pixmap> {
    let t = (2. * dpi).round().max(1.) as usize;
    let w = preview.image.bounds.right as usize;
    let h = preview.image.bounds.bottom as usize;
    let stride = w + 2 * t;
    let mut p = Pixmap::new(stride as u32, (h + 2 * t) as u32)?;
    for y in 0..h {
        let dst = ((y + t) * stride + t) * 4;
        p.data_mut()[dst..dst + w * 4]
            .copy_from_slice(&preview.image.pixels[y * w * 4..(y + 1) * w * 4]);
    }
    border(
        &mut p,
        0.,
        0.,
        stride as f32,
        (h + 2 * t) as f32,
        t as f32,
        primary,
    );
    let span = (preview.visible.bottom - preview.visible.top).max(1) as f32;
    let top = (preview.current.top.max(preview.visible.top) - preview.visible.top) as f32 / span
        * h as f32;
    let height = (preview.current.bottom - preview.current.top.max(preview.visible.top)).max(0)
        as f32
        / span
        * h as f32;
    border(
        &mut p,
        t as f32,
        t as f32 + top,
        w as f32,
        height.max(t as f32),
        t as f32,
        current,
    );
    if preview.visible.top > 0 {
        let cx = stride as f32 / 2.;
        let size = 8. * dpi;
        for i in 0..(size as i32) {
            let y = t as f32 + size - i as f32;
            fill(&mut p, cx - i as f32, y, t as f32, t as f32, primary);
            fill(&mut p, cx + i as f32, y, t as f32, t as f32, primary);
        }
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::ports::screen_capture::CapturedRegion;
    use std::sync::Arc;
    #[test]
    fn feedback_hole_is_transparent_and_tail_marker_uses_the_same_visible_range() {
        let blue = slint::Color::from_rgb_u8(20, 80, 240);
        let green = slint::Color::from_rgb_u8(30, 200, 50);
        for dpi in [1., 1.25] {
            let screen = Rect {
                left: -400,
                top: 0,
                right: 400,
                bottom: 400,
            };
            let source = Rect {
                left: -300,
                top: 100,
                right: 0,
                bottom: 300,
            };
            let frame = range(
                screen,
                source,
                dpi,
                blue,
                slint::Color::from_argb_u8(96, 0, 0, 0),
            )
            .unwrap();
            let pixels = frame.data().as_chunks::<4>().0;
            assert_eq!(pixels[150 * 800 + 150], [0, 0, 0, 0]);
            assert_eq!(pixels[10 * 800 + 10][3], 96);
            assert_eq!(pixels[100 * 800 + 100], [20, 80, 240, 255]);
            let p = Preview {
                image: Arc::new(CapturedRegion {
                    bounds: Rect {
                        left: 0,
                        top: 0,
                        right: 100,
                        bottom: 200,
                    },
                    pixels: [100, 110, 120, 255].repeat(100 * 200),
                }),
                extent: Rect {
                    left: 0,
                    top: 0,
                    right: 400,
                    bottom: 2000,
                },
                visible: Rect {
                    left: 0,
                    top: 1200,
                    right: 400,
                    bottom: 2000,
                },
                current: Rect {
                    left: 0,
                    top: 1600,
                    right: 400,
                    bottom: 2000,
                },
            };
            let drawn = preview(&p, dpi, blue, green).unwrap();
            let t = (2. * dpi).round() as usize;
            let stride = 100 + 2 * t;
            let pixels = drawn.data().as_chunks::<4>().0;
            assert_eq!(pixels[(t + 50) * stride + t + 50], [100, 110, 120, 255]);
            assert_eq!(pixels[(t + 100) * stride + t + 50], [30, 200, 50, 255]);
            assert!(
                p.image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == [100, 110, 120, 255])
            );
        }
    }
}
