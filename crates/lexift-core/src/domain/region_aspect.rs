//! Integer aspect constraints shared by crop gestures and numeric drafts.
use super::{Point, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeAxis {
    #[default]
    Width,
    Height,
}

/// Immutable reduced ratio; rounded output dimensions never replace this source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AspectRatio {
    width: u64,
    height: u64,
}
impl AspectRatio {
    pub fn new(width: u64, height: u64) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let (mut a, mut b) = (width, height);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self {
            width: width / a,
            height: height / a,
        }
    }

    /// Link a draft without desktop clipping. Only the derived dimension saturates.
    pub fn dimensions(self, axis: SizeAxis, value: u64) -> (u64, u64) {
        let value = value.max(1);
        match axis {
            SizeAxis::Width => (value, rounded(value, self.height, self.width)),
            SizeAxis::Height => (rounded(value, self.width, self.height), value),
        }
    }

    /// Clamp a common rational scale before rounding either edge, preserving the anchor.
    pub(super) fn fit(self, axis: SizeAxis, value: u64, max_w: u64, max_h: u64) -> (u64, u64) {
        let mut scale = (
            value.max(1),
            match axis {
                SizeAxis::Width => self.width,
                SizeAxis::Height => self.height,
            },
        );
        for limit in [(max_w.max(1), self.width), (max_h.max(1), self.height)] {
            if u128::from(limit.0) * u128::from(scale.1) < u128::from(scale.0) * u128::from(limit.1)
            {
                scale = limit;
            }
        }
        (
            rounded(scale.0, self.width, scale.1),
            rounded(scale.0, self.height, scale.1),
        )
    }

    pub(super) fn dragged(
        self,
        b: Rect,
        start: Point,
        point: Point,
        handle: usize,
        desktop: Rect,
    ) -> Rect {
        if point == start {
            return b;
        }
        let left = matches!(handle, 0 | 3 | 5);
        let top = matches!(handle, 0..=2);
        let width = i64::from(b.right) - i64::from(b.left);
        let height = i64::from(b.bottom) - i64::from(b.top);
        let dx = (i64::from(point.x) - i64::from(start.x)) * if left { -1 } else { 1 };
        let dy = (i64::from(point.y) - i64::from(start.y)) * if top { -1 } else { 1 };
        let axis = match handle {
            1 | 6 => SizeAxis::Height,
            3 | 4 => SizeAxis::Width,
            _ if i128::from(dx.abs()) * i128::from(height)
                >= i128::from(dy.abs()) * i128::from(width) =>
            {
                SizeAxis::Width
            }
            _ => SizeAxis::Height,
        };
        // A side handle ignores perpendicular motion, even after pixel rounding.
        if match axis {
            SizeAxis::Width => dx == 0,
            SizeAxis::Height => dy == 0,
        } {
            return b;
        }
        let value = match axis {
            SizeAxis::Width => width + dx,
            SizeAxis::Height => height + dy,
        }
        .max(1) as u64;
        let x = if left { b.right } else { b.left };
        let y = if top { b.bottom } else { b.top };
        let max_w = if left {
            i64::from(x) - i64::from(desktop.left)
        } else {
            i64::from(desktop.right) - i64::from(x)
        };
        let max_h = if top {
            i64::from(y) - i64::from(desktop.top)
        } else {
            i64::from(desktop.bottom) - i64::from(y)
        };
        let (w, h) = self.fit(axis, value, max_w.max(1) as u64, max_h.max(1) as u64);
        let (x, y, w, h) = (i64::from(x), i64::from(y), w as i64, h as i64);
        Rect {
            left: (if left { x - w } else { x }) as i32,
            right: (if left { x } else { x + w }) as i32,
            top: (if top { y - h } else { y }) as i32,
            bottom: (if top { y } else { y + h }) as i32,
        }
    }
}

fn rounded(value: u64, numerator: u64, denominator: u64) -> u64 {
    ((u128::from(value) * u128::from(numerator) + u128::from(denominator / 2))
        / u128::from(denominator))
    .clamp(1, u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::super::{RegionEditor, RegionNudge};
    use super::*;
    const DESKTOP: Rect = Rect {
        left: -1000,
        top: -1000,
        right: 1000,
        bottom: 1000,
    };
    const ORIGINAL: Rect = Rect {
        left: -100,
        top: -60,
        right: 100,
        bottom: 60,
    };
    fn editor() -> RegionEditor {
        let mut e = RegionEditor::new(ORIGINAL);
        assert!(!e.apply_size(
            200,
            120,
            Some(AspectRatio::new(5, 3)),
            SizeAxis::Width,
            DESKTOP
        ));
        e
    }
    #[test]
    fn all_handles_follow_confirmed_anchors_and_cancel_at_both_dpis() {
        let points = [
            (-100, -60),
            (0, -60),
            (100, -60),
            (-100, 0),
            (100, 0),
            (-100, 60),
            (0, 60),
            (100, 60),
        ];
        for scale in [1., 1.25] {
            for (handle, (x, y)) in points.into_iter().enumerate() {
                let mut e = editor();
                let start = Point { x, y };
                assert!(e.begin(start, scale, true));
                e.update(start, DESKTOP);
                assert_eq!(e.bounds(), Some(ORIGINAL));
                let dx = if matches!(handle, 1 | 6) {
                    0
                } else if matches!(handle, 0 | 3 | 5) {
                    -40
                } else {
                    40
                };
                let dy = if matches!(handle, 3 | 4) {
                    0
                } else if handle <= 2 {
                    -24
                } else {
                    24
                };
                let end = Point {
                    x: x + dx,
                    y: y + dy,
                };
                e.update(end, DESKTOP);
                let b = e.bounds().unwrap();
                assert_eq!(
                    (b.right - b.left, b.bottom - b.top),
                    (240, 144),
                    "handle {handle}"
                );
                assert_eq!(
                    if matches!(handle, 0 | 3 | 5) {
                        b.right
                    } else {
                        b.left
                    },
                    if matches!(handle, 0 | 3 | 5) {
                        ORIGINAL.right
                    } else {
                        ORIGINAL.left
                    }
                );
                assert_eq!(
                    if handle <= 2 { b.bottom } else { b.top },
                    if handle <= 2 {
                        ORIGINAL.bottom
                    } else {
                        ORIGINAL.top
                    }
                );
                e.update(
                    Point {
                        x: x + dx / 2,
                        y: y + dy / 2,
                    },
                    DESKTOP,
                );
                e.update(end, DESKTOP);
                assert_eq!(e.bounds(), Some(b));
                assert!(e.cancel());
                assert_eq!(e.bounds(), Some(ORIGINAL));
                assert_eq!(e.aspect(), Some(AspectRatio::new(5, 3)));
            }
        }
    }
    #[test]
    fn fit_limits_both_dimensions_without_changing_the_ratio_source() {
        let ratio = AspectRatio::new(1920, 1080);
        assert_eq!(ratio, AspectRatio::new(16, 9));
        assert_eq!(ratio.fit(SizeAxis::Width, 1000, 500, 200), (356, 200));
        assert_eq!(ratio.fit(SizeAxis::Height, 1000, 100, 500), (100, 56));
        assert_eq!(
            AspectRatio::new(1, 2).dimensions(SizeAxis::Height, 3),
            (2, 3)
        );
        for ratio in [
            AspectRatio::new(1, u64::MAX),
            AspectRatio::new(u64::MAX, 1),
            AspectRatio::new(u64::MAX, u64::MAX - 1),
        ] {
            for axis in [SizeAxis::Width, SizeAxis::Height] {
                for v in [0, 1, 10, u64::MAX] {
                    let (w, h) = ratio.fit(axis, v, 101, 57);
                    assert!((1..=101).contains(&w) && (1..=57).contains(&h));
                    let linked = ratio.dimensions(axis, v);
                    assert!(linked.0 > 0 && linked.1 > 0);
                }
            }
        }
    }
    #[test]
    fn keyboard_and_numeric_share_limits_cancel_and_no_drift() {
        for (dw, dh, axis) in [
            (1, 0, SizeAxis::Width),
            (-1, 0, SizeAxis::Width),
            (0, 10, SizeAxis::Height),
            (0, -10, SizeAxis::Height),
        ] {
            let mut keyboard = editor();
            assert!(keyboard.begin_keyboard());
            assert!(keyboard.nudge(RegionNudge::Resize { dw, dh }, DESKTOP));
            let mut numeric = editor();
            numeric.apply_size(
                (200 + dw) as u64,
                (120 + dh) as u64,
                Some(AspectRatio::new(5, 3)),
                axis,
                DESKTOP,
            );
            assert_eq!(keyboard.bounds(), numeric.bounds());
            assert!(keyboard.cancel_keyboard());
            assert_eq!(keyboard.bounds(), Some(ORIGINAL));
        }
        let mut e = editor();
        e.begin_keyboard();
        for _ in 0..400 {
            e.nudge(RegionNudge::Resize { dw: 1, dh: 0 }, DESKTOP);
            e.nudge(RegionNudge::Resize { dw: -1, dh: 0 }, DESKTOP);
        }
        assert_eq!(e.bounds(), Some(ORIGINAL));
        e.nudge(
            RegionNudge::Move {
                dx: 2000,
                dy: -2000,
            },
            DESKTOP,
        );
        let b = e.bounds().unwrap();
        assert_eq!((b.right - b.left, b.bottom - b.top), (200, 120));
        assert_eq!(e.aspect(), Some(AspectRatio::new(5, 3)));
        e.cancel_keyboard();
        assert!(!e.apply_size(200, 120, None, SizeAxis::Width, DESKTOP));
        assert_eq!(e.aspect(), None);
    }
    #[test]
    fn pointer_crossing_clamps_without_flip_and_boundary_is_joint() {
        for (start, end) in [
            (Point { x: 100, y: 0 }, Point { x: i32::MAX, y: 0 }),
            (Point { x: -100, y: 0 }, Point { x: i32::MIN, y: 0 }),
            (Point { x: 0, y: -60 }, Point { x: 0, y: i32::MAX }),
        ] {
            let mut e = editor();
            assert!(e.begin(start, 1., true));
            e.update(end, DESKTOP);
            let b = e.bounds().unwrap();
            assert!(
                b.left >= DESKTOP.left
                    && b.top >= DESKTOP.top
                    && b.right <= DESKTOP.right
                    && b.bottom <= DESKTOP.bottom
            );
            assert!(b.right > b.left && b.bottom > b.top);
            let w = b.right - b.left;
            let h = b.bottom - b.top;
            assert!((h as f64 - w as f64 * 3. / 5.).abs() <= 1.);
        }
        let mut e = editor();
        e.begin_keyboard();
        assert!(!e.apply_size(10, 10, None, SizeAxis::Width, DESKTOP));
        assert_eq!(e.aspect(), Some(AspectRatio::new(5, 3)));
        e.cancel_keyboard();
        let r = AspectRatio::new(1, 100);
        e.apply_size(1, 150, Some(r), SizeAxis::Height, DESKTOP);
        let b = e.bounds().unwrap();
        assert!(!e.apply_size(
            (b.right - b.left) as u64,
            (b.bottom - b.top) as u64,
            Some(r),
            SizeAxis::Width,
            DESKTOP
        ));
        assert_eq!(e.bounds(), Some(b));
        let p = Point {
            x: b.right,
            y: (b.top + b.bottom) / 2,
        };
        assert!(e.begin(p, 1., false));
        e.update(Point { x: p.x, y: p.y + 5 }, DESKTOP);
        assert_eq!(e.bounds(), Some(b));
    }
}
