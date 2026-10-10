//! Reusable physical-pixel rectangle gesture; independent of any window or OCR engine.
use super::geometry::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionContext {
    pub source_session: u64,
    pub request_id: u64,
    pub snapshot_id: u64,
}
impl SelectionContext {
    pub fn standalone(request_id: u64) -> Self {
        Self {
            source_session: request_id,
            request_id,
            snapshot_id: request_id,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionSelectionResult {
    pub context: SelectionContext,
    pub bounds: Rect,
}
#[derive(Default)]
pub struct RegionSelection {
    start: Option<Point>,
    current: Option<Point>,
}
impl RegionSelection {
    pub fn begin(&mut self, point: Point) {
        self.start = Some(point);
        self.current = Some(point);
    }
    pub fn update(&mut self, point: Point) {
        if self.start.is_some() {
            self.current = Some(point);
        }
    }
    pub fn cancel(&mut self) {
        self.start = None;
        self.current = None;
    }
    pub fn bounds(&self) -> Option<Rect> {
        let a = self.start?;
        let b = self.current?;
        Some(Rect {
            left: a.x.min(b.x),
            top: a.y.min(b.y),
            right: a.x.max(b.x),
            bottom: a.y.max(b.y),
        })
    }
    pub fn preview(&self, display: Rect) -> Option<Rect> {
        let b = self.bounds()?;
        Some(Rect {
            left: b.left.clamp(display.left, display.right),
            top: b.top.clamp(display.top, display.bottom),
            right: b.right.clamp(display.left, display.right),
            bottom: b.bottom.clamp(display.top, display.bottom),
        })
    }
    pub fn finish(
        &mut self,
        point: Point,
        context: SelectionContext,
    ) -> Option<RegionSelectionResult> {
        self.update(point);
        let bounds = self.bounds();
        self.cancel();
        bounds
            .filter(|b| b.right > b.left && b.bottom > b.top)
            .map(|bounds| RegionSelectionResult { context, bounds })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_directions_cross_negative_origin_and_clip_each_monitor() {
        let context = SelectionContext {
            source_session: 11,
            request_id: 12,
            snapshot_id: 13,
        };
        for (a, b) in [
            (Point { x: -30, y: -20 }, Point { x: 70, y: 80 }),
            (Point { x: 70, y: -20 }, Point { x: -30, y: 80 }),
            (Point { x: -30, y: 80 }, Point { x: 70, y: -20 }),
            (Point { x: 70, y: 80 }, Point { x: -30, y: -20 }),
        ] {
            let mut g = RegionSelection::default();
            g.begin(a);
            g.update(b);
            assert_eq!(
                g.preview(Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100
                }),
                Some(Rect {
                    left: 0,
                    top: 0,
                    right: 70,
                    bottom: 80
                })
            );
            assert_eq!(
                g.finish(b, context),
                Some(RegionSelectionResult {
                    context,
                    bounds: Rect {
                        left: -30,
                        top: -20,
                        right: 70,
                        bottom: 80
                    }
                })
            );
            assert_eq!(g.bounds(), None);
        }
    }
    #[test]
    fn zero_area_or_cancel_does_not_submit_and_next_drag_still_works() {
        let c = SelectionContext::standalone(1);
        let mut g = RegionSelection::default();
        g.begin(Point { x: 2, y: 3 });
        assert!(g.finish(Point { x: 2, y: 9 }, c).is_none());
        g.begin(Point { x: 2, y: 3 });
        g.cancel();
        assert!(g.finish(Point { x: 9, y: 9 }, c).is_none());
        g.begin(Point { x: 2, y: 3 });
        assert!(g.finish(Point { x: 9, y: 9 }, c).is_some());
    }
}
