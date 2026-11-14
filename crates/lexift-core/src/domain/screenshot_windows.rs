use super::{
    geometry::{Point, Rect},
    window_candidate::WindowCandidate,
};
use crate::{Error, Result, ports::screen_capture::CapturedRegion};

/// Frozen front-to-back layer. Uncertain layers occlude but cannot supply pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub candidate: WindowCandidate,
    pub coverage: Vec<Rect>,
    pub selectable: bool,
}

pub(super) fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.left < r.right && r.top < r.bottom).then_some(r)
}

pub(super) fn subtract(a: Rect, b: Rect, out: &mut Vec<Rect>) {
    let Some(i) = intersection(a, b) else {
        out.push(a);
        return;
    };
    for r in [
        Rect { bottom: i.top, ..a },
        Rect { top: i.bottom, ..a },
        Rect {
            top: i.top,
            bottom: i.bottom,
            right: i.left,
            ..a
        },
        Rect {
            top: i.top,
            bottom: i.bottom,
            left: i.right,
            ..a
        },
    ] {
        if r.left < r.right && r.top < r.bottom {
            out.push(r);
        }
    }
}

/// Ignore ordering changes between disjoint windows, which cannot alter ownership.
pub fn stable(before: &[Layer], after: &[Layer]) -> bool {
    if before.len() != after.len() {
        return false;
    }
    let mut ranks = Vec::with_capacity(before.len());
    for layer in before {
        let Some(rank) = after.iter().position(|current| {
            current.candidate.window == layer.candidate.window
                && current.candidate.process_id == layer.candidate.process_id
                && current.candidate.bounds == layer.candidate.bounds
                && current.selectable == layer.selectable
                && current.coverage == layer.coverage
        }) else {
            return false;
        };
        ranks.push(rank);
    }
    before.iter().enumerate().all(|(i, a)| {
        before[..i].iter().enumerate().all(|(j, b)| {
            ranks[j] < ranks[i] || intersection(a.candidate.bounds, b.candidate.bounds).is_none()
        })
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub layers: Vec<Layer>,
    pub selected: Vec<u32>,
    pub mask: Vec<Rect>,
}

impl Selection {
    pub fn toggle(&mut self, point: Point, layout: &[Rect]) -> Result<()> {
        let pixel = Rect {
            left: point.x,
            top: point.y,
            right: point.x.saturating_add(1),
            bottom: point.y.saturating_add(1),
        };
        if !layout.iter().any(|r| intersection(*r, pixel).is_some()) {
            return Ok(());
        }
        let Some(layer) = self
            .layers
            .iter()
            .find(|l| l.coverage.iter().any(|r| intersection(*r, pixel).is_some()))
            .filter(|l| l.selectable)
        else {
            return Ok(());
        };
        let order = layer.candidate.order;
        let mut selected = self.selected.clone();
        if let Some(index) = selected.iter().position(|value| *value == order) {
            selected.remove(index);
        } else {
            selected.push(order);
        }
        let mut mask = Vec::new();
        for (index, layer) in self.layers.iter().enumerate() {
            if !selected.contains(&layer.candidate.order) {
                continue;
            }
            for &r in &layer.coverage {
                let mut visible: Vec<_> =
                    layout.iter().filter_map(|&b| intersection(r, b)).collect();
                for front in &self.layers[..index] {
                    for &b in &front.coverage {
                        let mut rest = Vec::new();
                        for part in visible {
                            subtract(part, b, &mut rest);
                        }
                        if rest.len() > 16384 {
                            return Err(Error::new("Window selection is too complex"));
                        }
                        visible = rest;
                    }
                }
                mask.extend(visible);
                if mask.len() > 16384 {
                    return Err(Error::new("Window selection is too complex"));
                }
            }
        }
        self.selected = selected;
        self.mask = mask;
        Ok(())
    }
}

/// Clear unselected pixels in a source-coordinate raster, without copying its backing buffer.
pub fn clip_pixels(
    pixels: &mut [u8],
    bounds: Rect,
    mask: &[Rect],
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use std::sync::atomic::Ordering;
    let stride = usize::try_from(i64::from(bounds.right) - i64::from(bounds.left))
        .ok()
        .and_then(|width| width.checked_mul(4))
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::new("Invalid masked image"))?;
    if !pixels.len().is_multiple_of(stride)
        || i64::try_from(pixels.len() / stride).ok()
            != Some(i64::from(bounds.bottom) - i64::from(bounds.top))
    {
        return Err(Error::new("Invalid masked image"));
    }
    let mut pending: Vec<_> = mask
        .iter()
        .filter_map(|&part| intersection(bounds, part))
        .collect();
    pending.sort_unstable_by_key(|part| part.top);
    let mut pending = pending.into_iter().peekable();
    let mut active: Vec<Rect> = Vec::new();
    for (y, row) in (bounds.top..bounds.bottom).zip(pixels.chunks_exact_mut(stride)) {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::new("Annotation export cancelled"));
        }
        active.retain(|part| part.bottom > y);
        while pending.peek().is_some_and(|part| part.top == y) {
            let part = pending.next().unwrap();
            let at = active.partition_point(|old| old.left < part.left);
            active.insert(at, part);
        }
        let mut left = 0;
        for part in &active {
            let start = (i64::from(part.left) - i64::from(bounds.left)) as usize * 4;
            let end = (i64::from(part.right) - i64::from(bounds.left)) as usize * 4;
            if start > left {
                row[left..start].fill(0);
            }
            left = left.max(end);
        }
        row[left..].fill(0);
    }
    Ok(())
}

pub fn compose(frames: &[CapturedRegion], mask: &[Rect]) -> Result<CapturedRegion> {
    let bounds = mask
        .iter()
        .copied()
        .reduce(|a, b| Rect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })
        .ok_or_else(|| Error::new("Select a visible window"))?;
    let width = i64::from(bounds.right) - i64::from(bounds.left);
    let height = i64::from(bounds.bottom) - i64::from(bounds.top);
    let size = width
        .checked_mul(height)
        .filter(|&n| n > 0 && n <= 40_000_000)
        .ok_or_else(|| Error::new("Window selection is too large"))? as usize
        * 4;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(size)
        .map_err(|_| Error::new("Window selection is too large"))?;
    pixels.resize(size, 0);
    for frame in frames {
        let fw = (frame.bounds.right - frame.bounds.left) as usize;
        let fh = (frame.bounds.bottom - frame.bounds.top) as usize;
        if fw.checked_mul(fh).and_then(|n| n.checked_mul(4)) != Some(frame.pixels.len()) {
            return Err(Error::new("Invalid frozen image"));
        }
        for part in mask.iter().filter_map(|&r| intersection(r, frame.bounds)) {
            for y in part.top..part.bottom {
                let src = ((y - frame.bounds.top) as usize * fw
                    + (part.left - frame.bounds.left) as usize)
                    * 4;
                let dst = ((y - bounds.top) as usize * width as usize
                    + (part.left - bounds.left) as usize)
                    * 4;
                let len = (part.right - part.left) as usize * 4;
                pixels[dst..dst + len].copy_from_slice(&frame.pixels[src..src + len]);
            }
        }
    }
    Ok(CapturedRegion { bounds, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::magnifier::WindowToken;

    fn span(left: i32, right: i32) -> Rect {
        Rect {
            left,
            right,
            top: 0,
            bottom: 1,
        }
    }

    fn layer(order: u32, left: i32, right: i32, selectable: bool) -> Layer {
        Layer {
            candidate: WindowCandidate {
                window: WindowToken(order as _),
                process_id: 1,
                bounds: span(left, right),
                order,
            },
            coverage: vec![span(left, right)],
            selectable,
        }
    }

    #[test]
    fn annotation_clip_matches_frozen_composite_for_holes_and_negative_origin() {
        let bounds = Rect {
            left: -3,
            top: -2,
            right: 3,
            bottom: 2,
        };
        let original = CapturedRegion {
            bounds,
            pixels: [9, 27, 81, 255].repeat(24),
        };
        let mask = [
            Rect {
                right: -1,
                ..bounds
            },
            Rect { left: 1, ..bounds },
            Rect {
                bottom: -1,
                ..bounds
            },
            Rect { top: 1, ..bounds },
        ];
        let expected = compose(std::slice::from_ref(&original), &mask).unwrap();
        let mut actual = original;
        clip_pixels(&mut actual.pixels, bounds, &mask, &Default::default()).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.pixels.as_chunks::<4>().0.contains(&[0; 4]));
    }

    #[test]
    fn visible_layers_share_preview_and_output_mask_across_desktop_gap() {
        let mut selection = Selection {
            layers: vec![
                layer(1, -2, -1, false),
                layer(2, -1, 2, true),
                layer(3, -4, 5, true),
            ],
            ..Default::default()
        };
        let layout = [span(-4, 0), span(1, 5)];
        let frames = layout.map(|bounds| CapturedRegion {
            bounds,
            pixels: (bounds.left..bounds.right)
                .flat_map(|x| [(x + 10) as u8, 25, 73, 255])
                .collect(),
        });
        selection.toggle(Point { x: -2, y: 0 }, &layout).unwrap();
        assert!(selection.selected.is_empty());
        selection.toggle(Point { x: -3, y: 0 }, &layout).unwrap();
        assert_eq!(selection.mask, [span(-4, -2), span(2, 5)]);
        selection.toggle(Point { x: 1, y: 0 }, &layout).unwrap();
        let image = compose(&frames, &selection.mask).unwrap();
        assert_eq!(image.bounds, span(-4, 5));
        let expected: Vec<_> = (-4..5)
            .flat_map(|x| {
                if matches!(x, -2 | 0) {
                    [0; 4]
                } else {
                    [(x + 10) as u8, 25, 73, 255]
                }
            })
            .collect();
        assert_eq!(image.pixels, expected);
    }

    #[test]
    fn cancelled_save_preserves_selection_and_stale_success_cannot_close_retry() {
        use crate::{
            AppEvent, AppState,
            domain::screenshot::{ScreenshotEvent, ScreenshotPhase},
            ports::annotation_save::SaveOutcome,
        };
        let mut state = AppState::default();
        state.screenshot.session = 1;
        state.screenshot.phase = ScreenshotPhase::Selecting;
        state.screenshot.layout = vec![span(0, 8)];
        state.screenshot.windows = Some(Selection {
            layers: vec![layer(0, 1, 7, true)],
            ..Default::default()
        });
        let run = |s: &mut AppState, e| s.reduce(AppEvent::Screenshot(e));
        run(
            &mut state,
            ScreenshotEvent::ToggleWindow(1, Point { x: 3, y: 0 }),
        );
        let last = state.screenshot.last_region.clone();
        assert_eq!(
            run(
                &mut state,
                ScreenshotEvent::OutputMask(
                    1,
                    crate::domain::screenshot::MaskTarget::Save {
                        owner: None,
                        title: String::new()
                    }
                )
            )
            .len(),
            1
        );
        assert!(
            run(
                &mut state,
                ScreenshotEvent::OutputMask(
                    1,
                    crate::domain::screenshot::MaskTarget::Save {
                        owner: None,
                        title: String::new()
                    }
                )
            )
            .is_empty()
        );
        run(
            &mut state,
            ScreenshotEvent::MaskedOutputFinished(1, 1, Ok(SaveOutcome::Cancelled)),
        );
        assert_eq!(
            state.screenshot.windows.as_ref().unwrap().mask,
            [span(1, 7)]
        );
        run(
            &mut state,
            ScreenshotEvent::OutputMask(
                1,
                crate::domain::screenshot::MaskTarget::Save {
                    owner: None,
                    title: String::new(),
                },
            ),
        );
        run(
            &mut state,
            ScreenshotEvent::MaskedOutputFinished(1, 1, Ok(SaveOutcome::Saved)),
        );
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Output);
        run(
            &mut state,
            ScreenshotEvent::MaskedOutputFinished(1, 2, Ok(SaveOutcome::Saved)),
        );
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Idle);
        assert_eq!(state.screenshot.last_region, last);
    }

    #[test]
    fn only_disjoint_reordering_preserves_frozen_visibility() {
        let a = layer(1, 0, 4, true);
        let b = layer(2, 5, 8, true);
        let overlap = layer(3, 3, 7, false);
        assert!(stable(&[a.clone(), b.clone()], &[b, a.clone()]));
        assert!(!stable(&[a.clone(), overlap.clone()], &[overlap, a]));
    }

    #[test]
    fn rejects_empty_or_excessive_output_before_allocating() {
        assert!(compose(&[], &[]).is_err());
        assert!(
            compose(
                &[],
                &[Rect {
                    bottom: 40_000,
                    ..span(0, 40_000)
                }]
            )
            .is_err()
        );
    }
}
