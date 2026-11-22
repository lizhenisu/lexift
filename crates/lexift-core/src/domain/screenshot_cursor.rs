//! Immutable cursor pixels associated with a single frozen desktop.
use super::geometry::Rect;
use crate::ports::screen_capture::CapturedRegion;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pixels {
    /// Premultiplied RGBA.
    Alpha(Vec<u8>),
    /// One AND byte per pixel and three packed RGB XOR bytes per pixel.
    Raster { and: Vec<u8>, xor: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub bounds: Rect,
    pub hotspot: super::geometry::Point,
    pub pixels: Pixels,
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let result = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (result.left < result.right && result.top < result.bottom).then_some(result)
}

impl Cursor {
    pub fn intersects(&self, bounds: Rect) -> bool {
        let Some(overlap) = self.clip_to(bounds) else {
            return false;
        };
        let width = (self.bounds.right - self.bounds.left) as usize;
        (overlap.top..overlap.bottom).any(|y| {
            (overlap.left..overlap.right).any(|x| {
                let i = (y - self.bounds.top) as usize * width + (x - self.bounds.left) as usize;
                match &self.pixels {
                    Pixels::Alpha(pixels) => pixels.get(i * 4 + 3).is_some_and(|&alpha| alpha != 0),
                    Pixels::Raster { and, xor } => {
                        and.get(i).is_some_and(|&mask| mask != 255)
                            || xor.get(i * 3..i * 3 + 3).is_some_and(|p| p != [0, 0, 0])
                    }
                }
            })
        })
    }

    pub fn clip_to(&self, bounds: Rect) -> Option<Rect> {
        intersection(self.bounds, bounds)
    }

    /// Composite only the intersecting rectangle, in physical desktop coordinates.
    pub fn paint(&self, bounds: Rect, rgba: &mut [u8]) {
        let Some(overlap) = intersection(self.bounds, bounds) else {
            return;
        };
        let source_width = (self.bounds.right - self.bounds.left) as usize;
        let target_width = (bounds.right - bounds.left) as usize;
        let target_height = (bounds.bottom - bounds.top) as usize;
        let source_len = source_width * (self.bounds.bottom - self.bounds.top) as usize;
        let valid = match &self.pixels {
            Pixels::Alpha(colors) => colors.len() == source_len * 4,
            Pixels::Raster { and, xor } => and.len() == source_len && xor.len() == source_len * 3,
        };
        if !valid || rgba.len() != target_width * target_height * 4 {
            return;
        }
        for y in overlap.top..overlap.bottom {
            for x in overlap.left..overlap.right {
                let src =
                    (y - self.bounds.top) as usize * source_width + (x - self.bounds.left) as usize;
                let dst =
                    ((y - bounds.top) as usize * target_width + (x - bounds.left) as usize) * 4;
                let pixel = &mut rgba[dst..dst + 4];
                match &self.pixels {
                    Pixels::Alpha(colors) => {
                        let source = &colors[src * 4..src * 4 + 4];
                        let inv = 255 - u16::from(source[3]);
                        for (dst, &src) in pixel.iter_mut().zip(source) {
                            *dst = src.saturating_add(((u16::from(*dst) * inv + 127) / 255) as u8);
                        }
                    }
                    // A transparent destination is a hole in the captured desktop.
                    Pixels::Raster { and, xor } if pixel[3] == 255 => {
                        for c in 0..3 {
                            pixel[c] = (pixel[c] & and[src]) ^ xor[src * 3 + c];
                        }
                    }
                    Pixels::Raster { .. } => {}
                }
            }
        }
    }

    /// Produce a bounded preview patch from the clean desktop for XOR cursors too.
    pub fn patches(&self, frames: &[CapturedRegion], mask: Option<&[Rect]>) -> Vec<CapturedRegion> {
        frames
            .iter()
            .filter_map(|frame| {
                let bounds = intersection(self.bounds, frame.bounds)?;
                let mut image =
                    super::screen_snapshot::crop_frames(std::slice::from_ref(frame), bounds)
                        .ok()?;
                self.paint(bounds, &mut image.pixels);
                if let Some(mask) = mask {
                    super::screenshot_windows::clip_pixels(
                        &mut image.pixels,
                        bounds,
                        mask,
                        &std::sync::atomic::AtomicBool::new(false),
                    )
                    .ok()?;
                }
                Some(image)
            })
            .collect()
    }

    /// Apply cursor patches without writing into projection gaps or a mask's transparent pixels.
    pub fn paint_desktop(
        &self,
        image: &mut CapturedRegion,
        frames: &[CapturedRegion],
        mask: Option<&[Rect]>,
    ) {
        for patch in self.patches(frames, mask) {
            let Some(bounds) = intersection(image.bounds, patch.bounds) else {
                continue;
            };
            let dst_width = (image.bounds.right - image.bounds.left) as usize;
            let src_width = (patch.bounds.right - patch.bounds.left) as usize;
            for y in bounds.top..bounds.bottom {
                for x in bounds.left..bounds.right {
                    let src = ((y - patch.bounds.top) as usize * src_width
                        + (x - patch.bounds.left) as usize)
                        * 4;
                    let dst = ((y - image.bounds.top) as usize * dst_width
                        + (x - image.bounds.left) as usize)
                        * 4;
                    if patch.pixels[src + 3] != 0 && image.pixels[dst + 3] != 0 {
                        image.pixels[dst..dst + 4].copy_from_slice(&patch.pixels[src..src + 4]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geometry::Point;

    fn rect(left: i32, right: i32) -> Rect {
        Rect {
            left,
            top: -1,
            right,
            bottom: 0,
        }
    }

    #[test]
    fn cursor_patches_respect_negative_coordinates_holes_and_display_coverage() {
        let cursor = Cursor {
            bounds: rect(-3, 2),
            hotspot: Point { x: -2, y: -1 },
            pixels: Pixels::Alpha(vec![
                0, 0, 0, 0, 64, 0, 16, 128, 255, 0, 0, 255, 0, 0, 255, 255, 0, 255, 0, 255,
            ]),
        };
        let frames = [
            CapturedRegion {
                bounds: rect(-3, -1),
                pixels: [80, 40, 20, 255].repeat(2),
            },
            CapturedRegion {
                bounds: rect(0, 2),
                pixels: [80, 40, 20, 255].repeat(2),
            },
        ];
        assert!(!cursor.intersects(rect(-3, -2)));
        assert!(cursor.intersects(rect(1, 2)));
        let mask = [rect(-3, -1), rect(1, 2)];
        let mut image = crate::domain::screenshot_windows::compose(&frames, &mask).unwrap();
        cursor.paint_desktop(&mut image, &frames, Some(&mask));
        assert_eq!(
            &image.pixels,
            &[
                80, 40, 20, 255, 104, 20, 26, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 255,
            ]
        );
        assert_eq!(cursor.patches(&frames, Some(&mask)).len(), 2);
    }

    #[test]
    fn legacy_inversion_uses_the_original_desktop_pixel() {
        let cursor = Cursor {
            bounds: rect(3, 5),
            hotspot: Point { x: 4, y: -1 },
            pixels: Pixels::Raster {
                and: vec![255, 0],
                xor: vec![255, 255, 255, 40, 60, 80],
            },
        };
        let mut pixels = [18, 40, 80, 255, 88, 99, 100, 255];
        cursor.paint(rect(3, 5), &mut pixels);
        assert_eq!(pixels, [237, 215, 175, 255, 40, 60, 80, 255]);
    }
}
