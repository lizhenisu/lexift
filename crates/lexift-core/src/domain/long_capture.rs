//! Bounded scroll registration over immutable image strips.
use super::geometry::Rect;
use crate::{Error, Result, ports::screen_capture::CapturedRegion};
use std::collections::{HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
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
    Located,
    Uncertain,
    CaptureLost,
    Limit,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trim {
    Above,
    Below,
}

/// Restrict growth while the capture worker owns an automatic wheel step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    Both,
    TailOnly,
}

impl Preview {
    pub fn can_trim(&self, direction: Trim) -> bool {
        self.can_trim_on(Axis::Vertical, direction)
    }

    pub fn can_trim_on(&self, axis: Axis, direction: Trim) -> bool {
        let Some(body) = self.body else { return false };
        let (current, body) = match axis {
            Axis::Vertical => (self.current, body),
            Axis::Horizontal => (transpose_rect(self.current), transpose_rect(body)),
        };
        current.bottom > current.top
            && match direction {
                Trim::Above => current.top > body.top,
                Trim::Below => current.bottom < body.bottom,
            }
    }
}

/// One bounded view of accepted pixels, in original image coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub image: Arc<CapturedRegion>,
    pub extent: Rect,
    pub visible: Rect,
    pub current: Rect,
    pub body: Option<Rect>,
}

/// Retains accepted body strips and the full captures at both boundaries.
pub struct Stitcher {
    axis: Axis,
    width: usize,
    height: usize,
    head: Arc<CapturedRegion>,
    tail: Arc<CapturedRegion>,
    strips: VecDeque<Arc<[u8]>>,
    bands: Option<Bands>,
    row_keys: VecDeque<u64>,
    known_rows: HashSet<u64>,
    position: Option<usize>,
    preview_position: usize,
    located: Option<Arc<CapturedRegion>>,
}

/// Fixed rows in physical capture coordinates. Zero rows means ordinary stitching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bands {
    top: usize,
    bottom: usize,
}

fn row_key(row: &[u8]) -> u64 {
    let mut hash = DefaultHasher::new();
    row.hash(&mut hash);
    hash.finish()
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

    fn displacement(
        self,
        a: &[u8],
        b: &[u8],
        width: usize,
        exact: bool,
        cancel: &AtomicBool,
    ) -> Option<usize> {
        let stride = width * 4;
        if !self.matches(a, b, stride) {
            return None;
        }
        displacement(
            self.body(a, stride),
            self.body(b, stride),
            width,
            exact || self != Self::default(),
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

fn initial_match(
    a: &[u8],
    b: &[u8],
    width: usize,
    exact: bool,
    cancel: &AtomicBool,
) -> Option<(Bands, usize)> {
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
        if let Some(dy) = bands.displacement(a, b, width, exact, cancel) {
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

fn transpose_rect(rect: Rect) -> Rect {
    Rect {
        left: rect.top,
        top: rect.left,
        right: rect.bottom,
        bottom: rect.right,
    }
}

/// Store horizontal frames as column strips using the same registration and history machinery.
fn transpose(frame: &CapturedRegion, cancel: &AtomicBool) -> Result<CapturedRegion> {
    let (width, height) = dimensions(frame)?;
    if width > MAX_HEIGHT {
        return Err(Error::new("Invalid long capture frame"));
    }
    let mut pixels = vec![0; frame.pixels.len()];
    for x in 0..width {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::new("Long capture cancelled"));
        }
        for y in 0..height {
            let src = (y * width + x) * 4;
            let dst = (x * height + y) * 4;
            pixels[dst..dst + 4].copy_from_slice(&frame.pixels[src..src + 4]);
        }
    }
    Ok(CapturedRegion {
        bounds: transpose_rect(frame.bounds),
        pixels,
    })
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
        Self::new_on_axis(first, Axis::Vertical)
    }

    fn new_on_axis(first: Arc<CapturedRegion>, axis: Axis) -> Result<Self> {
        let (width, height) = dimensions(&first)?;
        if width * height * 60 > MAX_BUFFER_BYTES {
            return Err(Error::new("Long capture buffer limit"));
        }
        Ok(Self {
            axis,
            width,
            height,
            strips: VecDeque::from([Arc::from(first.pixels.as_slice())]),
            row_keys: first.pixels.chunks_exact(width * 4).map(row_key).collect(),
            known_rows: HashSet::new(),
            head: first.clone(),
            tail: first,
            bands: None,
            position: Some(0),
            located: None,
            preview_position: 0,
        })
    }
    pub fn size(&self) -> (usize, usize) {
        match self.axis {
            Axis::Vertical => (self.width, self.height),
            Axis::Horizontal => (self.height, self.width),
        }
    }
    pub fn axis(&self) -> Axis {
        self.axis
    }
    pub fn change_axis(&mut self, axis: Axis, cancel: &AtomicBool) -> Result<bool> {
        if axis == self.axis || self.bands.is_some() {
            return Ok(false);
        }
        let first = Arc::new(transpose(&self.head, cancel)?);
        *self = Self::new_on_axis(first, axis)?;
        Ok(true)
    }
    pub fn retained_bytes(&self) -> usize {
        let stride = self.width * 4;
        let bands = self.bands.unwrap_or_default();
        (self.height - bands.top - bands.bottom) * stride
            + self.head.pixels.len()
            + if Arc::ptr_eq(&self.head, &self.tail) {
                0
            } else {
                self.tail.pixels.len()
            }
            + self.located.as_ref().map_or(0, |frame| {
                if Arc::ptr_eq(frame, &self.head) || Arc::ptr_eq(frame, &self.tail) {
                    0
                } else {
                    frame.pixels.len()
                }
            })
    }

    pub fn invalidate_position(&mut self) {
        self.position = None;
        self.located = None;
    }

    pub fn at_tail(&self) -> bool {
        self.position == Some(self.height - self.frame_height())
    }

    pub fn position(&self) -> Option<usize> {
        self.position
    }

    fn frame_height(&self) -> usize {
        self.head.pixels.len() / (self.width * 4)
    }

    fn confirm_position(&mut self, offset: usize) {
        self.position = Some(offset);
        self.preview_position = offset;
    }

    /// Match retained rows without flattening. Stop before expensive or ambiguous comparisons.
    fn locate(
        &self,
        frame: &CapturedRegion,
        cancel: &AtomicBool,
    ) -> std::result::Result<Option<usize>, ()> {
        let Some(bands) = self.bands else {
            return Ok(None);
        };
        let stride = self.width * 4;
        if !bands.matches(&self.head.pixels, &frame.pixels, stride) {
            return Ok(None);
        }
        let body = bands.body(&frame.pixels, stride);
        let height = body.len() / stride;
        if [0, height / 2, height - 1].into_iter().any(|y| {
            !self
                .known_rows
                .contains(&row_key(&body[y * stride..(y + 1) * stride]))
        }) {
            return Ok(None);
        }
        let mut keys = Vec::new();
        for row in body.chunks_exact(stride) {
            if cancel.load(Ordering::Relaxed) {
                return Err(());
            }
            keys.push(row_key(row));
        }
        // A prefix table keeps long runs of identical rows linear, including blank margins.
        let mut prefix = vec![0; keys.len()];
        for i in 1..keys.len() {
            let mut matching = prefix[i - 1];
            while matching > 0 && keys[i] != keys[matching] {
                matching = prefix[matching - 1];
            }
            prefix[i] = matching + usize::from(keys[i] == keys[matching]);
        }
        let rows = std::cell::OnceCell::<Vec<&[u8]>>::new();
        let mut found = None;
        let mut budget = 2 * body.len();
        let mut matching = 0;
        for (index, key) in self.row_keys.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(());
            }
            while matching > 0 && *key != keys[matching] {
                matching = prefix[matching - 1];
            }
            matching += usize::from(*key == keys[matching]);
            if matching < keys.len() {
                continue;
            }
            let offset = index + 1 - keys.len();
            matching = prefix[matching - 1];
            let rows = rows.get_or_init(|| {
                self.body_strips()
                    .flat_map(|strip| strip.chunks_exact(stride))
                    .collect()
            });
            let candidate = &rows[offset..=index];
            let mut equal = true;
            for (row, expected) in candidate.iter().zip(body.chunks_exact(stride)) {
                budget = budget.checked_sub(stride).ok_or(())?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(());
                }
                if *row != expected {
                    equal = false;
                    break;
                }
            }
            if equal && found.replace(offset).is_some() {
                return Err(());
            }
        }
        Ok(found)
    }
    pub fn push(&mut self, frame: Arc<CapturedRegion>, cancel: &AtomicBool) -> Result<Feedback> {
        let feedback = self.sample(frame, Growth::Both, cancel)?;
        if matches!(feedback, Feedback::Uncertain | Feedback::Limit) {
            self.invalidate_position();
        }
        Ok(feedback)
    }

    /// Try the latest compositor sample while leaving the confirmed position intact
    /// on rejection. The worker invalidates it once the viewport settles or times out.
    pub fn sample(
        &mut self,
        frame: Arc<CapturedRegion>,
        growth: Growth,
        cancel: &AtomicBool,
    ) -> Result<Feedback> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Feedback::Cancelled);
        }
        let frame = if self.axis == Axis::Horizontal {
            if transpose_rect(frame.bounds) != self.head.bounds {
                return Err(Error::new("Long capture geometry changed"));
            }
            match transpose(&frame, cancel) {
                Ok(frame) => Arc::new(frame),
                Err(_) if cancel.load(Ordering::Relaxed) => return Ok(Feedback::Cancelled),
                Err(error) => return Err(error),
            }
        } else {
            frame
        };
        let (w, h) = dimensions(&frame)?;
        if frame.bounds != self.head.bounds || w != self.width {
            return Err(Error::new("Long capture geometry changed"));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Feedback::Cancelled);
        }

        let at_head = self.position == Some(0);
        let at_tail = self.at_tail();
        let same_as_tail = self.tail.pixels == frame.pixels;
        if same_as_tail && (at_tail || self.bands.is_none()) {
            self.confirm_position(self.height - h);
            return Ok(Feedback::Unchanged);
        }
        let same_as_head = if Arc::ptr_eq(&self.head, &self.tail) {
            same_as_tail
        } else {
            self.head.pixels == frame.pixels
        };
        if (at_head && same_as_head)
            || self
                .located
                .as_ref()
                .is_some_and(|known| known.pixels == frame.pixels)
        {
            return Ok(Feedback::Located);
        }

        let unavailable = || {
            if cancel.load(Ordering::Relaxed) {
                Feedback::Cancelled
            } else {
                Feedback::Uncertain
            }
        };
        match self.locate(&frame, cancel) {
            Ok(Some(offset)) => {
                self.confirm_position(offset);
                self.located = None;
                if self.at_tail() {
                    return Ok(Feedback::Unchanged);
                }
                if offset != 0 {
                    self.located = Some(frame);
                }
                return Ok(Feedback::Located);
            }
            Err(()) => return Ok(unavailable()),
            Ok(None) => {}
        }

        // With transposed frames the same registration searches along physical columns.
        let exact = self.axis == Axis::Horizontal;
        let match_boundary = |a, b| {
            if let Some(bands) = self.bands {
                bands
                    .displacement(a, b, w, exact, cancel)
                    .map(|shift| (bands, shift))
            } else {
                initial_match(a, b, w, exact, cancel)
            }
        };
        let head = (growth == Growth::Both && at_head && !same_as_head && !same_as_tail)
            .then(|| match_boundary(&frame.pixels, &self.head.pixels))
            .flatten();
        let tail = (at_tail && !same_as_head && !same_as_tail)
            .then(|| match_boundary(&self.tail.pixels, &frame.pixels))
            .flatten();
        let (prepend, bands, shift) = match (head, tail) {
            (Some((bands, shift)), None) => (true, bands, shift),
            (None, Some((bands, shift))) => (false, bands, shift),
            _ => return Ok(unavailable()),
        };

        let Some(height) = self.height.checked_add(shift) else {
            return Ok(Feedback::Limit);
        };
        // Reserve flattening, output composition, both anchors and two in-flight captures.
        let pixels = w.checked_mul(height);
        if height > MAX_HEIGHT
            || pixels.is_none_or(|p| {
                p > MAX_PIXELS || p * 48 + frame.pixels.len() * 4 > MAX_BUFFER_BYTES
            })
        {
            return Ok(Feedback::Limit);
        }
        let stride = w * 4;
        let body = bands.body(&frame.pixels, stride);
        let new_pixels = if prepend {
            &body[..shift * stride]
        } else {
            &body[body.len() - shift * stride..]
        };
        let mut keys = Vec::with_capacity(shift);
        for row in new_pixels.chunks_exact(stride) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(Feedback::Cancelled);
            }
            keys.push(row_key(row));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Feedback::Cancelled);
        }

        if self.bands.is_none() {
            if bands != Bands::default() {
                self.strips[0] = Arc::from(bands.body(&self.head.pixels, stride));
                self.row_keys.truncate(h - bands.bottom);
                self.row_keys.drain(..bands.top);
            }
            self.known_rows.extend(self.row_keys.iter().copied());
        }
        self.known_rows.extend(keys.iter().copied());
        let strip = Arc::from(new_pixels);
        if prepend {
            self.strips.push_front(strip);
            for key in keys.into_iter().rev() {
                self.row_keys.push_front(key);
            }
            self.head = frame;
        } else {
            self.strips.push_back(strip);
            self.row_keys.extend(keys);
            self.tail = frame;
        }
        self.bands = Some(bands);
        self.height = height;
        self.located = None;
        self.confirm_position(if prepend { 0 } else { height - h });
        Ok(Feedback::Accepted)
    }

    fn body_strips(&self) -> impl Iterator<Item = &[u8]> {
        self.strips.iter().map(AsRef::as_ref)
    }

    pub fn trim(&mut self, direction: Trim) -> bool {
        let Some(offset) = self.position else {
            return false;
        };
        let stride = self.width * 4;
        let frame_height = self.frame_height();
        let removed = match direction {
            Trim::Above => offset,
            Trim::Below => self.height - offset - frame_height,
        };
        if removed == 0 {
            return false;
        }
        let frame = if offset == 0 {
            self.head.clone()
        } else if self.at_tail() {
            self.tail.clone()
        } else {
            self.located.take().expect("known interior capture")
        };
        let mut remaining = removed * stride;
        while remaining > 0 {
            let strip = match direction {
                Trim::Above => self.strips.pop_front(),
                Trim::Below => self.strips.pop_back(),
            }
            .expect("accepted body covers trim");
            if remaining >= strip.len() {
                remaining -= strip.len();
            } else {
                match direction {
                    Trim::Above => self.strips.push_front(Arc::from(&strip[remaining..])),
                    Trim::Below => self
                        .strips
                        .push_back(Arc::from(&strip[..strip.len() - remaining])),
                }
                break;
            }
        }
        self.height -= removed;
        match direction {
            Trim::Above => {
                self.row_keys.drain(..removed);
                self.head = frame;
                self.confirm_position(0);
            }
            Trim::Below => {
                self.row_keys.truncate(self.row_keys.len() - removed);
                self.tail = frame;
                self.confirm_position(self.height - frame_height);
            }
        }
        self.known_rows.clear();
        self.known_rows.extend(self.row_keys.iter().copied());
        true
    }

    fn output_strips(&self) -> impl Iterator<Item = &[u8]> {
        let bands = self.bands.unwrap_or_default();
        let stride = self.width * 4;
        std::iter::once(&self.head.pixels[..bands.top * stride])
            .chain(self.body_strips())
            .chain(std::iter::once(
                &self.tail.pixels[self.tail.pixels.len() - bands.bottom * stride..],
            ))
    }

    /// Flatten only at the immutable editor/output handoff, never for capture feedback.
    pub fn flatten(&self, cancel: &AtomicBool) -> Result<CapturedRegion> {
        if self.axis == Axis::Horizontal {
            let mut pixels = vec![0; self.width * self.height * 4];
            for (column, strip) in self
                .output_strips()
                .flat_map(|strip| strip.chunks_exact(self.width * 4))
                .enumerate()
            {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::new("Long capture cancelled"));
                }
                for (row, pixel) in strip.as_chunks::<4>().0.iter().enumerate() {
                    let offset = (row * self.height + column) * 4;
                    pixels[offset..offset + 4].copy_from_slice(pixel);
                }
            }
            return Ok(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: self.height as i32,
                    bottom: self.width as i32,
                },
                pixels,
            });
        }
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
        let (max_width, max_height) = match self.axis {
            Axis::Vertical => (max_width, max_height),
            Axis::Horizontal => (max_height, max_width),
        };
        let w = max_width.max(1).min(self.width);
        let visible_height = (max_height.max(1).saturating_mul(self.width) / w)
            .max(1)
            .min(self.height);
        let bands = self.bands.unwrap_or_default();
        let current = self
            .position
            .map(|offset| Rect {
                left: 0,
                top: (offset + bands.top) as i32,
                right: self.width as i32,
                bottom: (offset + self.frame_height() - bands.bottom) as i32,
            })
            .unwrap_or_default();
        let tail = self.height - visible_height;
        let start = if self.preview_position == self.height - self.frame_height() {
            tail
        } else if self.preview_position == 0 {
            0
        } else {
            (self.preview_position + (bands.top + self.frame_height() - bands.bottom) / 2)
                .saturating_sub(visible_height / 2)
                .min(tail)
        };
        let h = (visible_height * w / self.width).max(1);
        let horizontal = self.axis == Axis::Horizontal;
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
                let dst = if horizontal {
                    (x * h + y) * 4
                } else {
                    (y * w + x) * 4
                };
                pixels[dst..dst + 4].copy_from_slice(&strip[src..src + 4]);
            }
        }
        let mut preview = Preview {
            image: Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: if horizontal { h } else { w } as i32,
                    bottom: if horizontal { w } else { h } as i32,
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
                bottom: (start + visible_height) as i32,
            },
            current,
            body: Some(Rect {
                left: 0,
                top: bands.top as i32,
                right: self.width as i32,
                bottom: (self.height - bands.bottom) as i32,
            }),
        };
        if horizontal {
            preview.extent = transpose_rect(preview.extent);
            preview.visible = transpose_rect(preview.visible);
            preview.current = transpose_rect(preview.current);
            preview.body = preview.body.map(transpose_rect);
        }
        preview
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
    fn columns(offset: usize, width: usize) -> Arc<CapturedRegion> {
        let height = 40;
        let pixels = (0..height)
            .flat_map(|y| {
                (offset..offset + width).flat_map(move |x| {
                    let k = (x as u32 + 1).wrapping_mul(0x9e3779b9).rotate_left(y % 29)
                        ^ (y + 1).wrapping_mul(0x85ebca6b);
                    [k as u8, (k >> 8) as u8, (k >> 16) as u8, 255]
                })
            })
            .collect();
        Arc::new(CapturedRegion {
            bounds: Rect {
                left: -16,
                top: 8,
                right: width as i32 - 16,
                bottom: height as i32 + 8,
            },
            pixels,
        })
    }

    #[test]
    fn horizontal_output_preview_and_trim_share_exact_column_order() {
        let cancel = AtomicBool::new(false);
        let mut stitch = Stitcher::new(columns(0, 80)).unwrap();
        assert!(stitch.change_axis(Axis::Horizontal, &cancel).unwrap());
        assert_eq!(
            stitch.flatten(&cancel).unwrap().pixels,
            columns(0, 80).pixels
        );
        assert!(stitch.change_axis(Axis::Vertical, &cancel).unwrap());
        assert!(stitch.change_axis(Axis::Horizontal, &cancel).unwrap());
        assert_eq!(
            stitch.push(columns(0, 80), &cancel).unwrap(),
            Feedback::Unchanged
        );
        for offset in [1, 33, 73] {
            assert_eq!(
                stitch.push(columns(offset, 80), &cancel).unwrap(),
                Feedback::Accepted
            );
        }
        assert_eq!(stitch.size(), (153, 40));
        assert!(!stitch.change_axis(Axis::Vertical, &cancel).unwrap());
        assert_eq!(
            stitch.flatten(&cancel).unwrap().pixels,
            columns(0, 153).pixels
        );
        let preview = stitch.preview(90, 40);
        assert_eq!(preview.visible.left, 63);
        assert_eq!(preview.current.left, 73);
        assert_eq!(preview.image.pixels, columns(63, 90).pixels);
        assert_eq!(
            stitch.push(columns(33, 80), &cancel).unwrap(),
            Feedback::Located
        );
        assert!(
            stitch
                .preview(90, 40)
                .can_trim_on(Axis::Horizontal, Trim::Above)
        );
        assert!(stitch.trim(Trim::Above));
        assert_eq!(
            stitch.flatten(&cancel).unwrap().pixels,
            columns(33, 120).pixels
        );
        assert_eq!(
            stitch.push(columns(49, 80), &cancel).unwrap(),
            Feedback::Located
        );
        assert!(stitch.trim(Trim::Below));
        assert_eq!(
            stitch.flatten(&cancel).unwrap().pixels,
            columns(33, 96).pixels
        );
        assert_eq!(
            stitch.push(columns(77, 80), &cancel).unwrap(),
            Feedback::Accepted
        );
        assert_eq!(
            stitch.flatten(&cancel).unwrap().pixels,
            columns(33, 124).pixels
        );
    }

    fn fixed_columns(
        offset: usize,
        width: usize,
        left: usize,
        right: usize,
    ) -> Arc<CapturedRegion> {
        let mut image = (*columns(offset, width)).clone();
        let header = columns(9000, width);
        let footer = columns(11000, right);
        let stride = width * 4;
        for (y, row) in image.pixels.chunks_exact_mut(stride).enumerate() {
            let start = y * stride;
            row[..left * 4].copy_from_slice(&header.pixels[start..start + left * 4]);
            let end = stride - right * 4;
            row[end..].copy_from_slice(&footer.pixels[y * right * 4..(y + 1) * right * 4]);
        }
        Arc::new(image)
    }

    #[test]
    fn horizontal_fixed_columns_are_kept_once_through_preview_trim_and_continuation() {
        let cancel = AtomicBool::new(false);
        for (left, right) in [(8, 0), (0, 12), (8, 12)] {
            let frame = |offset| fixed_columns(offset, 80, left, right);
            let mut stitch = Stitcher::new(frame(0)).unwrap();
            assert!(stitch.change_axis(Axis::Horizontal, &cancel).unwrap());
            assert_eq!(stitch.flatten(&cancel).unwrap().pixels, frame(0).pixels);
            for offset in [1, 25, 55] {
                assert_eq!(
                    stitch.push(frame(offset), &cancel).unwrap(),
                    Feedback::Accepted
                );
            }
            let expected = fixed_columns(0, 135, left, right);
            assert_eq!(stitch.flatten(&cancel).unwrap().pixels, expected.pixels);
            let full = stitch.preview(135, 40);
            assert_eq!(full.image.pixels, expected.pixels);
            let tail = stitch.preview(90, 40);
            assert_eq!(tail.visible.left, 45);
            assert_eq!(tail.current.left, (55 + left) as i32);
            assert_eq!(tail.current.right, (135 - right) as i32);
            let clipped: Vec<_> = expected
                .pixels
                .as_chunks::<{ 135 * 4 }>()
                .0
                .iter()
                .flat_map(|row| row[45 * 4..].iter().copied())
                .collect();
            assert_eq!(tail.image.pixels, clipped);

            assert_eq!(stitch.push(frame(25), &cancel).unwrap(), Feedback::Located);
            assert!(stitch.trim(Trim::Above));
            assert_eq!(
                stitch.flatten(&cancel).unwrap().pixels,
                fixed_columns(25, 110, left, right).pixels
            );
            assert_eq!(stitch.push(frame(40), &cancel).unwrap(), Feedback::Located);
            assert!(stitch.trim(Trim::Below));
            assert_eq!(
                stitch.flatten(&cancel).unwrap().pixels,
                fixed_columns(25, 95, left, right).pixels
            );
            assert_eq!(stitch.push(frame(65), &cancel).unwrap(), Feedback::Accepted);
            assert_eq!(
                stitch.flatten(&cancel).unwrap().pixels,
                fixed_columns(25, 120, left, right).pixels
            );
        }
    }

    #[test]
    fn horizontal_band_mutations_and_lost_overlap_preserve_the_accepted_image() {
        let cancel = AtomicBool::new(false);
        let first = fixed_columns(0, 80, 8, 12);
        let mut stitch = Stitcher::new(first.clone()).unwrap();
        assert!(stitch.change_axis(Axis::Horizontal, &cancel).unwrap());
        let second = fixed_columns(25, 80, 8, 12);
        assert_eq!(
            stitch.push(second.clone(), &cancel).unwrap(),
            Feedback::Accepted
        );
        let accepted = stitch.flatten(&cancel).unwrap();
        let mut changed = (*fixed_columns(45, 80, 8, 12)).clone();
        changed.pixels[0] ^= 0x80;
        for next in [
            Arc::new(changed),
            fixed_columns(45, 80, 12, 12),
            fixed_columns(100, 80, 8, 12),
        ] {
            assert_eq!(stitch.push(next, &cancel).unwrap(), Feedback::Uncertain);
            assert_eq!(stitch.flatten(&cancel).unwrap().pixels, accepted.pixels);
            assert!(stitch.position().is_none());
        }
        assert_eq!(stitch.push(second, &cancel).unwrap(), Feedback::Unchanged);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(stitch.push(first, &cancel).unwrap(), Feedback::Cancelled);
    }

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
        assert_eq!(stitch.push(image(320), &cancel).unwrap(), Feedback::Located);
        let earlier = stitch.preview(48, 120);
        assert_eq!(earlier.extent, p.extent);
        assert!(earlier.visible.top < p.visible.top);
        assert_eq!(earlier.current.top, 320);
        assert_eq!(stitch.flatten(&cancel).unwrap(), full);
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
        assert_eq!(
            stitch.push(frame(15), &AtomicBool::new(false)).unwrap(),
            Feedback::Located
        );
        assert_eq!(stitch.position(), Some(15));
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
        assert_eq!(s.push(image(100), &cancel).unwrap(), Feedback::Located);
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
    fn both_axes_grow_at_either_boundary_and_resume_after_cropping() {
        let cancel = AtomicBool::new(false);
        for axis in [Axis::Vertical, Axis::Horizontal] {
            for (top, bottom) in [(0, 0), (12, 16)] {
                let orient = |frame: CapturedRegion| {
                    if axis == Axis::Horizontal {
                        transpose(&frame, &cancel).unwrap()
                    } else {
                        frame
                    }
                };
                let frame = |offset| Arc::new(orient((*fixed_frame(offset, top, bottom)).clone()));
                let expected = |start, end| {
                    let stride = 96 * 4;
                    let first = fixed_frame(start, top, bottom);
                    let footer = (160 - bottom) * stride;
                    let mut pixels = first.pixels[..footer].to_vec();
                    let extra = image(start + 160 - bottom);
                    pixels.extend_from_slice(&extra.pixels[..(end - start - 160) * stride]);
                    pixels.extend_from_slice(&first.pixels[footer..]);
                    orient(CapturedRegion {
                        bounds: Rect {
                            left: 0,
                            top: 0,
                            right: 96,
                            bottom: (end - start) as i32,
                        },
                        pixels,
                    })
                };
                let mut stitch = Stitcher::new(frame(120)).unwrap();
                stitch.change_axis(axis, &cancel).unwrap();
                for offset in [100, 70] {
                    assert_eq!(
                        stitch.push(frame(offset), &cancel).unwrap(),
                        Feedback::Accepted
                    );
                }
                assert_eq!(stitch.push(frame(95), &cancel).unwrap(), Feedback::Located);
                let preview = stitch.preview(80, 80);
                assert_eq!(
                    stitch.push(frame(50), &cancel).unwrap(),
                    Feedback::Uncertain
                );
                let unknown = stitch.preview(80, 80);
                assert_eq!(unknown.visible, preview.visible);
                assert_eq!(unknown.image.pixels, preview.image.pixels);
                assert_eq!(unknown.current, Rect::default());
                assert_eq!(stitch.push(frame(70), &cancel).unwrap(), Feedback::Located);
                assert_eq!(stitch.push(frame(48), &cancel).unwrap(), Feedback::Accepted);
                assert_eq!(stitch.position(), Some(0));
                assert_eq!(
                    stitch.push(frame(120), &cancel).unwrap(),
                    Feedback::Unchanged
                );
                assert_eq!(
                    stitch.push(frame(145), &cancel).unwrap(),
                    Feedback::Accepted
                );
                assert!(stitch.flatten(&cancel).unwrap().pixels == expected(48, 305).pixels);

                assert_eq!(stitch.push(frame(80), &cancel).unwrap(), Feedback::Located);
                assert!(stitch.trim(Trim::Above));
                assert_eq!(stitch.push(frame(65), &cancel).unwrap(), Feedback::Accepted);
                assert_eq!(stitch.push(frame(115), &cancel).unwrap(), Feedback::Located);
                assert!(stitch.trim(Trim::Below));
                assert!(stitch.at_tail());
                assert_eq!(
                    stitch.push(frame(135), &cancel).unwrap(),
                    Feedback::Accepted
                );
                let expected = expected(65, 295);
                assert_eq!(
                    stitch.size(),
                    (
                        expected.bounds.right as usize,
                        expected.bounds.bottom as usize
                    )
                );
                assert!(stitch.flatten(&cancel).unwrap().pixels == expected.pixels);
                assert!(stitch.preview(300, 300).image.pixels == expected.pixels);
                assert!(stitch.trim(Trim::Above));
                assert!(Arc::ptr_eq(&stitch.head, &stitch.tail));
            }
        }
    }

    #[test]
    fn automatic_growth_rejects_new_head_content_but_can_locate_both_ends() {
        let cancel = AtomicBool::new(false);
        let mut stitch = Stitcher::new(image(50)).unwrap();
        assert_eq!(
            stitch.sample(image(40), Growth::TailOnly, &cancel).unwrap(),
            Feedback::Uncertain
        );
        assert_eq!(stitch.size(), (96, 160));
        assert_eq!(
            stitch.push(image(50), &cancel).unwrap(),
            Feedback::Unchanged
        );
        assert_eq!(stitch.push(image(40), &cancel).unwrap(), Feedback::Accepted);
        assert_eq!(
            stitch.sample(image(50), Growth::TailOnly, &cancel).unwrap(),
            Feedback::Unchanged
        );
        assert_eq!(
            stitch.sample(image(65), Growth::TailOnly, &cancel).unwrap(),
            Feedback::Accepted
        );
        assert_eq!(
            stitch.sample(image(40), Growth::TailOnly, &cancel).unwrap(),
            Feedback::Located
        );
        assert!(!stitch.at_tail());
        assert_eq!(
            stitch.sample(image(35), Growth::TailOnly, &cancel).unwrap(),
            Feedback::Uncertain
        );
        assert_eq!(stitch.size(), (96, 185));
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
    fn locked_bands_reject_mutations_and_locate_reverse_without_appending() {
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
            fixed_frame(130, 12, 16),
        ] {
            assert_eq!(stitch.push(frame, &cancel).unwrap(), Feedback::Uncertain);
            assert_eq!(stitch.flatten(&cancel).unwrap(), before);
        }
        assert_eq!(
            stitch.push(fixed_frame(5, 12, 16), &cancel).unwrap(),
            Feedback::Located
        );
        assert_eq!(stitch.flatten(&cancel).unwrap(), before);
        assert_eq!(
            stitch.push(fixed_frame(30, 12, 16), &cancel).unwrap(),
            Feedback::Unchanged
        );
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
    fn trim_uses_the_live_frame_and_preserves_fixed_bands_and_continuation() {
        let cancel = AtomicBool::new(false);
        let stride = 96 * 4;
        for (top, bottom) in [(0, 0), (12, 0), (0, 16), (12, 16)] {
            let frame = |offset| fixed_frame(offset, top, bottom);
            let mut stitch = Stitcher::new(frame(0)).unwrap();
            assert!(!stitch.trim(Trim::Above));
            assert!(!stitch.trim(Trim::Below));
            for offset in [28, 68, 118] {
                assert_eq!(
                    stitch.push(frame(offset), &cancel).unwrap(),
                    Feedback::Accepted
                );
            }
            let original = stitch.flatten(&cancel).unwrap().pixels;
            assert_eq!(stitch.push(frame(15), &cancel).unwrap(), Feedback::Located);
            let view = stitch.preview(96, 165);
            assert_eq!(view.current.top, (15 + top) as i32);
            assert!(view.can_trim(Trim::Above) && view.can_trim(Trim::Below));
            assert!(
                view.image.pixels
                    == original
                        [view.visible.top as usize * stride..view.visible.bottom as usize * stride]
            );
            assert!(stitch.trim(Trim::Above));
            assert!(!stitch.at_tail());
            assert_eq!(stitch.push(frame(57), &cancel).unwrap(), Feedback::Located);
            assert!(stitch.trim(Trim::Below));
            assert!(stitch.at_tail());
            assert!(!stitch.trim(Trim::Below));
            let mut expected = original[..top * stride].to_vec();
            expected
                .extend_from_slice(&original[(top + 15) * stride..(57 + 160 - bottom) * stride]);
            expected.extend_from_slice(&original[original.len() - bottom * stride..]);
            assert!(stitch.flatten(&cancel).unwrap().pixels == expected);
            assert_eq!(stitch.push(frame(79), &cancel).unwrap(), Feedback::Accepted);
            assert_eq!(stitch.size(), (96, 224));
            let result = stitch.flatten(&cancel).unwrap();
            expected.truncate(expected.len() - bottom * stride);
            expected.extend_from_slice(&frame(79).pixels[(160 - bottom - 22) * stride..]);
            assert!(result.pixels == expected);
        }
    }

    #[test]
    fn duplicate_history_matches_do_not_offer_a_crop() {
        let cancel = AtomicBool::new(false);
        let mut stitch = Stitcher::new(image(0)).unwrap();
        assert_eq!(stitch.push(image(50), &cancel).unwrap(), Feedback::Accepted);
        stitch
            .strips
            .push_back(Arc::from(image(0).pixels.as_slice()));
        stitch
            .row_keys
            .extend(image(0).pixels.chunks_exact(stitch.width * 4).map(row_key));
        stitch.height += 160;
        assert_eq!(stitch.locate(&image(0), &cancel), Err(()));
        stitch.invalidate_position();
        assert!(!stitch.preview(96, 160).can_trim(Trim::Above));
        assert!(!stitch.trim(Trim::Below));
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
            let anchor = stitch.tail.clone();
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
                    assert!(Arc::ptr_eq(&stitch.tail, &anchor));
                    assert_eq!(stitch.retained_bytes(), (height - 12 + 320) * width * 4);
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
        let first = fixed_frame(0, 12, 0);
        let mut moved = (*fixed_frame(25, 12, 0)).clone();
        let mut ambiguous = (*first).clone();
        let row = ambiguous.pixels[11 * stride..12 * stride].to_vec();
        ambiguous.pixels[36 * stride..37 * stride].copy_from_slice(&row);
        // Preserve the exact 25-row body overlap while making the 11-row boundary valid too.
        moved.pixels[11 * stride..12 * stride].copy_from_slice(&row);
        for (first, moved) in [
            (solid(0), solid(25)),
            (Arc::new(ambiguous), Arc::new(moved)),
        ] {
            for axis in [Axis::Vertical, Axis::Horizontal] {
                let orient = |image: &Arc<CapturedRegion>| match axis {
                    Axis::Vertical => image.clone(),
                    Axis::Horizontal => Arc::new(transpose(image, &cancel).unwrap()),
                };
                let expected = orient(&first);
                let mut stitch = Stitcher::new(expected.clone()).unwrap();
                if axis == Axis::Horizontal {
                    assert!(stitch.change_axis(axis, &cancel).unwrap());
                }
                assert_eq!(
                    stitch.push(orient(&moved), &cancel).unwrap(),
                    Feedback::Uncertain
                );
                assert!(stitch.flatten(&cancel).unwrap().pixels == expected.pixels);
            }
        }
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
