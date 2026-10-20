//! Bounded downward registration. Pixels are immutable; an uncertain frame never advances the anchor.
use super::geometry::Rect;
use crate::{Error, Result, ports::screen_capture::CapturedRegion};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub mod workflow;

pub const MAX_PIXELS: usize = 32_000_000;
pub const MAX_HEIGHT: usize = 32768;
pub const MAX_BUFFER_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    Accepted,
    Unchanged,
    Uncertain,
    CaptureLost,
    Limit,
    Cancelled,
}

/// One bounded view of accepted pixels, in original image coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub image: Arc<CapturedRegion>,
    pub extent: Rect,
    pub visible: Rect,
    pub current: Rect,
}

/// Retains only the accepted image strips and one full physical capture as a matching anchor.
pub struct Stitcher {
    width: usize,
    height: usize,
    anchor: Arc<CapturedRegion>,
    strips: Vec<Arc<[u8]>>,
}

fn dimensions(frame: &CapturedRegion) -> Result<(usize, usize)> {
    let w = i64::from(frame.bounds.right) - i64::from(frame.bounds.left);
    let h = i64::from(frame.bounds.bottom) - i64::from(frame.bounds.top);
    if w <= 0
        || h <= 0
        || w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS as i64)
        || h > MAX_HEIGHT as i64
        || w * h * 4 != frame.pixels.len() as i64
    {
        return Err(Error::new("Invalid long capture frame"));
    }
    Ok((w as usize, h as usize))
}

impl Stitcher {
    pub fn new(first: Arc<CapturedRegion>) -> Result<Self> {
        let (width, height) = dimensions(&first)?;
        if width * height * 60 > MAX_BUFFER_BYTES {
            return Err(Error::new("Long capture buffer limit"));
        }
        Ok(Self {
            width,
            height,
            strips: vec![Arc::from(first.pixels.as_slice())],
            anchor: first,
        })
    }
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    pub fn retained_bytes(&self) -> usize {
        self.width * self.height * 4 + self.anchor.pixels.len()
    }
    pub fn push(&mut self, frame: Arc<CapturedRegion>, cancel: &AtomicBool) -> Result<Feedback> {
        let (w, h) = dimensions(&frame)?;
        if frame.bounds != self.anchor.bounds || w != self.width {
            return Err(Error::new("Long capture geometry changed"));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Feedback::Cancelled);
        }
        if frame.pixels == self.anchor.pixels {
            return Ok(Feedback::Unchanged);
        }
        let Some(dy) = displacement(&self.anchor, &frame, w, h, cancel) else {
            return Ok(if cancel.load(Ordering::Relaxed) {
                Feedback::Cancelled
            } else {
                Feedback::Uncertain
            });
        };
        let Some(height) = self.height.checked_add(dy) else {
            return Ok(Feedback::Limit);
        };
        // Reserve space for flattening, output composition, two in-flight captures and the anchor.
        let pixels = w.checked_mul(height);
        if height > MAX_HEIGHT
            || pixels.is_none_or(|p| {
                p > MAX_PIXELS || p * 48 + frame.pixels.len() * 3 > MAX_BUFFER_BYTES
            })
        {
            return Ok(Feedback::Limit);
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Feedback::Cancelled);
        }
        self.strips
            .push(Arc::from(&frame.pixels[(h - dy) * w * 4..]));
        self.height = height;
        self.anchor = frame;
        Ok(Feedback::Accepted)
    }
    /// Flatten only at the immutable editor/output handoff, never for capture feedback.
    pub fn flatten(&self, cancel: &AtomicBool) -> Result<CapturedRegion> {
        let mut pixels = Vec::with_capacity(self.width * self.height * 4);
        for strip in &self.strips {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::new("Long capture cancelled"));
            }
            pixels.extend_from_slice(strip);
        }
        Ok(CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            },
            pixels,
        })
    }
    /// Keep a stable preview width and sample only the accepted tail when its height is bounded.
    pub fn preview(&self, max_width: usize, max_height: usize) -> Preview {
        let w = max_width.max(1).min(self.width);
        let visible_height = (max_height.max(1).saturating_mul(self.width) / w)
            .max(1)
            .min(self.height);
        let start = self.height - visible_height;
        let h = (visible_height * w / self.width).max(1);
        let mut pixels = vec![0; w * h * 4];
        let mut strip_index = 0;
        let mut strip_start = 0;
        for y in 0..h {
            let sy = start + y * visible_height / h;
            while sy >= strip_start + self.strips[strip_index].len() / (self.width * 4) {
                strip_start += self.strips[strip_index].len() / (self.width * 4);
                strip_index += 1;
            }
            for x in 0..w {
                let src = ((sy - strip_start) * self.width + x * self.width / w) * 4;
                let dst = (y * w + x) * 4;
                pixels[dst..dst + 4].copy_from_slice(&self.strips[strip_index][src..src + 4]);
            }
        }
        Preview {
            image: Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                },
                pixels,
            }),
            extent: Rect {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            },
            visible: Rect {
                left: 0,
                top: start as i32,
                right: self.width as i32,
                bottom: self.height as i32,
            },
            current: Rect {
                left: 0,
                top: (self.height - (self.anchor.bounds.bottom - self.anchor.bounds.top) as usize)
                    as i32,
                right: self.width as i32,
                bottom: self.height as i32,
            },
        }
    }
}

fn gray(p: &[u8], i: usize) -> i32 {
    (i32::from(p[i]) * 77 + i32::from(p[i + 1]) * 150 + i32::from(p[i + 2]) * 29) >> 8
}

/// Coarse grayscale search followed by full-resolution validation. Ambiguous aliases are rejected.
fn displacement(
    a: &CapturedRegion,
    b: &CapturedRegion,
    w: usize,
    h: usize,
    cancel: &AtomicBool,
) -> Option<usize> {
    if w < 8 || h < 16 {
        return None;
    }
    let max = h * 3 / 4;
    let mut scores = Vec::with_capacity(max);
    for dy in 1..=max {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let overlap = h - dy;
        let mut error = 0u64;
        let mut count = 0u64;
        for y in (0..overlap).step_by((overlap / 32).max(1)) {
            for x in (0..w).step_by((w / 48).max(1)) {
                let ia = ((y + dy) * w + x) * 4;
                let ib = (y * w + x) * 4;
                error += (gray(&a.pixels, ia) - gray(&b.pixels, ib)).unsigned_abs() as u64;
                count += 1;
            }
        }
        scores.push((error * 1024 / count, dy));
    }
    scores.sort_unstable();
    let (score, _) = *scores.first()?;
    if score > 2 * 1024 {
        return None;
    }
    // Sparse text can produce several similar coarse scores. Validate those candidates
    // at original resolution rather than rejecting a correct shift on coarse aliases.
    let candidates: Vec<_> = scores
        .into_iter()
        .filter(|&(s, _)| s <= score + 1024)
        .collect();
    if candidates.len() > 16 {
        return None;
    }
    let mut accepted = None;
    for (_, dy) in candidates {
        if validate_overlap(a, b, w, h, dy, cancel) {
            if accepted.is_some_and(|old: usize| old.abs_diff(dy) > 2) {
                return None;
            }
            accepted.get_or_insert(dy);
        }
    }
    accepted
}

fn validate_overlap(
    a: &CapturedRegion,
    b: &CapturedRegion,
    w: usize,
    h: usize,
    dy: usize,
    cancel: &AtomicBool,
) -> bool {
    let overlap = h - dy;
    let mut errors = 0usize;
    let mut n = 0usize;
    let mut min = 255;
    let mut max = 0;
    for y in 0..overlap {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        for x in 0..w {
            let ia = ((y + dy) * w + x) * 4;
            let ib = (y * w + x) * 4;
            let value = gray(&a.pixels, ia);
            min = min.min(value);
            max = max.max(value);
            if (0..4).any(|c| a.pixels[ia + c].abs_diff(b.pixels[ib + c]) > 2) {
                errors += 1;
            }
            n += 1;
        }
    }
    max - min >= 12 && errors * 1000 <= n * 5
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(y0: usize) -> Arc<CapturedRegion> {
        image_width(y0, 96)
    }
    fn image_width(y0: usize, width: usize) -> Arc<CapturedRegion> {
        let mut pixels = Vec::new();
        for y in y0..y0 + 160 {
            for x in 0..width {
                let v = (y as u32)
                    .wrapping_mul(747796405)
                    .wrapping_add((x as u32).wrapping_mul(2891336453))
                    ^ ((y * x) as u32);
                pixels.extend_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]);
            }
        }
        Arc::new(CapturedRegion {
            bounds: Rect {
                left: -100,
                top: 50,
                right: width as i32 - 100,
                bottom: 210,
            },
            pixels,
        })
    }
    #[test]
    fn bounded_tail_preview_preserves_width_pixels_and_current_frame_version() {
        let cancel = AtomicBool::new(false);
        let mut stitch = Stitcher::new(image(0)).unwrap();
        let first = stitch.preview(48, 120);
        assert_eq!(first.image.bounds.right, 48);
        assert_eq!(first.visible.top, 0);
        for y in [80, 160, 240, 320, 400] {
            assert_eq!(stitch.push(image(y), &cancel).unwrap(), Feedback::Accepted);
        }
        let p = stitch.preview(48, 120);
        assert_eq!(p.image.bounds.right, 48);
        assert_eq!(p.image.bounds.bottom, 120);
        assert!(p.visible.top > 0);
        assert_eq!(p.current.bottom, p.extent.bottom);
        assert_eq!(p.current.top, 400);
        let full = stitch.flatten(&cancel).unwrap();
        for y in 0..120usize {
            for x in 0..48usize {
                let sy = p.visible.top as usize + y * 2;
                let src = (sy * 96 + x * 2) * 4;
                let dst = (y * 48 + x) * 4;
                assert_eq!(&p.image.pixels[dst..dst + 4], &full.pixels[src..src + 4]);
            }
        }
        assert_eq!(
            stitch.push(image(320), &cancel).unwrap(),
            Feedback::Uncertain
        );
        assert_eq!(stitch.preview(48, 120), p);
    }
    #[test]
    fn sparse_repeated_rows_use_full_resolution_to_reject_coarse_aliases() {
        let frame = |offset: usize| {
            let mut pixels = vec![255; 600 * 400 * 4];
            for y in 0..400 {
                for x in 0..600 {
                    let row = (y + offset) / 48;
                    let line = (y + offset) % 48;
                    // Repeated text strokes plus a small, unique row identifier.
                    if (8..18).contains(&line)
                        && ((20..480).contains(&x) && x % 11 < 3
                            || (500..560).contains(&x) && ((row >> (x / 8 % 6)) & 1) != 0)
                    {
                        let i = (y * 600 + x) * 4;
                        pixels[i..i + 3].fill(24);
                    }
                }
            }
            Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 600,
                    bottom: 400,
                },
                pixels,
            })
        };
        let mut stitch = Stitcher::new(frame(0)).unwrap();
        assert_eq!(
            stitch.push(frame(125), &AtomicBool::new(false)).unwrap(),
            Feedback::Accepted
        );
        assert_eq!(stitch.size(), (600, 525));
    }
    #[test]
    fn height_and_reserved_buffer_limits_preserve_the_last_anchor() {
        for width in [96, 1280] {
            let cancel = AtomicBool::new(false);
            let mut s = Stitcher::new(image_width(0, width)).unwrap();
            let mut accepted = 0;
            for y in (100..=40000).step_by(100) {
                let before = s.size();
                let result = s.push(image_width(y, width), &cancel).unwrap();
                if result == Feedback::Limit {
                    assert_eq!(s.size(), before);
                    assert!(before.1 <= MAX_HEIGHT);
                    assert!(s.retained_bytes() < MAX_BUFFER_BYTES);
                    assert_eq!(
                        s.push(image_width(accepted, width), &cancel).unwrap(),
                        Feedback::Unchanged
                    );
                    if width == 96 {
                        assert_eq!(before.1, 32760);
                    } else {
                        assert!(before.1 < MAX_HEIGHT);
                    }
                    break;
                }
                assert_eq!(result, Feedback::Accepted);
                accepted = y;
            }
            assert!(accepted > 0 && accepted < 40000);
        }
    }
    #[test]
    fn exact_forward_strips_recovery_and_thumbnail() {
        let cancel = AtomicBool::new(false);
        let mut s = Stitcher::new(image(0)).unwrap();
        assert_eq!(s.push(image(0), &cancel).unwrap(), Feedback::Unchanged);
        for y in [1, 11, 61, 140] {
            assert_eq!(s.push(image(y), &cancel).unwrap(), Feedback::Accepted);
        }
        let before = s.flatten(&cancel).unwrap().pixels;
        assert_eq!(s.push(image(100), &cancel).unwrap(), Feedback::Uncertain);
        assert_eq!(s.flatten(&cancel).unwrap().pixels, before);
        assert_eq!(s.push(image(140), &cancel).unwrap(), Feedback::Unchanged);
        assert_eq!(s.push(image(160), &cancel).unwrap(), Feedback::Accepted);
        let mut expected = image(0).pixels.clone();
        expected.extend_from_slice(&image(160).pixels);
        assert_eq!(s.flatten(&cancel).unwrap().pixels, expected);
        let thumb = s.preview(32, 60).image;
        assert!(thumb.bounds.right <= 32 && thumb.bounds.bottom <= 60);
    }
    #[test]
    fn unsafe_frames_never_change_result() {
        let cancel = AtomicBool::new(false);
        let mut s = Stitcher::new(image(0)).unwrap();
        assert_eq!(s.push(image(130), &cancel).unwrap(), Feedback::Uncertain);
        let mut changed = (*image(30)).clone();
        changed.pixels[..96 * 40 * 4].fill(230);
        assert_eq!(
            s.push(Arc::new(changed), &cancel).unwrap(),
            Feedback::Uncertain
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(s.push(image(30), &cancel).unwrap(), Feedback::Cancelled);
        assert_eq!(s.size(), (96, 160));
    }
    #[test]
    fn blank_periodic_and_invalid_frames_are_rejected() {
        assert!(
            Stitcher::new(Arc::new(CapturedRegion {
                bounds: Rect::default(),
                pixels: vec![]
            }))
            .is_err()
        );
        for periodic in [false, true] {
            let mut a = (*image(0)).clone();
            for y in 0..160 {
                for x in 0..96 {
                    let i = (y * 96 + x) * 4;
                    let v = if periodic { ((y % 8) * 25) as u8 } else { 255 };
                    a.pixels[i..i + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
            let mut b = a.clone();
            b.pixels.rotate_left(96 * 3 * 4);
            if !periodic {
                b.pixels[0] = 254;
            }
            let mut s = Stitcher::new(Arc::new(a)).unwrap();
            assert_eq!(
                s.push(Arc::new(b), &AtomicBool::new(false)).unwrap(),
                Feedback::Uncertain
            );
        }
    }
}
