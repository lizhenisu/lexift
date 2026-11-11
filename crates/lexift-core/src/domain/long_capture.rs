//! Bounded downward registration. Pixels are immutable; an uncertain frame never advances the anchor.
use super::geometry::Rect;
use crate::{Error, Result, ports::screen_capture::CapturedRegion};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub mod auto_scroll;
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
    bands: Option<Bands>,
}

/// Fixed rows in physical capture coordinates. Zero rows means ordinary stitching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bands {
    top: usize,
    bottom: usize,
}

impl Bands {
    fn body(self, pixels: &[u8], stride: usize) -> &[u8] {
        &pixels[self.top * stride..pixels.len() - self.bottom * stride]
    }

    fn matches(self, a: &[u8], b: &[u8], stride: usize) -> bool {
        let top = self.top * stride;
        let bottom = a.len() - self.bottom * stride;
        a[..top] == b[..top] && a[bottom..] == b[bottom..]
    }

    fn displacement(self, a: &[u8], b: &[u8], width: usize, cancel: &AtomicBool) -> Option<usize> {
        let stride = width * 4;
        if !self.matches(a, b, stride) {
            return None;
        }
        displacement(
            self.body(a, stride),
            self.body(b, stride),
            width,
            self != Self::default(),
            cancel,
        )
    }
}

fn textured(pixels: &[u8]) -> bool {
    let mut min = 255;
    let mut max = 0;
    for i in (0..pixels.len()).step_by(4) {
        let value = gray(pixels, i);
        min = min.min(value);
        max = max.max(value);
        if max - min >= 12 {
            return true;
        }
    }
    false
}

/// Require an observable boundary rather than trimming an arbitrary portion of a flat edge.
fn edge_rows(
    a: &[u8],
    b: &[u8],
    stride: usize,
    bottom: bool,
    cancel: &AtomicBool,
) -> Option<usize> {
    let height = a.len() / stride;
    for distance in 0..=height / 4 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let y = if bottom {
            height - 1 - distance
        } else {
            distance
        };
        let row = y * stride..(y + 1) * stride;
        if a[row.clone()] != b[row] {
            let band = if bottom {
                &a[a.len() - distance * stride..]
            } else {
                &a[..distance * stride]
            };
            return Some(if distance >= 4 && textured(band) {
                distance
            } else {
                0
            });
        }
    }
    // The common edge exceeds the supported height. Never try an arbitrary smaller crop.
    Some(0)
}

fn initial_match(a: &[u8], b: &[u8], width: usize, cancel: &AtomicBool) -> Option<(Bands, usize)> {
    let stride = width * 4;
    let top = edge_rows(a, b, stride, false, cancel)?;
    let bottom = edge_rows(a, b, stride, true, cancel)?;
    let mut found = None;
    for bands in [
        Bands::default(),
        Bands { top, bottom: 0 },
        Bands { top: 0, bottom },
        Bands { top, bottom },
    ] {
        if bands.top + bands.bottom > a.len() / stride / 3
            || found.is_some_and(|(previous, _)| previous == bands)
        {
            continue;
        }
        if let Some(dy) = bands.displacement(a, b, width, cancel) {
            if found.is_some() || ambiguous_boundary(bands, a, b, stride, dy) {
                return None;
            }
            found = Some((bands, dy));
        }
        if top == 0 && bottom == 0 {
            break;
        }
    }
    found
}

/// A smaller eligible band would explain the same shift if its adjacent overlap row also matched.
/// Every smaller boundary must include this row, so one comparison rules out the whole range.
fn ambiguous_boundary(bands: Bands, a: &[u8], b: &[u8], stride: usize, dy: usize) -> bool {
    let end = a.len() - bands.bottom * stride;
    let top = bands.top * stride;
    let shift = dy * stride;
    (bands.top > 4
        && textured(&a[..top - stride])
        && a[top + shift - stride..top + shift] == b[top - stride..top])
        || (bands.bottom > 4
            && textured(&a[end + stride..])
            && a[end..end + stride] == b[end - shift..end - shift + stride])
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
            bands: None,
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
        let matched = if let Some(bands) = self.bands {
            bands
                .displacement(&self.anchor.pixels, &frame.pixels, w, cancel)
                .map(|dy| (bands, dy))
        } else {
            initial_match(&self.anchor.pixels, &frame.pixels, w, cancel)
        };
        let Some((bands, dy)) = matched else {
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
        let end = (h - bands.bottom) * w * 4;
        self.strips
            .push(Arc::from(&frame.pixels[end - dy * w * 4..end]));
        self.bands = Some(bands);
        self.height = height;
        self.anchor = frame;
        Ok(Feedback::Accepted)
    }
    fn output_strips(&self) -> impl Iterator<Item = &[u8]> {
        let footer = self.bands.unwrap_or_default().bottom * self.width * 4;
        self.strips
            .iter()
            .enumerate()
            .map(move |(index, strip)| {
                if index == 0 {
                    &strip[..strip.len() - footer]
                } else {
                    strip.as_ref()
                }
            })
            .chain(std::iter::once(
                &self.anchor.pixels[self.anchor.pixels.len() - footer..],
            ))
    }

    /// Flatten only at the immutable editor/output handoff, never for capture feedback.
    pub fn flatten(&self, cancel: &AtomicBool) -> Result<CapturedRegion> {
        let mut pixels = Vec::with_capacity(self.width * self.height * 4);
        for strip in self.output_strips() {
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
        let stride = self.width * 4;
        let mut strips = self.output_strips();
        let mut strip = strips.next().expect("first capture strip");
        let mut strip_start = 0;
        for y in 0..h {
            let sy = start + y * visible_height / h;
            while sy >= strip_start + strip.len() / stride {
                strip_start += strip.len() / stride;
                strip = strips.next().expect("output strips cover preview");
            }
            for x in 0..w {
                let src = ((sy - strip_start) * self.width + x * self.width / w) * 4;
                let dst = (y * w + x) * 4;
                pixels[dst..dst + 4].copy_from_slice(&strip[src..src + 4]);
            }
        }
        let bands = self.bands.unwrap_or_default();
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
                top: (self.height - self.anchor.pixels.len() / stride + bands.top) as i32,
                right: self.width as i32,
                bottom: (self.height - bands.bottom) as i32,
            },
        }
    }
}

fn gray(p: &[u8], i: usize) -> i32 {
    (i32::from(p[i]) * 77 + i32::from(p[i + 1]) * 150 + i32::from(p[i + 2]) * 29) >> 8
}

/// Coarse grayscale search followed by full-resolution validation. Ambiguous aliases are rejected.
fn displacement(a: &[u8], b: &[u8], w: usize, exact: bool, cancel: &AtomicBool) -> Option<usize> {
    let h = a.len() / (w * 4);
    if w < 8 || h < 16 {
        return None;
    }
    let max = if exact {
        (h * 3 / 4).min(h - 16)
    } else {
        h * 3 / 4
    };
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
                error += (gray(a, ia) - gray(b, ib)).unsigned_abs() as u64;
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
        let overlap = if exact {
            let stride = w * 4;
            a[dy * stride..]
                .chunks_exact(stride)
                .zip(b.chunks_exact(stride))
                .all(|(left, right)| !cancel.load(Ordering::Relaxed) && left == right)
                && textured(&a[dy * stride..])
        } else {
            validate_overlap(a, b, w, h, dy, cancel)
        };
        if overlap {
            if accepted.is_some_and(|old: usize| old.abs_diff(dy) > if exact { 0 } else { 2 }) {
                return None;
            }
            accepted.get_or_insert(dy);
        }
    }
    accepted
}

fn validate_overlap(
    a: &[u8],
    b: &[u8],
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
            let value = gray(a, ia);
            min = min.min(value);
            max = max.max(value);
            if (0..4).any(|c| a[ia + c].abs_diff(b[ib + c]) > 2) {
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
    fn fixed_frame(offset: usize, top: usize, bottom: usize) -> Arc<CapturedRegion> {
        let mut frame = (*image(offset)).clone();
        let stride = 96 * 4;
        frame.pixels[..top * stride].copy_from_slice(&image(9000).pixels[..top * stride]);
        let end = frame.pixels.len();
        let tail = end - bottom * stride;
        frame.pixels[tail..].copy_from_slice(&image(11000).pixels[tail..]);
        Arc::new(frame)
    }

    #[test]
    fn fixed_edges_are_kept_once_in_export_and_preview() {
        let cancel = AtomicBool::new(false);
        for (top, bottom) in [(12, 0), (0, 16), (12, 16)] {
            let first = fixed_frame(0, top, bottom);
            let mut stitch = Stitcher::new(first.clone()).unwrap();
            assert!(stitch.flatten(&cancel).unwrap().pixels == first.pixels);
            let stride = 96 * 4;
            let mut expected = first.pixels[..(160 - bottom) * stride].to_vec();
            for (before, offset) in [(0, 1), (1, 27), (27, 126)] {
                let frame = fixed_frame(offset, top, bottom);
                assert_eq!(
                    stitch.push(frame.clone(), &cancel).unwrap(),
                    Feedback::Accepted
                );
                let end = (160 - bottom) * stride;
                expected.extend_from_slice(&frame.pixels[end - (offset - before) * stride..end]);
                let mut full = expected.clone();
                full.extend_from_slice(&frame.pixels[end..]);
                assert_eq!(stitch.flatten(&cancel).unwrap().pixels, full);
                let preview = stitch.preview(96, 180);
                assert_eq!(preview.current.top, (offset + top) as i32);
                assert_eq!(preview.current.bottom, (offset + 160 - bottom) as i32);
                assert_eq!(
                    preview.image.pixels,
                    full[preview.visible.top as usize * stride..]
                );
            }
        }
    }

    #[test]
    fn locked_bands_reject_mutations_and_reverse_or_excessive_motion() {
        let cancel = AtomicBool::new(false);
        let mut stitch = Stitcher::new(fixed_frame(0, 12, 16)).unwrap();
        assert_eq!(
            stitch.push(fixed_frame(30, 12, 16), &cancel).unwrap(),
            Feedback::Accepted
        );
        let before = stitch.flatten(&cancel).unwrap();
        let mut changed = (*fixed_frame(60, 12, 16)).clone();
        changed.pixels[0] ^= 1;
        let mut floating = (*fixed_frame(60, 12, 16)).clone();
        for y in 50..70 {
            floating.pixels[(y * 96 + 20) * 4..(y * 96 + 50) * 4].fill(220);
        }
        for frame in [
            Arc::new(changed),
            Arc::new(floating),
            fixed_frame(60, 14, 16),
            fixed_frame(5, 12, 16),
            fixed_frame(130, 12, 16),
        ] {
            assert_eq!(stitch.push(frame, &cancel).unwrap(), Feedback::Uncertain);
            assert_eq!(stitch.flatten(&cancel).unwrap(), before);
        }
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            stitch.push(fixed_frame(60, 12, 16), &cancel).unwrap(),
            Feedback::Cancelled
        );
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            stitch.push(fixed_frame(60, 12, 16), &cancel).unwrap(),
            Feedback::Accepted
        );
    }

    #[test]
    fn fixed_footer_respects_buffer_limit_and_retains_output() {
        let cancel = AtomicBool::new(false);
        // A wide frame reaches the conservative buffer limit before the height limit.
        let width = 8000;
        let frame = |offset| {
            let mut frame = (*image_width(offset, width)).clone();
            let footer = (160 - 12) * width * 4;
            frame.pixels[footer..].copy_from_slice(&image_width(9000, width).pixels[footer..]);
            Arc::new(frame)
        };
        let mut stitch = Stitcher::new(frame(0)).unwrap();
        assert_eq!(stitch.push(frame(90), &cancel).unwrap(), Feedback::Accepted);
        let mut offset = 90;
        loop {
            let height = stitch.height;
            let anchor = stitch.anchor.clone();
            match stitch.push(frame(offset + 90), &cancel).unwrap() {
                Feedback::Accepted => {
                    offset += 90;
                    assert!(
                        offset < 2000,
                        "buffer limit should precede the height limit"
                    );
                }
                Feedback::Limit => {
                    assert_eq!(stitch.height, height);
                    assert!(Arc::ptr_eq(&stitch.anchor, &anchor));
                    assert_eq!(stitch.retained_bytes(), (height + 160) * width * 4);
                    break;
                }
                feedback => panic!("unexpected feedback: {feedback:?}"),
            }
        }
    }

    #[test]
    fn stationary_edges_need_texture_and_a_unique_boundary() {
        let cancel = AtomicBool::new(false);
        let stride = 96 * 4;
        let solid = |offset| {
            let mut frame = (*image(offset)).clone();
            frame.pixels[..12 * stride].fill(230);
            Arc::new(frame)
        };
        let mut stitch = Stitcher::new(solid(0)).unwrap();
        assert_eq!(
            stitch.push(solid(25), &cancel).unwrap(),
            Feedback::Uncertain
        );

        let first = fixed_frame(0, 12, 0);
        let mut moved = (*fixed_frame(25, 12, 0)).clone();
        let mut ambiguous = (*first).clone();
        let row = ambiguous.pixels[11 * stride..12 * stride].to_vec();
        ambiguous.pixels[36 * stride..37 * stride].copy_from_slice(&row);
        // Preserve the exact 25-row body overlap while making the 11-row boundary valid too.
        moved.pixels[11 * stride..12 * stride].copy_from_slice(&row);
        let first = Arc::new(ambiguous);
        let mut stitch = Stitcher::new(first.clone()).unwrap();
        assert_eq!(
            stitch.push(Arc::new(moved), &cancel).unwrap(),
            Feedback::Uncertain
        );
        assert!(stitch.flatten(&cancel).unwrap().pixels == first.pixels);
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
