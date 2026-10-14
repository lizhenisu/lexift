//! Per-target coverage removed by the eraser. Coordinates are physical desktop pixels.
use super::annotation::{Bounds, ObjectId, Session};

#[derive(Clone, Debug, PartialEq)]
pub enum EraseRegion {
    Rectangle(Bounds),
    RoundedRectangle {
        bounds: Bounds,
        radius: f32,
    },
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
            Self::RoundedRectangle { bounds, radius } => rounded_contains(*bounds, *radius, p),
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
            Self::Rectangle(b) | Self::RoundedRectangle { bounds: b, .. } => *b,
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

/// Shared geometry for rectangular content and temporary eraser controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedRectangle {
    pub bounds: Bounds,
    pub rounding: f32,
    pub creation_scale: f32,
}
impl RoundedRectangle {
    pub fn radius(self) -> f32 {
        (self.rounding * self.creation_scale).clamp(
            0.,
            self.bounds.width().min(self.bounds.height()).max(0.) / 2.,
        )
    }
    pub fn handles(self, scale: f32) -> Vec<(f32, f32)> {
        let b = self.bounds;
        let inset = (self.radius() * 0.5 + 10. * scale)
            .min(b.width() / 2.)
            .min(b.height() / 2.);
        let mut handles = b.handles().to_vec();
        handles.extend([
            (b.left + inset, b.top + inset),
            (b.right - inset, b.top + inset),
            (b.left + inset, b.bottom - inset),
            (b.right - inset, b.bottom - inset),
        ]);
        handles
    }
    pub fn edit_handle(&mut self, handle: usize, point: (f32, f32), scale: f32) {
        if handle < 8 {
            self.bounds = self.bounds.resized(handle, point);
        } else if handle < 12 {
            let b = self.bounds;
            let x = if handle == 8 || handle == 10 {
                point.0 - b.left
            } else {
                b.right - point.0
            };
            let y = if handle < 10 {
                point.1 - b.top
            } else {
                b.bottom - point.1
            };
            self.rounding = ((x.min(y) - 10. * scale) * 2. / self.creation_scale).clamp(
                0.,
                b.width().min(b.height()).max(0.) / 2. / self.creation_scale,
            );
        }
    }
    pub fn region(self) -> EraseRegion {
        EraseRegion::RoundedRectangle {
            bounds: self.bounds,
            radius: self.radius(),
        }
    }
    pub fn hit(self, point: (f32, f32), scale: f32) -> Option<RectangleEraseHit> {
        self.handles(scale)
            .iter()
            .enumerate()
            .find_map(|(i, p)| {
                ((point.0 - p.0).abs() <= 9. * scale && (point.1 - p.1).abs() <= 9. * scale)
                    .then_some(RectangleEraseHit::Handle(i))
            })
            .or_else(|| {
                self.region()
                    .contains(point)
                    .then_some(RectangleEraseHit::Body)
            })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectangleEraseHit {
    Handle(usize),
    Body,
}
fn rounded_contains(b: Bounds, radius: f32, p: (f32, f32)) -> bool {
    if p.0 < b.left || p.0 > b.right || p.1 < b.top || p.1 > b.bottom {
        return false;
    }
    let r = radius.max(0.).min(b.width().min(b.height()).max(0.) / 2.);
    let x = p.0.clamp(b.left + r, b.right - r);
    let y = p.1.clamp(b.top + r, b.bottom - r);
    (p.0 - x).hypot(p.1 - y) <= r
}
/// Only coverage is retained: no screenshot, UI object or duplicate document body.
#[derive(Clone)]
pub(crate) struct RectangleEraseEdit {
    pub geometry: RoundedRectangle,
    editing: bool,
    objects: Vec<(ObjectId, Vec<ErasedCoverage>)>,
    watermark: Option<Vec<ErasedCoverage>>,
    spotlight: Option<Vec<ErasedCoverage>>,
}
impl RectangleEraseEdit {
    fn new(s: &Session, geometry: RoundedRectangle) -> Self {
        Self {
            geometry,
            editing: false,
            objects: s.objects.iter().map(|o| (o.id, o.erased.clone())).collect(),
            watermark: s.watermark.as_ref().map(|_| s.watermark_erased.clone()),
            spotlight: s
                .objects
                .iter()
                .any(|o| o.kind.is_spotlight())
                .then(|| s.spotlight_erased.clone()),
        }
    }
    fn restore(&self, s: &mut Session) {
        for (id, cuts) in &self.objects {
            if let Some(i) = s.object_index(*id) {
                s.objects[i].erased.clone_from(cuts);
            }
        }
        if let Some(cuts) = &self.watermark {
            s.watermark_erased.clone_from(cuts);
        }
        if let Some(cuts) = &self.spotlight {
            s.spotlight_erased.clone_from(cuts);
        }
    }
}
impl Session {
    pub fn rectangle_erase_geometry(&self) -> Option<RoundedRectangle> {
        self.rectangle_erase.as_ref().map(|e| e.geometry)
    }
    pub fn rectangle_erase_show_handles(&self) -> bool {
        self.rectangle_erase
            .as_ref()
            .is_some_and(|edit| edit.editing)
    }
    /// Finish creation independently of document history: blank geometry is still editable.
    pub(crate) fn complete_rectangle_erase(&mut self) -> bool {
        if let Some(edit) = &mut self.rectangle_erase {
            if edit.geometry.bounds.width() >= 2. && edit.geometry.bounds.height() >= 2. {
                edit.editing = true;
            } else {
                self.cancel_drag();
                return false;
            }
        }
        true
    }
    pub fn finish_rectangle_erase(&mut self) {
        self.rectangle_erase = None;
    }
    pub fn begin_rectangle_erase(&mut self, geometry: RoundedRectangle) {
        self.finish_rectangle_erase();
        self.begin_drag();
        self.rectangle_erase = Some(RectangleEraseEdit::new(self, geometry));
    }
    /// Restore the creation baseline before the raster adapter resolves visible targets.
    pub fn prepare_rectangle_erase(&mut self, geometry: RoundedRectangle) -> Option<EraseRegion> {
        let mut edit = self.rectangle_erase.take()?;
        edit.restore(self);
        edit.geometry = geometry;
        self.rectangle_erase = Some(edit);
        Some(geometry.region())
    }
    pub fn apply_rectangle_erase(&mut self, region: EraseRegion, targets: &[EraseTarget]) {
        let Some(edit) = &self.rectangle_erase else {
            return;
        };
        let targets: Vec<_> = targets
            .iter()
            .copied()
            .filter(|target| match target {
                EraseTarget::Object(id) => edit.objects.iter().any(|(existing, _)| existing == id),
                EraseTarget::Watermark => edit.watermark.is_some(),
                EraseTarget::Spotlight => edit.spotlight.is_some(),
            })
            .collect();
        self.erase(region, &targets);
    }
}

#[cfg(test)]
mod rectangle_edit_tests {
    use super::*;
    use crate::domain::annotation::{Extra, Kind, Object, Style};
    fn document() -> Session {
        let mut s = Session::new();
        s.add(Object {
            id: ObjectId::DRAFT,
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-100., -100.), (200., 200.)),
            points: vec![],
            style: Style::default(),
            extra: Extra::None,
            erased: vec![],
        });
        s.selected = None;
        s
    }
    fn geometry() -> RoundedRectangle {
        RoundedRectangle {
            bounds: Bounds::from_corners((0., 0.), (100., 100.)),
            rounding: 21.,
            creation_scale: 1.,
        }
    }
    fn apply(s: &mut Session, g: RoundedRectangle) {
        let region = s.prepare_rectangle_erase(g).unwrap();
        s.apply_rectangle_erase(region, &[EraseTarget::Object(s.objects[0].id)]);
    }
    #[test]
    fn rectangle_edit_move_cancel_undo_and_finalize() {
        let mut s = document();
        let g = geometry();
        s.begin_rectangle_erase(g);
        assert!(!s.rectangle_erase_show_handles());
        apply(&mut s, g);
        s.finish_drag();
        assert!(s.rectangle_erase_show_handles());
        assert_eq!(g.handles(1.).len(), 12);
        let mut moved = g;
        moved.bounds = Bounds::from_corners((110., 0.), (210., 100.));
        s.begin_drag();
        apply(&mut s, moved);
        assert!(!s.objects[0].erased.iter().any(|c| c.contains((50., 50.))));
        s.cancel_drag();
        assert_eq!(s.rectangle_erase_geometry(), Some(g));
        assert!(s.objects[0].erased[0].contains((50., 50.)));
        s.begin_drag();
        apply(&mut s, moved);
        s.finish_drag();
        assert!(s.undo());
        assert!(s.rectangle_erase_geometry().is_none());
        assert!(s.objects[0].erased[0].contains((50., 50.)));
        assert!(s.undo());
        assert!(s.objects[0].erased.is_empty());
    }
    #[test]
    fn rectangle_edit_preserves_earlier_holes_and_rejects_later_targets() {
        let mut s = document();
        let id = s.objects[0].id;
        s.erase(
            EraseRegion::Rectangle(Bounds::from_corners((-90., -90.), (-80., -80.))),
            &[EraseTarget::Object(id)],
        );
        s.add(s.objects[0].clone());
        let later = s.objects.pop().unwrap();
        let later_id = later.id;
        s.selected = None;
        let g = geometry();
        s.begin_rectangle_erase(g);
        apply(&mut s, g);
        s.finish_drag();
        // Even a caller that introduces an object outside normal add cannot expand frozen identities.
        let mut later = later;
        later.erased.clear();
        s.objects.push(later);
        s.begin_drag();
        let region = s.prepare_rectangle_erase(g).unwrap();
        s.apply_rectangle_erase(
            region,
            &[EraseTarget::Object(id), EraseTarget::Object(later_id)],
        );
        s.finish_drag();
        assert!(s.objects[1].erased.is_empty());
        assert!(s.objects[0].erased.iter().any(|c| c.contains((-85., -85.))));
        s.finish_rectangle_erase();
        assert!(s.rectangle_erase_geometry().is_none());
    }
    #[test]
    fn rectangle_edit_blank_geometry_survives_without_history_and_cancel_cleans_up() {
        let mut s = document();
        let g = geometry();
        s.begin_rectangle_erase(g);
        s.finish_drag();
        assert_eq!(s.rectangle_erase_geometry(), Some(g));
        assert!(s.rectangle_erase_show_handles());
        assert!(s.undo());
        assert!(s.objects.is_empty());
        s = document();
        s.begin_rectangle_erase(g);
        apply(&mut s, g);
        s.cancel_drag();
        assert!(s.rectangle_erase_geometry().is_none());
        assert!(s.objects[0].erased.is_empty());
    }
    #[test]
    fn rectangle_edit_rejects_tiny_creation_but_keeps_blank_valid_geometry() {
        for end in [(0., 0.), (1., 20.), (20., 1.), (2., 2.), (-20., -20.)] {
            let mut s = Session::new();
            let mut g = geometry();
            g.bounds = Bounds::from_corners((0., 0.), end);
            s.begin_rectangle_erase(g);
            s.finish_drag();
            let valid = g.bounds.width() >= 2. && g.bounds.height() >= 2.;
            assert_eq!(s.rectangle_erase_show_handles(), valid);
            assert_eq!(s.rectangle_erase_geometry().is_some(), valid);
            assert!(!s.can_undo());
        }
    }
    #[test]
    fn tiny_rectangle_creation_restores_coverage_instead_of_committing_hidden_cut() {
        let mut s = document();
        let mut g = geometry();
        g.bounds.right = g.bounds.left + 1.;
        s.begin_rectangle_erase(g);
        apply(&mut s, g);
        assert!(!s.objects[0].erased.is_empty());
        s.finish_drag();
        assert!(s.rectangle_erase_geometry().is_none());
        assert!(s.objects[0].erased.is_empty());
        assert!(s.undo());
        assert!(s.objects.is_empty());
    }
    #[test]
    fn rectangle_shared_controls_and_rounded_coverage() {
        for scale in [1., 1.25] {
            let mut g = geometry();
            g.creation_scale = scale;
            assert!(!g.region().contains((0., 0.)));
            assert!(g.region().contains((50., 50.)));
            for h in 0..12 {
                assert_eq!(
                    g.hit(g.handles(scale)[h], scale),
                    Some(RectangleEraseHit::Handle(h))
                );
            }
            g.edit_handle(8, (0., 0.), scale);
            assert_eq!(g.rounding, 0.);
            g.edit_handle(11, (0., 0.), scale);
            assert!(g.radius() <= 50.);
        }
    }
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
