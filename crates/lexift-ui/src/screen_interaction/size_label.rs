//! Immutable screenshot feedback assets. Slint is consulted only during hidden preparation.
use crate::WindowLifecycleCallbacks;
use lexift_core::domain::geometry::Rect;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, collections::HashMap, sync::Arc};
use tiny_skia::{Pixmap, PixmapPaint, Transform};
const CHARACTERS: &str = "0123456789 ×px";
thread_local! { static ATLASES: RefCell<HashMap<u32, Arc<Atlas>>> = RefCell::new(HashMap::new()); }
pub(crate) struct Atlas {
    glyphs: Vec<Pixmap>,
    advances: Vec<f32>,
    scale: f32,
    inset: f32,
    height: f32,
    radius: f32,
    gap: f32,
    background: slint::Color,
}
#[derive(Clone)]
pub(crate) struct Feedback {
    pub crop: Rect,
    pub host: Rect,
    pub bounds: Rect,
    pub text: String,
    atlas: Arc<Atlas>,
}
pub(crate) fn clear() {
    ATLASES.with(|a| a.borrow_mut().clear());
}
pub(crate) fn prepare(
    scale: f32,
    lifecycle: &WindowLifecycleCallbacks,
) -> Result<(), slint::PlatformError> {
    if ATLASES.with(|a| a.borrow().contains_key(&scale.to_bits())) {
        return Ok(());
    }
    let window = super::create(lifecycle, crate::SizeLabelGlyphWindow::new)?;
    crate::theme::apply(&window);
    let mut image = None;
    // A hidden native window may realize its monitor DPI on its first snapshot.
    for _ in 0..2 {
        let native = window.window().scale_factor().max(0.1);
        let ratio = scale / native;
        window.set_render_scale(ratio);
        window
            .window()
            .set_size(slint::LogicalSize::new(336. * ratio, 28. * ratio));
        image = Some(window.window().take_snapshot()?);
    }
    let image = image.unwrap();
    let native = window.window().scale_factor();
    let advances = (0..14)
        .map(|i| window.get_advances().row_data(i).unwrap_or(0.) * native)
        .collect();
    let mut glyphs = Vec::with_capacity(14);
    for i in 0..14 {
        let start = (i as f32 * 24. * scale).round() as u32;
        let end = (((i + 1) as f32 * 24. * scale).round() as u32).min(image.width());
        let mut glyph = Pixmap::new(end.saturating_sub(start).max(1), image.height())
            .ok_or_else(|| slint::PlatformError::from("size label glyph allocation failed"))?;
        for y in 0..image.height() {
            for x in start..end {
                let p = image.as_slice()[(y * image.width() + x) as usize];
                let at = ((y * glyph.width() + x - start) * 4) as usize;
                let alpha = u32::from(p.a);
                glyph.data_mut()[at..at + 4].copy_from_slice(&[
                    ((u32::from(p.r) * alpha + 127) / 255) as u8,
                    ((u32::from(p.g) * alpha + 127) / 255) as u8,
                    ((u32::from(p.b) * alpha + 127) / 255) as u8,
                    p.a,
                ]);
            }
        }
        glyphs.push(glyph);
    }
    let style = window.global::<crate::SizeLabelStyle>();
    let atlas = Arc::new(Atlas {
        glyphs,
        advances,
        scale,
        inset: style.get_inset() * scale,
        height: style.get_label_height() * scale,
        radius: style.get_radius() * scale,
        gap: style.get_gap() * scale,
        background: window.global::<crate::Colors>().get_foreground(),
    });
    ATLASES.with(|a| a.borrow_mut().insert(scale.to_bits(), atlas));
    Ok(())
}

/// Freeze screen-only placement and glyphs into the same crop feedback snapshot.
pub(crate) fn layout(crop: Rect, screens: &[(Rect, f32)], area: Option<Rect>) -> Option<Feedback> {
    if crop.right <= crop.left || crop.bottom <= crop.top {
        return None;
    }
    let (host, scale) = *screens
        .iter()
        .find(|(b, _)| {
            crop.left >= b.left && crop.left < b.right && crop.top >= b.top && crop.top < b.bottom
        })
        .or_else(|| {
            screens.iter().find(|(b, _)| {
                crop.left < b.right
                    && crop.right > b.left
                    && crop.top < b.bottom
                    && crop.bottom > b.top
            })
        })?;
    let atlas = ATLASES.with(|a| a.borrow().get(&scale.to_bits()).cloned())?;
    let text = format!("{} × {} px", crop.right - crop.left, crop.bottom - crop.top);
    let width = (text
        .chars()
        .map(|c| {
            atlas.advances[CHARACTERS
                .find(c)
                .map(|byte| CHARACTERS[..byte].chars().count())
                .unwrap()]
        })
        .sum::<f32>()
        + 2. * atlas.inset)
        .ceil() as u32;
    let height = atlas.height.ceil() as u32;
    let work = area.unwrap_or(host);
    let work = Rect {
        left: work.left.max(host.left),
        top: work.top.max(host.top),
        right: work.right.min(host.right),
        bottom: work.bottom.min(host.bottom),
    };
    let point = label_position(crop, (width, height), work, atlas.gap.round() as i32);
    Some(Feedback {
        crop,
        host,
        bounds: Rect {
            left: point.x,
            top: point.y,
            right: point.x + width as i32,
            bottom: point.y + height as i32,
        },
        text,
        atlas,
    })
}
/// Fixed order: above, left, right, below; otherwise inset near the crop's top-left.
/// Only the alignment axis is clamped for external candidates, preserving separation.
fn label_position(
    crop: Rect,
    size: (u32, u32),
    area: Rect,
    gap: i32,
) -> lexift_core::domain::geometry::Point {
    use lexift_core::domain::geometry::Point;
    let (w, h) = (size.0 as i32, size.1 as i32);
    let clamp_x = |x: i32| x.clamp(area.left, (area.right - w).max(area.left));
    let clamp_y = |y: i32| y.clamp(area.top, (area.bottom - h).max(area.top));
    let x = clamp_x(crop.left);
    let y = clamp_y(crop.top);
    for (left, top) in [
        (x, crop.top - gap - h),
        (crop.left - gap - w, y),
        (crop.right + gap, y),
        (x, crop.bottom + gap),
    ] {
        if left >= area.left && top >= area.top && left + w <= area.right && top + h <= area.bottom
        {
            return Point { x: left, y: top };
        }
    }
    Point {
        x: clamp_x(crop.left + gap),
        y: clamp_y(crop.top + gap),
    }
}

impl Feedback {
    pub(crate) fn paint(&self, frame: &mut Pixmap, monitor: Rect) {
        if self.host != monitor {
            return;
        }
        let x = (self.bounds.left - monitor.left) as f32;
        let y = (self.bounds.top - monitor.top) as f32;
        let w = (self.bounds.right - self.bounds.left) as f32;
        let h = (self.bounds.bottom - self.bounds.top) as f32;
        let r = self.atlas.radius;
        let mut path = tiny_skia::PathBuilder::new();
        path.move_to(x + r, y);
        path.line_to(x + w - r, y);
        path.quad_to(x + w, y, x + w, y + r);
        path.line_to(x + w, y + h - r);
        path.quad_to(x + w, y + h, x + w - r, y + h);
        path.line_to(x + r, y + h);
        path.quad_to(x, y + h, x, y + h - r);
        path.line_to(x, y + r);
        path.quad_to(x, y, x + r, y);
        path.close();
        let mut paint = tiny_skia::Paint::default();
        let c = self.atlas.background;
        paint.set_color_rgba8(c.red(), c.green(), c.blue(), c.alpha());
        frame.fill_path(
            &path.finish().unwrap(),
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        let mut at = x + self.atlas.inset;
        for c in self.text.chars() {
            let index = CHARACTERS.chars().position(|ch| ch == c).unwrap();
            frame.draw_pixmap(
                at.round() as i32,
                y.round() as i32,
                self.atlas.glyphs[index].as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            at += self.atlas.advances[index];
        }
    }
    pub(crate) fn scale(&self) -> f32 {
        self.atlas.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(v: [i32; 4]) -> Rect {
        Rect {
            left: v[0],
            top: v[1],
            right: v[2],
            bottom: v[3],
        }
    }
    #[test]
    fn fixed_order_and_inside_fallback_at_both_dpis_and_origins() {
        // Expected coordinates include the same eight-logical-pixel gap as the top caption.
        let cases = [
            ([200, 100, 400, 300], (200, 64)),
            ([200, 2, 400, 300], (92, 2)),
            ([2, 2, 400, 300], (408, 2)),
            ([2, 2, 798, 300], (2, 308)),
            ([600, 2, 798, 598], (492, 2)),
            ([2, 2, 200, 598], (208, 2)),
            ([200, 300, 400, 598], (200, 264)),
            ([600, 300, 798, 598], (600, 264)),
            ([2, 2, 798, 598], (10, 10)),
            ([0, 0, 800, 600], (8, 8)),
            ([750, 100, 798, 300], (700, 64)),
            ([2, 580, 798, 598], (2, 544)),
        ];
        for scale in [1., 1.25] {
            for (ox, oy) in [(0, 0), (-1000, -400)] {
                let scaled = |v: [i32; 4]| {
                    rect([
                        (v[0] as f32 * scale) as i32 + ox,
                        (v[1] as f32 * scale) as i32 + oy,
                        (v[2] as f32 * scale) as i32 + ox,
                        (v[3] as f32 * scale) as i32 + oy,
                    ])
                };
                for (crop, expected) in cases {
                    let p = label_position(
                        scaled(crop),
                        ((100. * scale) as u32, (28. * scale) as u32),
                        scaled([0, 0, 800, 600]),
                        (8. * scale) as i32,
                    );
                    assert_eq!(
                        (p.x, p.y),
                        (
                            (expected.0 as f32 * scale) as i32 + ox,
                            (expected.1 as f32 * scale) as i32 + oy
                        ),
                        "crop={crop:?} scale={scale}"
                    );
                }
            }
        }
    }
}
