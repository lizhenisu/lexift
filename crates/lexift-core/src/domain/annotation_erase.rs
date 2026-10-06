//! Per-target coverage removed by the eraser. Coordinates are physical desktop pixels.
use super::annotation::{Bounds, ObjectId};

#[derive(Clone, Debug, PartialEq)]
pub enum EraseRegion {
    Rectangle(Bounds),
    Brush {
        points: Vec<(f32, f32)>,
        diameter: f32,
    },
}
impl EraseRegion {
    pub fn contains(&self, p: (f32, f32)) -> bool {
        match self {
            Self::Rectangle(b) => {
                p.0 >= b.left && p.0 <= b.right && p.1 >= b.top && p.1 <= b.bottom
            }
            Self::Brush { points, diameter } => {
                points
                    .first()
                    .is_some_and(|q| (p.0 - q.0).hypot(p.1 - q.1) <= diameter / 2.)
                    || points.windows(2).any(|q| {
                        let (dx, dy) = (q[1].0 - q[0].0, q[1].1 - q[0].1);
                        let t = (((p.0 - q[0].0) * dx + (p.1 - q[0].1) * dy)
                            / (dx * dx + dy * dy).max(f32::EPSILON))
                        .clamp(0., 1.);
                        (p.0 - q[0].0 - t * dx).hypot(p.1 - q[0].1 - t * dy) <= diameter / 2.
                    })
            }
        }
    }
    pub fn bounds(&self) -> Bounds {
        match self {
            Self::Rectangle(b) => *b,
            Self::Brush { points, diameter } => {
                let mut b = Bounds {
                    left: f32::INFINITY,
                    top: f32::INFINITY,
                    right: f32::NEG_INFINITY,
                    bottom: f32::NEG_INFINITY,
                };
                for &(x, y) in points {
                    b.left = b.left.min(x);
                    b.top = b.top.min(y);
                    b.right = b.right.max(x);
                    b.bottom = b.bottom.max(y);
                }
                let r = diameter / 2.;
                b.left -= r;
                b.top -= r;
                b.right += r;
                b.bottom += r;
                b
            }
        }
    }
}

/// Magnifier components have independent geometry during output-only movement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ErasePart {
    #[default]
    Object,
    MagnifierOutput,
    MagnifierSource,
    MagnifierConnector,
}

/// Affine transform keeps holes attached to the original object, including nonuniform scale.
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedCoverage {
    pub part: ErasePart,
    pub region: EraseRegion,
    pub transform: [f32; 6],
}
impl ErasedCoverage {
    pub fn new(region: EraseRegion) -> Self {
        Self {
            part: ErasePart::Object,
            region,
            transform: [1., 0., 0., 1., 0., 0.],
        }
    }
    pub fn applies_to(&self, part: ErasePart) -> bool {
        self.part == ErasePart::Object || self.part == part
    }
    pub fn contains(&self, p: (f32, f32)) -> bool {
        let [a, b, c, d, x, y] = self.transform;
        let det = a * d - b * c;
        if det.abs() < f32::EPSILON {
            return false;
        }
        self.region.contains((
            (d * (p.0 - x) - c * (p.1 - y)) / det,
            (-b * (p.0 - x) + a * (p.1 - y)) / det,
        ))
    }
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.transform[4] += dx;
        self.transform[5] += dy;
    }
    pub fn rotate(&mut self, center: (f32, f32), radians: f32) {
        let (sin, cos) = radians.sin_cos();
        let [a, b, c, d, x, y] = self.transform;
        self.transform = [
            cos * a - sin * b,
            sin * a + cos * b,
            cos * c - sin * d,
            sin * c + cos * d,
            center.0 + cos * (x - center.0) - sin * (y - center.1),
            center.1 + sin * (x - center.0) + cos * (y - center.1),
        ];
    }
    pub fn resize(&mut self, old: Bounds, new: Bounds) {
        let sx = new.width() / old.width().max(1.);
        let sy = new.height() / old.height().max(1.);
        let [a, b, c, d, x, y] = self.transform;
        self.transform = [
            a * sx,
            b * sy,
            c * sx,
            d * sy,
            new.left + (x - old.left) * sx,
            new.top + (y - old.top) * sy,
        ];
    }
}

/// Targets are resolved by identity before applying a cut; edit permissions do not limit erasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EraseTarget {
    Object(ObjectId),
    Watermark,
    Spotlight,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holes_follow_nonuniform_scale_and_rotation_on_negative_desktop() {
        let mut cut = ErasedCoverage::new(EraseRegion::Rectangle(Bounds::from_corners(
            (-200., -100.),
            (-180., -80.),
        )));
        let old = Bounds::from_corners((-300., -200.), (-100., 0.));
        let new = Bounds::from_corners((-600., -300.), (-200., -200.));
        cut.resize(old, new);
        assert!(cut.contains((-380., -245.)));
        assert!(!cut.contains((-350., -245.)));
        cut.rotate((-400., -250.), std::f32::consts::FRAC_PI_2);
        assert!(cut.contains((-405., -230.)));
        assert!(!cut.contains((-405., -200.)));
        cut.move_by(10000., -10000.);
        assert!(cut.contains((9595., -10230.)));
    }
}
