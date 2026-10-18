//! Bounded physical-pixel sampling; never samples an interaction or composited editor layer.
use crate::{domain::geometry::Point, ports::screen_capture::CapturedRegion};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);
impl Rgb {
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PixelSample {
    pub point: Point,
    pub color: Rgb,
    /// Row-major 15×9 neighborhood; missing or transparent pixels have no color.
    pub grid: [Option<Rgb>; 135],
}
pub fn sample_pixels(frames: &[CapturedRegion], point: Point) -> Option<PixelSample> {
    fn at(frames: &[CapturedRegion], x: i64, y: i64) -> Option<Rgb> {
        for f in frames {
            let b = f.bounds;
            let w = i64::from(b.right) - i64::from(b.left);
            let h = i64::from(b.bottom) - i64::from(b.top);
            if w <= 0 || h <= 0 || w.checked_mul(h)?.checked_mul(4)? as u64 != f.pixels.len() as u64
            {
                return None;
            }
            if x < i64::from(b.left)
                || x >= i64::from(b.right)
                || y < i64::from(b.top)
                || y >= i64::from(b.bottom)
            {
                continue;
            }
            let i = ((y - i64::from(b.top)) * w + x - i64::from(b.left)) as usize * 4;
            let p = f.pixels.get(i..i + 4)?;
            let a = u32::from(p[3]);
            if a == 0 {
                return None;
            }
            let channel = |v: u8| ((u32::from(v) * 255 + a / 2) / a).min(255) as u8;
            return Some(Rgb(channel(p[0]), channel(p[1]), channel(p[2])));
        }
        None
    }
    let color = at(frames, i64::from(point.x), i64::from(point.y))?;
    let grid = std::array::from_fn(|i| {
        at(
            frames,
            i64::from(point.x) + (i % 15) as i64 - 7,
            i64::from(point.y) + (i / 15) as i64 - 4,
        )
    });
    Some(PixelSample { point, color, grid })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geometry::Rect;
    #[test]
    fn physical_neighbors_cross_screens_and_do_not_fill_gaps_or_transparency() {
        let mut frames = vec![
            CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: -1,
                    right: 0,
                    bottom: 1,
                },
                pixels: [10, 20, 30, 255].repeat(4),
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: -1,
                    right: 2,
                    bottom: 1,
                },
                pixels: [40, 50, 60, 255].repeat(4),
            },
        ];
        let s = sample_pixels(&frames, Point { x: 0, y: 0 }).unwrap();
        assert_eq!(s.color.hex(), "#28323C");
        assert_eq!(s.grid[4 * 15 + 6], Some(Rgb(10, 20, 30)));
        assert_eq!(s.grid[4 * 15 + 8], Some(Rgb(40, 50, 60)));
        assert_eq!(s.grid[4 * 15 + 9], None);
        frames[1].pixels[..4].copy_from_slice(&[50, 25, 0, 128]);
        assert_eq!(
            sample_pixels(&frames, Point { x: 0, y: -1 }).unwrap().color,
            Rgb(100, 50, 0)
        );
        frames[1].pixels[3] = 0;
        assert!(sample_pixels(&frames, Point { x: 0, y: -1 }).is_none());
        assert!(
            sample_pixels(
                &frames,
                Point {
                    x: i32::MAX,
                    y: i32::MIN
                }
            )
            .is_none()
        );
        frames[0].pixels.pop();
        assert!(sample_pixels(&frames, Point { x: 0, y: 0 }).is_none());
    }
}
