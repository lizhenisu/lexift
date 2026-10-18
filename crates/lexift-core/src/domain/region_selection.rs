//! Reusable physical-pixel rectangle gesture; independent of any window or OCR engine.
use super::geometry::{Point, Rect};
#[path = "region_aspect.rs"]
mod aspect;
pub use aspect::{AspectRatio, SizeAxis};

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
    candidate: Option<Rect>,
    threshold: f64,
}
impl RegionSelection {
    pub fn begin(&mut self, point: Point) {
        self.candidate = None;
        self.start = Some(point);
        self.current = Some(point);
    }
    /// Small click jitter retains the pressed candidate; a drag never snaps back.
    pub fn begin_candidate(&mut self, point: Point, candidate: Option<Rect>, scale: f32) {
        self.begin(point);
        self.candidate = candidate;
        self.threshold = 4. * f64::from(scale.max(f32::EPSILON));
    }
    pub fn update(&mut self, point: Point) {
        if self.start.is_some() {
            if let Some(start) = self.start {
                let dx = (i64::from(point.x) - i64::from(start.x)).abs();
                let dy = (i64::from(point.y) - i64::from(start.y)).abs();
                if dx.max(dy) as f64 > self.threshold {
                    self.candidate = None;
                }
            }
            self.current = Some(point);
        }
    }
    pub fn cancel(&mut self) {
        self.start = None;
        self.current = None;
        self.candidate = None;
    }
    pub fn active(&self) -> bool {
        self.start.is_some()
    }
    pub fn bounds(&self) -> Option<Rect> {
        if let Some(bounds) = self.candidate {
            return Some(bounds);
        }
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
    fn window_click_jitter_and_drag_threshold_preserve_the_original_origin() {
        let window = Rect {
            left: -100,
            top: -50,
            right: 200,
            bottom: 150,
        };
        let start = Point { x: -20, y: 10 };
        let context = SelectionContext::standalone(7);
        for scale in [1., 1.25] {
            let threshold = (4. * scale) as i32;
            let mut g = RegionSelection::default();
            g.begin_candidate(start, Some(window), scale);
            assert_eq!(g.bounds(), Some(window));
            assert_eq!(
                g.finish(
                    Point {
                        x: start.x + threshold,
                        y: start.y
                    },
                    context
                )
                .unwrap()
                .bounds,
                window
            );
            assert!(!g.active());
            g.begin_candidate(start, Some(window), scale);
            g.update(Point {
                x: start.x + threshold + 1,
                y: start.y + 20,
            });
            g.update(Point {
                x: start.x + 1,
                y: start.y + 2,
            });
            assert_eq!(
                g.finish(
                    Point {
                        x: start.x + 1,
                        y: start.y + 2
                    },
                    context
                )
                .unwrap()
                .bounds,
                Rect {
                    left: -20,
                    top: 10,
                    right: -19,
                    bottom: 12
                }
            );
            g.begin_candidate(start, Some(window), scale);
            g.cancel();
            assert_eq!(g.finish(start, context), None);
            g.begin_candidate(start, None, scale);
            assert!(g.finish(Point { x: -19, y: 11 }, context).is_some());
        }
    }
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

/// Adjustment selected by the crop hit test, retained throughout the gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionAdjustment {
    Move,
    Resize(usize),
}

/// A physical-pixel crop operation. Resize keeps the top-left corner fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionNudge {
    Move { dx: i32, dy: i32 },
    Resize { dw: i32, dh: i32 },
}

/// Mutable crop rectangle. Content remains in screen coordinates when the crop changes.
#[derive(Default)]
pub struct RegionEditor {
    bounds: Option<Rect>,
    aspect: Option<AspectRatio>,
    drag: Option<(Rect, Point, Option<usize>)>,
    keyboard_origin: Option<Rect>,
}
impl RegionEditor {
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds: Some(bounds),
            aspect: None,
            drag: None,
            keyboard_origin: None,
        }
    }
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }
    pub fn aspect(&self) -> Option<AspectRatio> {
        self.aspect
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn adjustment(&self) -> Option<RegionAdjustment> {
        self.drag
            .map(|(_, _, handle)| handle.map_or(RegionAdjustment::Move, RegionAdjustment::Resize))
    }
    /// Shared hit contract for pointer handling and cursor feedback; handles take priority.
    pub fn hit(&self, point: Point, scale: f32, allow_body: bool) -> Option<RegionAdjustment> {
        let b = self.bounds?;
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
        handle
            .map(RegionAdjustment::Resize)
            .or(body.then_some(RegionAdjustment::Move))
    }
    pub fn begin(&mut self, point: Point, scale: f32, allow_body: bool) -> bool {
        if self.keyboard_origin.is_some() {
            return false;
        }
        let Some(hit) = self.hit(point, scale, allow_body) else {
            return false;
        };
        let handle = match hit {
            RegionAdjustment::Move => None,
            RegionAdjustment::Resize(h) => Some(h),
        };
        self.drag = Some((self.bounds.unwrap(), point, handle));
        true
    }
    pub fn update(&mut self, point: Point, desktop: Rect) {
        let Some((b, start, handle)) = self.drag else {
            return;
        };
        if handle.is_none() {
            self.bounds = Some(translated(
                b,
                point.x.saturating_sub(start.x),
                point.y.saturating_sub(start.y),
                desktop,
            ));
            return;
        }
        if let Some(aspect) = self.aspect {
            self.bounds = Some(aspect.dragged(b, start, point, handle.unwrap(), desktop));
            return;
        }
        let geometry = super::annotation::Bounds {
            left: b.left as f32,
            top: b.top as f32,
            right: b.right as f32,
            bottom: b.bottom as f32,
        };
        let changed = geometry.resized(handle.unwrap(), (point.x as f32, point.y as f32));
        self.bounds = Some(Rect {
            left: (changed.left.round() as i32).max(desktop.left),
            top: (changed.top.round() as i32).max(desktop.top),
            right: (changed.right.round() as i32).min(desktop.right),
            bottom: (changed.bottom.round() as i32).min(desktop.bottom),
        });
    }
    pub fn keyboard_adjusting(&self) -> bool {
        self.keyboard_origin.is_some()
    }
    pub fn begin_keyboard(&mut self) -> bool {
        if self.drag.is_some() || self.bounds.is_none() {
            return false;
        }
        if self.keyboard_origin.is_none() {
            self.keyboard_origin = self.bounds;
        }
        true
    }
    /// Apply one message without DPI scaling; return whether geometry actually changed.
    pub fn nudge(&mut self, action: RegionNudge, desktop: Rect) -> bool {
        if self.keyboard_origin.is_none() {
            return false;
        }
        let b = self.bounds.unwrap();
        let next = match action {
            RegionNudge::Move { dx, dy } => translated(b, dx, dy, desktop),
            RegionNudge::Resize { dw, dh } => self.resized(
                b,
                i64::from(b.right) - i64::from(b.left) + i64::from(dw),
                i64::from(b.bottom) - i64::from(b.top) + i64::from(dh),
                if dw != 0 {
                    SizeAxis::Width
                } else {
                    SizeAxis::Height
                },
                desktop,
            ),
        };
        self.bounds = Some(next);
        next != b
    }
    /// Set physical dimensions atomically, keeping the origin and clamping to the desktop.
    /// Active pointer/keyboard transactions must be finished before applying a draft.
    pub fn set_size(&mut self, width: u64, height: u64, desktop: Rect) -> bool {
        if self.drag.is_some() || self.keyboard_origin.is_some() {
            return false;
        }
        let Some(b) = self.bounds else { return false };
        let next = self.resized(
            b,
            width.min(i64::MAX as u64) as i64,
            height.min(i64::MAX as u64) as i64,
            SizeAxis::Width,
            desktop,
        );
        self.bounds = Some(next);
        next != b
    }
    /// Atomically apply a numeric draft and its constraint, outside active gestures.
    /// The return value reports geometry changes; constraint-only changes return false.
    pub fn apply_size(
        &mut self,
        width: u64,
        height: u64,
        aspect: Option<AspectRatio>,
        axis: SizeAxis,
        desktop: Rect,
    ) -> bool {
        if self.dragging() || self.keyboard_adjusting() || self.bounds.is_none() {
            return false;
        }
        let b = self.bounds.unwrap();
        if self.aspect == aspect
            && width == (i64::from(b.right) - i64::from(b.left)) as u64
            && height == (i64::from(b.bottom) - i64::from(b.top)) as u64
        {
            return false;
        }
        self.aspect = aspect;
        let next = self.resized(
            b,
            width.min(i64::MAX as u64) as i64,
            height.min(i64::MAX as u64) as i64,
            axis,
            desktop,
        );
        self.bounds = Some(next);
        next != b
    }
    fn resized(&self, b: Rect, width: i64, height: i64, axis: SizeAxis, desktop: Rect) -> Rect {
        let Some(aspect) = self.aspect else {
            return resized(b, width, height, desktop);
        };
        let (w, h) = aspect.fit(
            axis,
            match axis {
                SizeAxis::Width => width,
                SizeAxis::Height => height,
            }
            .max(1) as u64,
            (i64::from(desktop.right) - i64::from(b.left)).max(1) as u64,
            (i64::from(desktop.bottom) - i64::from(b.top)).max(1) as u64,
        );
        Rect {
            right: (i64::from(b.left) + w as i64) as i32,
            bottom: (i64::from(b.top) + h as i64) as i32,
            ..b
        }
    }
    pub fn finish_keyboard(&mut self) {
        self.keyboard_origin = None;
    }
    pub fn cancel_keyboard(&mut self) -> bool {
        if let Some(original) = self.keyboard_origin.take() {
            self.bounds = Some(original);
            true
        } else {
            false
        }
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

fn resized(b: Rect, width: i64, height: i64, desktop: Rect) -> Rect {
    Rect {
        right: (i64::from(b.left)
            + width.clamp(1, (i64::from(desktop.right) - i64::from(b.left)).max(1)))
            as i32,
        bottom: (i64::from(b.top)
            + height.clamp(1, (i64::from(desktop.bottom) - i64::from(b.top)).max(1)))
            as i32,
        ..b
    }
}

fn translated(b: Rect, dx: i32, dy: i32, desktop: Rect) -> Rect {
    let dx = dx.clamp(desktop.left - b.left, desktop.right - b.right);
    let dy = dy.clamp(desktop.top - b.top, desktop.bottom - b.bottom);
    Rect {
        left: b.left + dx,
        right: b.right + dx,
        top: b.top + dy,
        bottom: b.bottom + dy,
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test]
    fn absolute_size_shares_resize_limits_and_preserves_transactions() {
        let desktop = Rect {
            left: -1920,
            top: -200,
            right: 2560,
            bottom: 1440,
        };
        let original = Rect {
            left: -50,
            top: -10,
            right: 150,
            bottom: 90,
        };
        let mut e = RegionEditor::new(original);
        assert!(!e.set_size(200, 100, desktop));
        assert!(e.set_size(0, 0, desktop));
        assert_eq!(
            e.bounds(),
            Some(Rect {
                right: -49,
                bottom: -9,
                ..original
            })
        );
        assert!(e.set_size(u64::MAX, u64::MAX, desktop));
        assert_eq!(
            e.bounds(),
            Some(Rect {
                right: desktop.right,
                bottom: desktop.bottom,
                ..original
            })
        );
        assert!(!e.set_size(u64::MAX, u64::MAX, desktop));
        for (w, h) in [(1, 1), (201, 110), (999999, 999999)] {
            let mut absolute = RegionEditor::new(original);
            absolute.set_size(w, h, desktop);
            let mut relative = RegionEditor::new(original);
            relative.begin_keyboard();
            relative.nudge(
                RegionNudge::Resize {
                    dw: w as i32 - 200,
                    dh: h as i32 - 100,
                },
                desktop,
            );
            assert_eq!(absolute.bounds(), relative.bounds());
        }
        e = RegionEditor::new(original);
        e.begin_keyboard();
        assert!(!e.set_size(20, 30, desktop));
        e.cancel_keyboard();
        assert_eq!(e.bounds(), Some(original));
        assert!(e.begin(Point { x: 0, y: 0 }, 1., true));
        assert!(!e.set_size(20, 30, desktop));
        e.cancel();
        assert_eq!(e.bounds(), Some(original));
    }

    #[test]
    fn keyboard_physical_steps_limits_and_cancel_snapshot() {
        let desktop = Rect {
            left: -1920,
            top: -200,
            right: 2560,
            bottom: 1440,
        };
        let original = Rect {
            left: -30,
            top: 50,
            right: 170,
            bottom: 150,
        };
        for step in [1, 10] {
            for (dx, dy) in [(-step, 0), (step, 0), (0, -step), (0, step)] {
                let mut e = RegionEditor::new(original);
                assert!(!e.nudge(RegionNudge::Move { dx, dy }, desktop));
                assert!(e.begin_keyboard());
                assert!(e.nudge(RegionNudge::Move { dx, dy }, desktop));
                let b = e.bounds().unwrap();
                assert_eq!(
                    b,
                    Rect {
                        left: original.left + dx,
                        right: original.right + dx,
                        top: original.top + dy,
                        bottom: original.bottom + dy
                    }
                );
                assert!(e.begin_keyboard()); // repeats retain the first snapshot
                assert!(e.nudge(
                    RegionNudge::Resize {
                        dw: step,
                        dh: -step
                    },
                    desktop
                ));
                assert!(e.cancel_keyboard());
                assert_eq!(e.bounds(), Some(original));
                assert!(!e.cancel_keyboard());
            }
            for (dw, dh) in [(-step, 0), (step, 0), (0, -step), (0, step)] {
                let mut e = RegionEditor::new(original);
                e.begin_keyboard();
                assert!(e.nudge(RegionNudge::Resize { dw, dh }, desktop));
                assert_eq!(
                    e.bounds(),
                    Some(Rect {
                        right: original.right + dw,
                        bottom: original.bottom + dh,
                        ..original
                    })
                );
                e.finish_keyboard();
                assert!(!e.cancel_keyboard());
            }
        }
        let mut e = RegionEditor::new(original);
        e.begin_keyboard();
        e.nudge(
            RegionNudge::Move {
                dx: i32::MIN,
                dy: i32::MAX,
            },
            desktop,
        );
        let edge = e.bounds().unwrap();
        assert_eq!((edge.left, edge.bottom), (desktop.left, desktop.bottom));
        assert_eq!((edge.right - edge.left, edge.bottom - edge.top), (200, 100));
        assert!(!e.nudge(RegionNudge::Move { dx: -10, dy: 10 }, desktop));
        e.nudge(
            RegionNudge::Resize {
                dw: i32::MIN,
                dh: i32::MIN,
            },
            desktop,
        );
        let tiny = e.bounds().unwrap();
        assert_eq!((tiny.right - tiny.left, tiny.bottom - tiny.top), (1, 1));
        assert!(!e.nudge(RegionNudge::Resize { dw: -1, dh: -10 }, desktop));
        e.nudge(
            RegionNudge::Resize {
                dw: i32::MAX,
                dh: i32::MAX,
            },
            desktop,
        );
        assert_eq!(
            e.bounds(),
            Some(Rect {
                right: desktop.right,
                bottom: desktop.bottom,
                ..tiny
            })
        );
        assert!(!e.nudge(RegionNudge::Resize { dw: 10, dh: 10 }, desktop));
        e.cancel_keyboard();
        assert_eq!(e.bounds(), Some(original));
    }
    #[test]
    fn pointer_and_keyboard_transactions_are_exclusive() {
        let b = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let mut e = RegionEditor::new(b);
        assert!(e.begin(Point { x: 50, y: 50 }, 1., true));
        assert!(!e.begin_keyboard());
        e.finish();
        assert!(e.begin_keyboard());
        assert!(!e.begin(Point { x: 50, y: 50 }, 1., true));
        e.finish_keyboard();
        assert!(e.begin(Point { x: 50, y: 50 }, 1., true));
        assert!(!RegionEditor::default().begin_keyboard());
    }
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
            for scale in [1., 1.25] {
                assert_eq!(
                    edit.hit(
                        Point {
                            x: x as i32,
                            y: y as i32
                        },
                        scale,
                        true
                    ),
                    Some(RegionAdjustment::Resize(handle))
                );
            }
            assert!(edit.begin(
                Point {
                    x: x as i32,
                    y: y as i32
                },
                1.25,
                false
            ));
            assert_eq!(edit.adjustment(), Some(RegionAdjustment::Resize(handle)));
            edit.update(
                Point {
                    x: (x + 15.) as i32,
                    y: (y + 10.) as i32,
                },
                desktop,
            );
            assert_ne!(edit.bounds(), Some(original));
            assert_eq!(edit.adjustment(), Some(RegionAdjustment::Resize(handle)));
            assert!(edit.cancel());
            assert_eq!(edit.adjustment(), None);
            assert_eq!(edit.bounds(), Some(original));
        }
        let mut edit = RegionEditor::new(original);
        assert_eq!(edit.hit(Point { x: 0, y: 0 }, 1., false), None);
        assert_eq!(edit.hit(Point { x: 90, y: 90 }, 1., true), None);
        assert!(edit.begin(Point { x: 0, y: 0 }, 1., true));
        assert_eq!(edit.adjustment(), Some(RegionAdjustment::Move));
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
