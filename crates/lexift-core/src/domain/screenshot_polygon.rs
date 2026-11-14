use super::geometry::{Point, Rect};
use crate::{Error, Result};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polygon {
    pub points: Vec<Point>,
    pub mask: Option<Arc<Vec<Rect>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Add(Point),
    Close,
    Back,
}

pub(super) fn side(a: Point, b: Point, c: Point) -> i128 {
    (i128::from(b.x) - i128::from(a.x)) * (i128::from(c.y) - i128::from(a.y))
        - (i128::from(b.y) - i128::from(a.y)) * (i128::from(c.x) - i128::from(a.x))
}

fn on(a: Point, b: Point, p: Point) -> bool {
    side(a, b, p) == 0
        && (a.x.min(b.x)..=a.x.max(b.x)).contains(&p.x)
        && (a.y.min(b.y)..=a.y.max(b.y)).contains(&p.y)
}

fn intersects(a: Point, b: Point, c: Point, d: Point) -> bool {
    on(a, b, c)
        || on(a, b, d)
        || on(c, d, a)
        || on(c, d, b)
        || (side(a, b, c).signum() * side(a, b, d).signum() < 0
            && side(c, d, a).signum() * side(c, d, b).signum() < 0)
}

fn validate_edge(a: Point, b: Point, c: Point) -> Result<()> {
    if on(a, b, c) || on(b, c, a) {
        return Err(Error::new("Polygon edges overlap"));
    }
    Ok(())
}

impl Polygon {
    pub fn edit(&mut self, edit: Edit, displays: &[Rect]) -> Result<()> {
        match edit {
            Edit::Back => {
                if self.mask.take().is_none() {
                    self.points.pop();
                }
            }
            Edit::Add(p) if self.mask.is_none() => {
                if self.points.contains(&p) || self.points.len() >= 512 {
                    return Err(Error::new("Invalid polygon vertex"));
                }
                if let Some(&last) = self.points.last() {
                    if let Some(&prev) = self.points.iter().rev().nth(1) {
                        validate_edge(prev, last, p)?;
                    }
                    if self
                        .points
                        .windows(2)
                        .rev()
                        .skip(1)
                        .any(|e| intersects(e[0], e[1], last, p))
                    {
                        return Err(Error::new("Polygon edges intersect"));
                    }
                }
                self.points.push(p);
            }
            Edit::Close if self.mask.is_none() && self.points.len() >= 3 => {
                let p = &self.points;
                let last = p[p.len() - 1];
                validate_edge(p[p.len() - 2], last, p[0])?;
                validate_edge(last, p[0], p[1])?;
                if p[1..p.len() - 1]
                    .windows(2)
                    .any(|e| intersects(e[0], e[1], last, p[0]))
                {
                    return Err(Error::new("Polygon edges intersect"));
                }
                self.mask = Some(Arc::new(raster(p, displays)?));
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn bounds(mask: &[Rect]) -> Option<Rect> {
    mask.iter().copied().reduce(|a, b| Rect {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    })
}

/// The left crossing is inclusive and the right crossing is exclusive at pixel centers.
pub(super) fn raster(points: &[Point], displays: &[Rect]) -> Result<Vec<Rect>> {
    let min_y = points.iter().map(|p| p.y).min().unwrap();
    let max_y = points.iter().map(|p| p.y).max().unwrap();
    let edges: Vec<_> = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .filter(|(a, b)| a.y != b.y)
        .map(|(&a, &b)| if a.y < b.y { (a, b) } else { (b, a) })
        .collect();
    let mut mask = Vec::new();
    let mut crossings = Vec::with_capacity(edges.len());
    for display in displays {
        for y in min_y.max(display.top)..max_y.min(display.bottom) {
            crossings.clear();
            for &(a, b) in &edges {
                if y < a.y || y >= b.y {
                    continue;
                }
                let dy = i128::from(b.y) - i128::from(a.y);
                let dx = i128::from(b.x) - i128::from(a.x);
                let n = 2 * i128::from(a.x) * dy + (2 * (i128::from(y) - i128::from(a.y)) + 1) * dx;
                // First column whose center is at or to the right of the crossing.
                crossings.push(((n + dy - 1).div_euclid(2 * dy)) as i32);
            }
            crossings.sort_unstable();
            for span in crossings.as_chunks::<2>().0 {
                let left = span[0].max(display.left);
                let right = span[1].min(display.right);
                if left < right {
                    mask.push(Rect {
                        left,
                        right,
                        top: y,
                        bottom: y + 1,
                    });
                }
            }
            if mask.len() > 262_144 {
                return Err(Error::new("Polygon selection is too complex"));
            }
        }
    }
    let b = bounds(&mask).ok_or_else(|| Error::new("Polygon contains no screen pixels"))?;
    let width = i64::from(b.right) - i64::from(b.left);
    let height = i64::from(b.bottom) - i64::from(b.top);
    if width.checked_mul(height).is_none_or(|n| n > 40_000_000) {
        return Err(Error::new("Polygon selection is too large"));
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::screen_capture::CapturedRegion;

    fn p(x: i32, y: i32) -> Point {
        Point { x, y }
    }
    fn r(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn concave_mask_preserves_negative_coordinates_and_transparent_gap() {
        let displays = [r(-3, 0, -1, 4), r(0, 0, 4, 4)];
        let mut polygon = Polygon::default();
        for point in [p(-3, 0), p(4, 0), p(4, 2), p(1, 2), p(1, 4), p(-3, 4)] {
            polygon.edit(Edit::Add(point), &displays).unwrap();
        }
        polygon.edit(Edit::Close, &displays).unwrap();
        let frames = displays.map(|bounds| CapturedRegion {
            bounds,
            pixels: (bounds.top..bounds.bottom)
                .flat_map(|y| {
                    (bounds.left..bounds.right)
                        .flat_map(move |x| [(x + 20) as u8, y as u8, 71, 255])
                })
                .collect(),
        });
        let result =
            super::super::screenshot_windows::compose(&frames, polygon.mask.as_ref().unwrap())
                .unwrap();
        let expected: Vec<_> = (0..4)
            .flat_map(|y| {
                (-3..4).flat_map(move |x| {
                    if x == -1 || (y >= 2 && x >= 1) {
                        [0; 4]
                    } else {
                        [(x + 20) as u8, y as u8, 71, 255]
                    }
                })
            })
            .collect();
        assert_eq!(result.bounds, r(-3, 0, 4, 4));
        assert_eq!(result.pixels, expected);
        polygon.edit(Edit::Back, &displays).unwrap();
        assert_eq!(polygon.points.len(), 6);
        assert!(polygon.mask.is_none());
        polygon.edit(Edit::Back, &displays).unwrap();
        assert_eq!(polygon.points.len(), 5);
    }

    #[test]
    fn save_retry_is_bound_to_current_operation_and_preserves_r() {
        use crate::{
            AppEvent, AppState,
            domain::screenshot::{LastRegion, ScreenshotEvent, ScreenshotPhase},
            ports::annotation_save::SaveOutcome,
        };
        let layout = vec![r(0, 0, 8, 8)];
        let mut state = AppState::default();
        state.screenshot.session = 1;
        state.screenshot.phase = ScreenshotPhase::Selecting;
        state.screenshot.layout = layout.clone();
        state.screenshot.polygon = Some(Polygon::default());
        let last = LastRegion {
            bounds: r(1, 1, 2, 2),
            layout,
        };
        state.screenshot.last_region = Some(last.clone());
        let run = |s: &mut AppState, e| s.reduce(AppEvent::Screenshot(e));
        for point in [p(0, 0), p(4, 0), p(0, 4)] {
            run(
                &mut state,
                ScreenshotEvent::PolygonEdit(1, Edit::Add(point)),
            );
        }
        run(&mut state, ScreenshotEvent::PolygonEdit(1, Edit::Close));
        let save = ScreenshotEvent::OutputMask(
            1,
            crate::domain::screenshot::MaskTarget::Save {
                owner: None,
                title: String::new(),
            },
        );
        assert_eq!(run(&mut state, save.clone()).len(), 1);
        assert!(run(&mut state, save.clone()).is_empty());
        run(
            &mut state,
            ScreenshotEvent::MaskedOutputFinished(1, 1, Err(Error::new("write failure"))),
        );
        assert!(state.screenshot.polygon.as_ref().unwrap().mask.is_some());
        assert_eq!(run(&mut state, save).len(), 1);
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
        assert_eq!(state.screenshot.last_region, Some(last));
    }

    #[test]
    fn diagonal_uses_half_open_pixel_centers_and_crossings_preserve_draft() {
        let display = [r(0, 0, 8, 8)];
        let mut polygon = Polygon::default();
        for point in [p(0, 0), p(4, 0), p(0, 4)] {
            polygon.edit(Edit::Add(point), &display).unwrap();
        }
        assert!(polygon.edit(Edit::Add(p(0, 0)), &display).is_err());
        assert!(polygon.edit(Edit::Add(p(2, 0)), &display).is_err());
        polygon.edit(Edit::Close, &display).unwrap();
        assert_eq!(
            polygon.mask.as_ref().unwrap().as_slice(),
            [r(0, 0, 3, 1), r(0, 1, 2, 2), r(0, 2, 1, 3)]
        );
    }
}
