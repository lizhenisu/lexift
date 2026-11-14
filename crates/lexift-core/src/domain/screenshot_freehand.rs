use super::{
    geometry::{Point, Rect},
    screenshot_polygon::{Polygon, raster, side},
};
use crate::{Error, Result};
use std::sync::Arc;

const MAX_POINTS: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stroke {
    pub points: Vec<Point>,
    overflow: bool,
}

impl Stroke {
    pub fn push(&mut self, p: Point) {
        if self.overflow || self.points.last() == Some(&p) {
            return;
        }
        if let [.., a, b] = self.points.as_slice()
            && side(*a, *b, p) == 0
            && (a.x.min(p.x)..=a.x.max(p.x)).contains(&b.x)
            && (a.y.min(p.y)..=a.y.max(p.y)).contains(&b.y)
        {
            self.points.pop();
        }
        if self.points.len() == MAX_POINTS {
            self.overflow = true;
        } else {
            self.points.push(p);
        }
    }

    pub fn finish(mut self, displays: &[Rect]) -> Result<Polygon> {
        if self.overflow {
            return Err(Error::new("Polygon selection is too complex"));
        }
        if self.points.len() > 1 && self.points.first() == self.points.last() {
            self.points.pop();
        }
        if self.points.len() < 3 {
            return Err(Error::new("Polygon contains no screen pixels"));
        }
        let mask = raster(&self.points, displays)?;
        Ok(Polygon {
            points: self.points,
            mask: Some(Arc::new(mask)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[(i32, i32)]) -> Stroke {
        let mut stroke = Stroke::default();
        for &(x, y) in points {
            stroke.push(Point { x, y });
        }
        stroke
    }

    const DISPLAY: Rect = Rect {
        left: -10,
        top: -10,
        right: 10,
        bottom: 10,
    };

    #[test]
    fn odd_even_fills_both_lobes_and_cancels_double_wrap() {
        let bow = stroke(&[(0, 0), (4, 4), (0, 4), (4, 0)])
            .finish(&[DISPLAY])
            .unwrap();
        let rows: Vec<_> = bow
            .mask
            .unwrap()
            .iter()
            .map(|b| (b.top, b.left, b.right))
            .collect();
        assert_eq!(rows, [(0, 0, 3), (1, 1, 2), (2, 1, 2), (3, 0, 3)]);
        assert!(
            stroke(&[
                (0, 0),
                (4, 0),
                (4, 4),
                (0, 4),
                (0, 0),
                (4, 0),
                (4, 4),
                (0, 4)
            ])
            .finish(&[DISPLAY])
            .is_err()
        );
    }

    #[test]
    fn sampling_preserves_reversal_and_bounds_retained_points() {
        let sample = stroke(&[(0, 0), (1, 0), (2, 0), (2, 0), (1, 0), (1, 1)]);
        assert_eq!(
            sample.points,
            [(0, 0), (2, 0), (1, 0), (1, 1)].map(|(x, y)| Point { x, y })
        );
        assert!(stroke(&[(0, 0), (2, 0)]).finish(&[DISPLAY]).is_err());
        let mut oversized = Stroke::default();
        for i in 0..=MAX_POINTS {
            oversized.push(Point {
                x: i as i32,
                y: (i % 2) as i32,
            });
        }
        assert_eq!(oversized.points.len(), MAX_POINTS);
        assert!(oversized.finish(&[DISPLAY]).is_err());
    }

    #[test]
    fn invalid_redraw_preserves_confirmed_mask_and_stale_save_is_ignored() {
        use crate::{
            AppEvent, AppState,
            domain::screenshot::{ScreenshotEvent, ScreenshotPhase},
            ports::annotation_save::SaveOutcome,
        };
        let mut state = AppState::default();
        state.screenshot.session = 1;
        state.screenshot.phase = ScreenshotPhase::Selecting;
        state.screenshot.freehand = true;
        state.screenshot.layout = vec![DISPLAY];
        let run = |s: &mut AppState, e| s.reduce(AppEvent::Screenshot(e));
        run(
            &mut state,
            ScreenshotEvent::FreehandStroke(1, stroke(&[(-2, -2), (2, -2), (2, 2), (-2, 2)])),
        );
        let confirmed = state.screenshot.polygon.clone();
        assert!(confirmed.is_some());
        run(
            &mut state,
            ScreenshotEvent::FreehandStroke(1, stroke(&[(0, 0)])),
        );
        assert_eq!(state.screenshot.polygon, confirmed);
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
            ScreenshotEvent::MaskedOutputFinished(1, 1, Ok(SaveOutcome::Cancelled)),
        );
        assert_eq!(state.screenshot.polygon, confirmed);
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
    }
}
