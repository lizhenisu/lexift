//! Non-destructive, source-pixel crop geometry. Drag snapshots never own image buffers.
use super::geometry::Rect;

#[derive(Clone, Debug)]
pub struct Crop {
    pub bounds: Rect,
    image: Rect,
    drag: Option<Drag>,
}
#[derive(Clone, Debug)]
struct Drag {
    before: Rect,
    start: [f64; 2],
    axes: Option<[i8; 2]>, // None creates, [0, 0] moves, otherwise resizes.
}
impl Crop {
    pub fn new(image: Rect, bounds: Option<Rect>) -> Self {
        Self {
            bounds: bounds.unwrap_or(image),
            image,
            drag: None,
        }
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn hit(&self, p: [f64; 2], tolerance: f64) -> Option<[i8; 2]> {
        let b = self.bounds;
        if p[0] < f64::from(b.left) - tolerance
            || p[0] > f64::from(b.right) + tolerance
            || p[1] < f64::from(b.top) - tolerance
            || p[1] > f64::from(b.bottom) + tolerance
        {
            return None;
        }
        let axis = |v: f64, lo: i32, hi: i32| {
            if (v - f64::from(lo)).abs() <= tolerance {
                -1
            } else if (v - f64::from(hi)).abs() <= tolerance {
                1
            } else {
                0
            }
        };
        Some([axis(p[0], b.left, b.right), axis(p[1], b.top, b.bottom)])
    }
    pub fn begin(&mut self, p: [f64; 2], tolerance: f64) {
        if p.iter().any(|x| !x.is_finite()) {
            return;
        }
        self.drag = Some(Drag {
            before: self.bounds,
            start: p,
            axes: self
                .hit(p, tolerance)
                .filter(|axes| self.bounds != self.image || *axes != [0, 0]),
        });
    }
    pub fn update(&mut self, p: [f64; 2]) {
        let Some(d) = &self.drag else { return };
        if p.iter().any(|x| !x.is_finite()) {
            return;
        }
        let original = [
            [d.before.left, d.before.right],
            [d.before.top, d.before.bottom],
        ];
        let limits = [
            [self.image.left, self.image.right],
            [self.image.top, self.image.bottom],
        ];
        let mut result = original;
        for i in 0..2 {
            let [lo, hi] = limits[i];
            result[i] = match d.axes {
                None => {
                    let a = d.start[i]
                        .min(p[i])
                        .floor()
                        .clamp(f64::from(lo), f64::from(hi - 1)) as i32;
                    let b = d.start[i]
                        .max(p[i])
                        .ceil()
                        .clamp(f64::from(a + 1), f64::from(hi)) as i32;
                    [a, b]
                }
                Some([0, 0]) => {
                    let width = original[i][1] - original[i][0];
                    let a = (f64::from(original[i][0]) + (p[i] - d.start[i]).round())
                        .clamp(f64::from(lo), f64::from(hi - width))
                        as i32;
                    [a, a + width]
                }
                Some(axes) => match axes[i] {
                    -1 => [
                        (f64::from(original[i][0]) + p[i] - d.start[i])
                            .floor()
                            .clamp(f64::from(lo), f64::from(original[i][1] - 1))
                            as i32,
                        original[i][1],
                    ],
                    1 => [
                        original[i][0],
                        (f64::from(original[i][1]) + p[i] - d.start[i])
                            .ceil()
                            .clamp(f64::from(original[i][0] + 1), f64::from(hi))
                            as i32,
                    ],
                    _ => original[i],
                },
            };
        }
        self.bounds = Rect {
            left: result[0][0],
            right: result[0][1],
            top: result[1][0],
            bottom: result[1][1],
        };
    }
    pub fn finish(&mut self) {
        self.drag = None;
    }
    pub fn cancel_drag(&mut self) {
        if let Some(d) = self.drag.take() {
            self.bounds = d.before;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }
    #[test]
    fn crop_handle_hit_tolerance_does_not_jump_and_full_interior_starts_selection() {
        let full = rect(0, 0, 100, 100);
        let mut c = Crop::new(full, Some(rect(20, 20, 80, 80)));
        c.begin([22., 48.], 6.);
        c.update([22., 48.]);
        assert_eq!(c.bounds, rect(20, 20, 80, 80));
        c.update([32., 48.]);
        assert_eq!(c.bounds, rect(30, 20, 80, 80));
        c = Crop::new(full, None);
        c.begin([20.1, 20.1], 2.);
        c.update([80.1, 80.1]);
        assert_eq!(c.bounds, rect(20, 20, 81, 81));
    }
    #[test]
    fn crop_create_all_directions_round_outward_and_cancel() {
        for (a, b) in [
            ([10.2, 20.7], [30.4, 40.1]),
            ([30.4, 20.7], [10.2, 40.1]),
            ([10.2, 40.1], [30.4, 20.7]),
            ([30.4, 40.1], [10.2, 20.7]),
        ] {
            let mut c = Crop::new(rect(0, 0, 100, 100), Some(rect(60, 60, 90, 90)));
            c.begin(a, 1.);
            c.update(b);
            assert_eq!(c.bounds, rect(10, 20, 31, 41));
            c.cancel_drag();
            assert_eq!(c.bounds, rect(60, 60, 90, 90));
        }
    }
    #[test]
    fn crop_eight_handles_minimum_and_bounded_move() {
        let image = rect(0, 0, 100, 100);
        for p in [
            [20., 20.],
            [50., 20.],
            [80., 20.],
            [20., 50.],
            [80., 50.],
            [20., 80.],
            [50., 80.],
            [80., 80.],
        ] {
            let mut c = Crop::new(image, Some(rect(20, 20, 80, 80)));
            c.begin(p, 1.);
            c.update([-100., 200.]);
            c.finish();
            assert!(
                c.bounds.left >= 0
                    && c.bounds.top >= 0
                    && c.bounds.right <= 100
                    && c.bounds.bottom <= 100
            );
            assert!(c.bounds.right > c.bounds.left && c.bounds.bottom > c.bounds.top);
        }
        let mut c = Crop::new(image, Some(rect(20, 20, 80, 80)));
        c.begin([50., 50.], 1.);
        c.update([500., -500.]);
        assert_eq!(c.bounds, rect(40, 0, 100, 60));
    }
}
