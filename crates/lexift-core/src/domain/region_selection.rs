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

/// Mutable crop rectangle. Content remains in screen coordinates when the crop changes.
#[derive(Default)]
pub struct RegionEditor {
    bounds: Option<Rect>,
    drag: Option<(Rect, Point, Option<usize>)>,
}
impl RegionEditor {
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds: Some(bounds),
            drag: None,
        }
    }
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn begin(&mut self, point: Point, scale: f32, allow_body: bool) -> bool {
        let Some(b) = self.bounds else { return false };
        let geometry = super::annotation::Bounds {
            left: b.left as f32,
            top: b.top as f32,
            right: b.right as f32,
            bottom: b.bottom as f32,
        };
        let handle = geometry.handles().iter().position(|&(x, y)| {
            (x - point.x as f32).abs() <= 9. * scale && (y - point.y as f32).abs() <= 9. * scale
        });
        let body = allow_body
            && point.x >= b.left
            && point.x < b.right
            && point.y >= b.top
            && point.y < b.bottom;
        if handle.is_some() || body {
            self.drag = Some((b, point, handle));
            true
        } else {
            false
        }
    }
    pub fn update(&mut self, point: Point, desktop: Rect) {
        let Some((b, start, handle)) = self.drag else {
            return;
        };
        let geometry = super::annotation::Bounds {
            left: b.left as f32,
            top: b.top as f32,
            right: b.right as f32,
            bottom: b.bottom as f32,
        };
        let changed = if let Some(h) = handle {
            geometry.resized(h, (point.x as f32, point.y as f32))
        } else {
            let dx = (point.x - start.x).clamp(desktop.left - b.left, desktop.right - b.right);
            let dy = (point.y - start.y).clamp(desktop.top - b.top, desktop.bottom - b.bottom);
            super::annotation::Bounds {
                left: (b.left + dx) as f32,
                top: (b.top + dy) as f32,
                right: (b.right + dx) as f32,
                bottom: (b.bottom + dy) as f32,
            }
        };
        self.bounds = Some(Rect {
            left: (changed.left.round() as i32).max(desktop.left),
            top: (changed.top.round() as i32).max(desktop.top),
            right: (changed.right.round() as i32).min(desktop.right),
            bottom: (changed.bottom.round() as i32).min(desktop.bottom),
        });
    }
    pub fn finish(&mut self) {
        self.drag = None;
    }
    pub fn cancel(&mut self) -> bool {
        if let Some((b, _, _)) = self.drag.take() {
            self.bounds = Some(b);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test]
    fn crop_move_resize_all_handles_cancel_and_limits() {
        let original = Rect {
            left: -50,
            top: -30,
            right: 50,
            bottom: 30,
        };
        let desktop = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        for handle in 0..8 {
            let geometry = super::super::annotation::Bounds {
                left: -50.,
                top: -30.,
                right: 50.,
                bottom: 30.,
            };
            let (x, y) = geometry.handles()[handle];
            let mut edit = RegionEditor::new(original);
            assert!(edit.begin(
                Point {
                    x: x as i32,
                    y: y as i32
                },
                1.25,
                false
            ));
            edit.update(
                Point {
                    x: (x + 15.) as i32,
                    y: (y + 10.) as i32,
                },
                desktop,
            );
            assert_ne!(edit.bounds(), Some(original));
            assert!(edit.cancel());
            assert_eq!(edit.bounds(), Some(original));
        }
        let mut edit = RegionEditor::new(original);
        assert!(edit.begin(Point { x: 0, y: 0 }, 1., true));
        edit.update(Point { x: 500, y: 500 }, desktop);
        edit.finish();
        assert_eq!(
            edit.bounds(),
            Some(Rect {
                left: 0,
                top: 40,
                right: 100,
                bottom: 100
            })
        );
        assert!(!edit.cancel());
    }
}
