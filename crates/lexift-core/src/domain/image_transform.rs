//! Exact orthogonal image orientation. Boundary coordinates and pixel indices are distinct.
use super::geometry::Rect;
use crate::{Error, Result, ports::screen_capture::CapturedRegion};
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Orientation(u8);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Left,
    Right,
    Horizontal,
    Vertical,
}
impl Orientation {
    pub const ALL: [Self; 8] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
        Self(7),
    ];
    pub const fn identity() -> Self {
        Self(0)
    }
    pub fn code(self) -> u8 {
        self.0
    }
    pub fn matrix(self) -> [i32; 4] {
        [
            [1, 0, 0, 1],
            [0, -1, 1, 0],
            [-1, 0, 0, -1],
            [0, 1, -1, 0],
            [-1, 0, 0, 1],
            [0, -1, -1, 0],
            [1, 0, 0, -1],
            [0, 1, 1, 0],
        ][self.0 as usize]
    }
    /// Apply this orientation after `earlier`; screen-axis actions therefore multiply on the left.
    pub fn after(self, earlier: Self) -> Self {
        let [a, b, c, d] = self.matrix();
        let [e, f, g, h] = earlier.matrix();
        let m = [a * e + b * g, a * f + b * h, c * e + d * g, c * f + d * h];
        Self::ALL.into_iter().find(|x| x.matrix() == m).unwrap()
    }
    pub fn action(self, action: Action) -> Self {
        let op = match action {
            Action::Left => Self(3),
            Action::Right => Self(1),
            Action::Horizontal => Self(4),
            Action::Vertical => Self(6),
        };
        op.after(self)
    }
    pub fn inverse(self) -> Self {
        Self::ALL
            .into_iter()
            .find(|x| x.after(self) == Self::identity())
            .unwrap()
    }
    pub fn swaps_axes(self) -> bool {
        self.matrix()[0] == 0
    }
    pub fn point(self, p: [f64; 2], full: Rect) -> [f64; 2] {
        let [a, b, c, d] = self.matrix();
        let w = f64::from(full.right) - f64::from(full.left);
        let h = f64::from(full.bottom) - f64::from(full.top);
        let x = p[0] - f64::from(full.left);
        let y = p[1] - f64::from(full.top);
        [
            f64::from(full.left)
                + f64::from(a) * x
                + f64::from(b) * y
                + if a < 0 { w } else { 0. }
                + if b < 0 { h } else { 0. },
            f64::from(full.top)
                + f64::from(c) * x
                + f64::from(d) * y
                + if c < 0 { w } else { 0. }
                + if d < 0 { h } else { 0. },
        ]
    }
    pub fn rect(self, r: Rect, full: Rect) -> Result<Rect> {
        let p = [
            [r.left, r.top],
            [r.right, r.top],
            [r.right, r.bottom],
            [r.left, r.bottom],
        ]
        .map(|p| self.point(p.map(f64::from), full));
        let low = |i| p.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min);
        let high = |i| p.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max);
        let v = [low(0), low(1), high(0), high(1)];
        if v.iter()
            .any(|x| !x.is_finite() || *x < f64::from(i32::MIN) || *x > f64::from(i32::MAX))
        {
            return Err(Error::new("Image orientation bounds overflow"));
        }
        Ok(Rect {
            left: v[0] as i32,
            top: v[1] as i32,
            right: v[2] as i32,
            bottom: v[3] as i32,
        })
    }
    /// Rearrange tightly packed premultiplied RGBA without interpolation or alpha conversion.
    /// `full` is the original editable extent, so cropped output keeps its oriented source origin.
    pub fn pixels(
        self,
        image: &CapturedRegion,
        full: Rect,
        cancel: &AtomicBool,
    ) -> Result<CapturedRegion> {
        let w = usize::try_from(i64::from(image.bounds.right) - i64::from(image.bounds.left))
            .map_err(|_| Error::new("Invalid image width"))?;
        let h = usize::try_from(i64::from(image.bounds.bottom) - i64::from(image.bounds.top))
            .map_err(|_| Error::new("Invalid image height"))?;
        let len = w
            .checked_mul(h)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| Error::new("Image size overflow"))?;
        if w == 0 || h == 0 || len != image.pixels.len() {
            return Err(Error::new("Invalid image buffer"));
        }
        let bounds = self.rect(image.bounds, full)?;
        let out_w = if self.swaps_axes() { h } else { w };
        let [a, b, c, d] = self.matrix();
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(len)
            .map_err(|_| Error::new("Image allocation failed"))?;
        pixels.resize(len, 0);
        for y in 0..h {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::new("Image orientation cancelled"));
            }
            for x in 0..w {
                let column = if a == 1 {
                    x
                } else if a == -1 {
                    w - 1 - x
                } else if b == 1 {
                    y
                } else {
                    h - 1 - y
                };
                let row = if c == 1 {
                    x
                } else if c == -1 {
                    w - 1 - x
                } else if d == 1 {
                    y
                } else {
                    h - 1 - y
                };
                let to = (row * out_w + column) * 4;
                let from = (y * w + x) * 4;
                pixels[to..to + 4].copy_from_slice(&image.pixels[from..from + 4]);
            }
        }
        Ok(CapturedRegion { bounds, pixels })
    }
}
/// Finish a source-coordinate composition exactly once, after clipping to its confirmed crop.
pub fn finish(
    image: CapturedRegion,
    full: Rect,
    orientation: Orientation,
    color: super::image_color::Color,
    cancel: &AtomicBool,
) -> Result<CapturedRegion> {
    let mut image = if orientation == Orientation::identity() {
        image
    } else {
        orientation.pixels(&image, full, cancel)?
    };
    color.apply(&mut image.pixels, cancel)?;
    Ok(image)
}

/// Shared confirmed output/OCR order; scaling never changes retained source resources.
pub fn finish_scaled(
    image: CapturedRegion,
    full: Rect,
    orientation: Orientation,
    color: super::image_color::Color,
    scale: super::image_resize::ImageScale,
    cancel: &AtomicBool,
) -> Result<CapturedRegion> {
    scale.pixels(finish(image, full, orientation, color, cancel)?, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_orientations_roundtrip_pixels_boundaries_and_composition() {
        let full = Rect {
            left: -7,
            top: -3,
            right: -4,
            bottom: -1,
        };
        let image = CapturedRegion {
            bounds: full,
            pixels: (0..6)
                .flat_map(|i| [i, 0, 0, if i == 0 { 0 } else { 128 }])
                .collect(),
        };
        for o in Orientation::ALL {
            let rotated = o.pixels(&image, full, &AtomicBool::new(false)).unwrap();
            let restored = o
                .inverse()
                .pixels(&rotated, rotated.bounds, &AtomicBool::new(false))
                .unwrap();
            assert_eq!(restored, image);
            for p in [[-7., -3.], [-4., -1.], [-5.5, -2.5]] {
                assert_eq!(o.inverse().point(o.point(p, full), rotated.bounds), p);
            }
            for next in Orientation::ALL {
                let composed = next
                    .pixels(&rotated, rotated.bounds, &AtomicBool::new(false))
                    .unwrap();
                assert_eq!(
                    composed,
                    next.after(o)
                        .pixels(&image, full, &AtomicBool::new(false))
                        .unwrap()
                );
            }
        }
        let right = Orientation::identity()
            .action(Action::Right)
            .pixels(&image, full, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            right
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            [3, 0, 4, 1, 5, 2]
        );
        assert_ne!(
            Orientation::identity()
                .action(Action::Right)
                .action(Action::Horizontal),
            Orientation::identity()
                .action(Action::Horizontal)
                .action(Action::Right)
        );
    }
    #[test]
    fn orientation_crop_and_annotation_share_history_and_restore_baseline() {
        use super::super::annotation::Session;
        let full = Rect {
            left: -30,
            top: 10,
            right: 170,
            bottom: 410,
        };
        let crop = Rect {
            left: -20,
            top: 20,
            right: 100,
            bottom: 200,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        assert!(session.set_image_crop(crop, full));
        let right = Orientation::identity().action(Action::Right);
        assert!(session.orient_image(right));
        assert!(!session.orient_image(right));
        assert!(session.undo());
        assert_eq!(session.image_orientation, Orientation::identity());
        assert_eq!(session.image_crop, Some(crop));
        assert!(session.undo());
        assert_eq!(session.image_crop, None);
        assert!(session.redo());
        assert!(session.redo());
        assert_eq!(session.image_orientation, right);
        let restored = Session::restored_image(&session);
        assert_eq!(restored.image_orientation, right);
        assert_eq!(restored.image_crop, Some(crop));
        assert!(!restored.can_undo());
        assert!(!restored.can_redo());
    }
    #[test]
    fn cropped_pixel_range_invalid_buffers_and_cancellation() {
        let full = Rect {
            left: -20,
            top: 3,
            right: 0,
            bottom: 13,
        };
        let crop = Rect {
            left: -18,
            top: 4,
            right: -17,
            bottom: 5,
        };
        let image = CapturedRegion {
            bounds: crop,
            pixels: vec![20, 40, 60, 128],
        };
        for o in Orientation::ALL {
            let r = o.pixels(&image, full, &AtomicBool::new(false)).unwrap();
            assert_eq!(r.bounds, o.rect(crop, full).unwrap());
            assert_eq!(r.pixels, image.pixels);
        }
        assert!(
            Orientation::identity()
                .pixels(&image, full, &AtomicBool::new(true))
                .is_err()
        );
        assert!(
            Orientation::identity()
                .pixels(
                    &CapturedRegion {
                        bounds: crop,
                        pixels: vec![]
                    },
                    full,
                    &AtomicBool::new(false)
                )
                .is_err()
        );
    }
}
