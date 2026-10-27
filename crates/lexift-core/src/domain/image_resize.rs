//! Non-destructive uniform scale and bounded resampling of premultiplied RGBA.
use super::{geometry::Rect, image_transform::Orientation};
use crate::{Error, Result, ports::screen_capture::CapturedRegion};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_PIXELS: u64 = 40_000_000;

/// Exact source-boundary mapping to the rounded, oriented output raster.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    pub source: Rect,
    pub visible: Rect,
    pub orientation: Orientation,
    pub size: [u32; 2],
    origin: [f64; 2],
    ratio: [f64; 2],
}
impl Projection {
    pub fn new(
        source: Rect,
        visible: Rect,
        orientation: Orientation,
        scale: ImageScale,
    ) -> Result<Self> {
        let b = orientation.rect(visible, source)?;
        let size = scale.bounds_size(visible, orientation)?;
        Ok(Self {
            source,
            visible,
            orientation,
            size,
            origin: [f64::from(b.left), f64::from(b.top)],
            ratio: [
                f64::from(size[0]) / (f64::from(b.right) - f64::from(b.left)),
                f64::from(size[1]) / (f64::from(b.bottom) - f64::from(b.top)),
            ],
        })
    }
    pub fn forward(self, p: [f64; 2]) -> [f64; 2] {
        let p = self.orientation.point(p, self.source);
        std::array::from_fn(|i| (p[i] - self.origin[i]) * self.ratio[i])
    }
    pub fn inverse(self, p: [f64; 2]) -> [f64; 2] {
        let p = std::array::from_fn(|i| p[i] / self.ratio[i] + self.origin[i]);
        self.orientation.inverse().point(
            p,
            self.orientation
                .rect(self.source, self.source)
                .unwrap_or(self.source),
        )
    }
}

/// Reduced positive ratio. Coordinates and retained source pixels never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageScale {
    numerator: u32,
    denominator: u32,
}
impl Default for ImageScale {
    fn default() -> Self {
        Self {
            numerator: 1,
            denominator: 1,
        }
    }
}
impl ImageScale {
    pub fn new(numerator: u32, denominator: u32) -> Result<Self> {
        if numerator == 0
            || denominator == 0
            || u64::from(numerator) > MAX_PIXELS
            || u64::from(denominator) > MAX_PIXELS
        {
            return Err(Error::new("Invalid image scale"));
        }
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Ok(Self {
            numerator: numerator / a,
            denominator: denominator / a,
        })
    }
    pub fn factor(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }
    /// Rounded proposal for linked draft fields, not authorization to allocate pixels.
    pub fn draft_dimensions(self, width: u32, height: u32) -> Result<[u32; 2]> {
        let side = |n: u32| -> Result<u32> {
            if n == 0 {
                return Err(Error::new("Invalid image dimensions"));
            }
            let n = u64::from(n)
                .checked_mul(u64::from(self.numerator))
                .and_then(|n| n.checked_add(u64::from(self.denominator) / 2))
                .ok_or_else(|| Error::new("Image size overflow"))?
                / u64::from(self.denominator);
            u32::try_from(n.max(1)).map_err(|_| Error::new("Image size overflow"))
        };
        Ok([side(width)?, side(height)?])
    }
    pub fn dimensions(self, width: u32, height: u32) -> Result<[u32; 2]> {
        let size = self.draft_dimensions(width, height)?;
        if u64::from(size[0]) * u64::from(size[1]) > MAX_PIXELS {
            return Err(Error::new("Image is too large"));
        }
        Ok(size)
    }
    pub fn bounds_size(self, bounds: Rect, orientation: Orientation) -> Result<[u32; 2]> {
        let w = u32::try_from(i64::from(bounds.right) - i64::from(bounds.left))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let h = u32::try_from(i64::from(bounds.bottom) - i64::from(bounds.top))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let [w, h] = self.dimensions(w, h)?;
        Ok(if orientation.swaps_axes() {
            [h, w]
        } else {
            [w, h]
        })
    }
    /// Reserve source composition plus old, pending and installed RGBA content/feedback pairs.
    /// The transition retains the old view until the new pixel buffers have been installed.
    pub fn validate(
        self,
        source: Rect,
        visible: Rect,
        orientation: Orientation,
    ) -> Result<[u32; 2]> {
        if visible.left < source.left
            || visible.top < source.top
            || visible.right > source.right
            || visible.bottom > source.bottom
        {
            return Err(Error::new("Image range is outside source"));
        }
        let size = self.bounds_size(visible, orientation)?;
        let sw = u64::try_from(i64::from(source.right) - i64::from(source.left))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let sh = u64::try_from(i64::from(source.bottom) - i64::from(source.top))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let reserve = sw
            .checked_mul(sh)
            .and_then(|n| n.checked_mul(12))
            .and_then(|n| {
                n.checked_add(if self == Self::default() {
                    0
                } else {
                    u64::from(size[0]) * u64::from(size[1]) * 24
                })
            })
            .ok_or_else(|| Error::new("Image size overflow"))?;
        if reserve > super::long_capture::MAX_BUFFER_BYTES as u64 {
            return Err(Error::new("Image allocation budget exceeded"));
        }
        Ok(size)
    }
    /// Downsampling integrates pixel coverage; upsampling uses pixel-center bilinear weights.
    /// Only the output and four channel accumulators are allocated, never a scaled full source.
    pub fn pixels(self, image: CapturedRegion, cancel: &AtomicBool) -> Result<CapturedRegion> {
        let w = u32::try_from(i64::from(image.bounds.right) - i64::from(image.bounds.left))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let h = u32::try_from(i64::from(image.bounds.bottom) - i64::from(image.bounds.top))
            .map_err(|_| Error::new("Invalid image dimensions"))?;
        let [ow, oh] = self.dimensions(w, h)?;
        let source_pixels = u64::from(w) * u64::from(h);
        if source_pixels > MAX_PIXELS || source_pixels * 4 != image.pixels.len() as u64 {
            return Err(Error::new("Invalid image buffer"));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("Image resize cancelled"));
        }
        if [w, h] == [ow, oh] {
            return Ok(image);
        }
        let len = usize::try_from(u64::from(ow) * u64::from(oh) * 4)
            .map_err(|_| Error::new("Image size overflow"))?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(len)
            .map_err(|_| Error::new("Image allocation failed"))?;
        pixels.resize(len, 0);
        let down = self.factor() < 1.;

        // Express coverage in destination-grid units, and linear weights in
        // half-pixel units. Integer accumulation avoids half-value rounding drift.
        let linear = |i: u32, from: u32, to: u32| {
            let denominator = 2 * u64::from(to);
            let n = ((2 * i64::from(i) + 1) * i64::from(from) - i64::from(to))
                .clamp(0, i64::from(from - 1) * denominator as i64) as u64;
            let a = n / denominator;
            let remainder = n % denominator;
            [
                (a as u32, denominator - remainder),
                ((a + 1).min(u64::from(from - 1)) as u32, remainder),
            ]
        };
        let total = if down {
            source_pixels
        } else {
            4 * u64::from(ow) * u64::from(oh)
        };
        for y in 0..oh {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::new("Image resize cancelled"));
            }
            for x in 0..ow {
                if x % 4096 == 0 && cancel.load(Ordering::Acquire) {
                    return Err(Error::new("Image resize cancelled"));
                }
                let mut sum = [0_u64; 4];
                let mut add = |xx: u32, yy: u32, weight: u64| {
                    let index = (u64::from(yy) * u64::from(w) + u64::from(xx)) as usize * 4;
                    for (channel, value) in sum.iter_mut().enumerate() {
                        *value += u64::from(image.pixels[index + channel]) * weight;
                    }
                };
                if down {
                    let xl = u64::from(x) * u64::from(w);
                    let xh = u64::from(x + 1) * u64::from(w);
                    let yl = u64::from(y) * u64::from(h);
                    let yh = u64::from(y + 1) * u64::from(h);
                    for yy in yl / u64::from(oh)..yh.div_ceil(u64::from(oh)) {
                        if yy % 64 == 0 && cancel.load(Ordering::Acquire) {
                            return Err(Error::new("Image resize cancelled"));
                        }
                        let wy = ((yy + 1) * u64::from(oh)).min(yh) - (yy * u64::from(oh)).max(yl);
                        for xx in xl / u64::from(ow)..xh.div_ceil(u64::from(ow)) {
                            if xx % 4096 == 0 && cancel.load(Ordering::Acquire) {
                                return Err(Error::new("Image resize cancelled"));
                            }
                            let wx =
                                ((xx + 1) * u64::from(ow)).min(xh) - (xx * u64::from(ow)).max(xl);
                            add(xx as u32, yy as u32, wx * wy);
                        }
                    }
                } else {
                    for (yy, wy) in linear(y, h, oh) {
                        for (xx, wx) in linear(x, w, ow) {
                            add(xx, yy, wx * wy);
                        }
                    }
                }
                let p = &mut pixels
                    [(u64::from(y) * u64::from(ow) * 4 + u64::from(x) * 4) as usize..][..4];
                for (c, value) in p.iter_mut().enumerate() {
                    *value = ((sum[c] + total / 2) / total).min(255) as u8;
                }
                let alpha = p[3];
                for value in &mut p[..3] {
                    *value = (*value).min(alpha);
                }
                if alpha == 0 {
                    p.fill(0);
                }
            }
        }
        let right = image
            .bounds
            .left
            .checked_add(ow as i32)
            .ok_or_else(|| Error::new("Image bounds overflow"))?;
        let bottom = image
            .bounds
            .top
            .checked_add(oh as i32)
            .ok_or_else(|| Error::new("Image bounds overflow"))?;
        Ok(CapturedRegion {
            bounds: Rect {
                right,
                bottom,
                ..image.bounds
            },
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratios_rounding_limits_and_orientation() {
        assert_eq!(
            ImageScale::new(2, 4).unwrap(),
            ImageScale::new(1, 2).unwrap()
        );
        assert!(ImageScale::new(0, 1).is_err());
        let proposal = ImageScale::new(8000, 321).unwrap();
        assert_eq!(proposal.draft_dimensions(321, 160).unwrap(), [8000, 3988]);
        assert_eq!(
            ImageScale::new(10000, 321)
                .unwrap()
                .draft_dimensions(321, 160)
                .unwrap(),
            [10000, 4984]
        );
        assert!(
            ImageScale::new(10000, 321)
                .unwrap()
                .dimensions(321, 160)
                .is_err()
        );
        assert_eq!(
            ImageScale::new(1, 2).unwrap().dimensions(3, 1).unwrap(),
            [2, 1]
        );
        assert!(
            ImageScale::new(2, 1)
                .unwrap()
                .dimensions(4000, 4000)
                .is_err()
        );
        let b = Rect {
            left: -40,
            top: 20,
            right: 60,
            bottom: 220,
        };
        assert_eq!(
            ImageScale::new(1, 2)
                .unwrap()
                .bounds_size(
                    b,
                    Orientation::identity().action(super::super::image_transform::Action::Right)
                )
                .unwrap(),
            [100, 50]
        );
    }
    #[test]
    fn coverage_bilinear_alpha_identity_and_cancellation() {
        let image = CapturedRegion {
            bounds: Rect {
                left: -2,
                top: 3,
                right: 0,
                bottom: 5,
            },
            pixels: vec![0, 0, 0, 0, 128, 0, 0, 128, 0, 128, 0, 128, 0, 0, 128, 128],
        };
        let cancel = AtomicBool::new(false);
        let half = ImageScale::new(1, 2)
            .unwrap()
            .pixels(image.clone(), &cancel)
            .unwrap();
        assert_eq!(half.pixels, [32, 32, 32, 96]);
        assert_eq!(
            ImageScale::default()
                .pixels(image.clone(), &cancel)
                .unwrap()
                .pixels,
            image.pixels
        );
        let up = ImageScale::new(2, 1)
            .unwrap()
            .pixels(image.clone(), &cancel)
            .unwrap();
        assert_eq!(&up.pixels[..4], &[0, 0, 0, 0]);
        for p in up.pixels.as_chunks::<4>().0 {
            assert!(p[..3].iter().all(|c| *c <= p[3]));
        }
        cancel.store(true, Ordering::Release);
        assert!(ImageScale::default().pixels(image, &cancel).is_err());
    }
    #[test]
    fn rounded_projection_is_invertible_for_every_direction_and_negative_origin() {
        let source = Rect {
            left: -301,
            top: 77,
            right: 100,
            bottom: 380,
        };
        let visible = Rect {
            left: -250,
            top: 100,
            right: 90,
            bottom: 379,
        };
        for orientation in Orientation::ALL {
            for scale in [
                ImageScale::new(1, 3).unwrap(),
                ImageScale::new(7, 3).unwrap(),
            ] {
                let p = Projection::new(source, visible, orientation, scale).unwrap();
                for point in [[-250., 100.], [90., 379.], [-137.25, 201.75]] {
                    let q = p.inverse(p.forward(point));
                    assert!((q[0] - point[0]).abs() < 1e-9 && (q[1] - point[1]).abs() < 1e-9);
                }
                let corners = [
                    [visible.left as f64, visible.top as f64],
                    [visible.right as f64, visible.bottom as f64],
                ]
                .map(|q| p.forward(q));
                for (i, size) in p.size.into_iter().enumerate() {
                    assert!(((corners[0][i] - corners[1][i]).abs() - (size as f64)).abs() < 1e-9);
                }
            }
        }
    }
    #[test]
    fn independent_noninteger_row_and_column_reference() {
        let cancel = AtomicBool::new(false);
        // Three opaque samples integrated into two equal 1.5-pixel intervals.
        for vertical in [false, true] {
            let input = CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: if vertical { 1 } else { 3 },
                    bottom: if vertical { 3 } else { 1 },
                },
                pixels: vec![0, 0, 0, 255, 90, 60, 30, 255, 240, 180, 120, 255],
            };
            let result = ImageScale::new(2, 3)
                .unwrap()
                .pixels(input, &cancel)
                .unwrap();
            assert_eq!(result.pixels, [30, 20, 10, 255, 190, 140, 90, 255]);
        }
        // Pixel-center linear interpolation: x = 0, .25, .75, 1.
        let input = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 1,
            },
            pixels: vec![0, 0, 0, 0, 120, 80, 40, 160],
        };
        let result = ImageScale::new(2, 1)
            .unwrap()
            .pixels(input, &cancel)
            .unwrap();
        assert_eq!(
            &result.pixels[..16],
            &[
                0, 0, 0, 0, 30, 20, 10, 40, 90, 60, 30, 120, 120, 80, 40, 160
            ]
        );
    }
    #[test]
    fn shared_history_restore_and_crop_keep_exact_ratio() {
        use super::super::{annotation::Session, image_transform::Action};
        let full = Rect {
            left: -30,
            top: 7,
            right: 170,
            bottom: 407,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        session.image_pixel_parameters = true;
        let scale = ImageScale::new(3, 2).unwrap();
        assert!(session.resize_image(scale).unwrap());
        assert!(session.set_image_crop(
            Rect {
                right: 70,
                bottom: 207,
                ..full
            },
            full
        ));
        assert_eq!(session.image_scale, scale);
        assert!(session.orient_image(Orientation::identity().action(Action::Right)));
        assert_eq!(
            scale
                .bounds_size(session.image_crop.unwrap(), session.image_orientation)
                .unwrap(),
            [300, 150]
        );
        assert!(session.undo());
        assert!(session.undo());
        assert!(session.undo());
        assert!(session.can_redo());
        assert!(!session.resize_image(Default::default()).unwrap());
        assert!(session.can_redo());
        assert!(session.redo());
        let restored = session.restored_image();
        assert_eq!(restored.image_scale, scale);
        assert!(restored.image_pixel_parameters);
        assert!(!restored.can_undo());
        assert!(session.resize_image(Default::default()).unwrap());
        assert!(session.undo());
        assert_eq!(session.image_scale, scale);
        assert!(session.image_pixel_parameters);
        assert!(
            session
                .resize_image(ImageScale::new(40000000, 1).unwrap())
                .is_err()
        );
        assert_eq!(session.image_scale, scale);
    }
    #[test]
    fn exact_half_coverage_rounds_consistently_across_prime_width_intervals() {
        let mut pixels = [1, 1, 1, 255].repeat(321);
        pixels.extend([0, 0, 0, 255].repeat(321));
        let image = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 321,
                bottom: 2,
            },
            pixels,
        };
        let resized = ImageScale::new(1, 2)
            .unwrap()
            .pixels(image, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(resized.pixels, [1, 1, 1, 255].repeat(161));
        let source = Rect {
            left: 0,
            top: 0,
            right: 2000,
            bottom: 4000,
        };
        assert!(
            ImageScale::new(2, 1)
                .unwrap()
                .dimensions(2000, 4000)
                .is_ok()
        );
        assert!(
            ImageScale::new(2, 1)
                .unwrap()
                .validate(source, source, Orientation::identity())
                .is_err()
        );
        assert!(
            ImageScale::default()
                .validate(source, source, Orientation::identity())
                .is_ok()
        );
    }
}
