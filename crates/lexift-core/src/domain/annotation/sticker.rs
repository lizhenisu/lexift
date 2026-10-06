//! Static privacy overlays. Only geometry and texture settings enter undo snapshots.
use super::Bounds;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StickerTexture {
    #[default]
    Glass,
    Frost,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StickerAnnotation {
    pub texture: StickerTexture,
    pub opacity: u8,
    pub texture_size: f32,
    pub diameter: f32,
    pub creation_scale: f32,
    pub seed: u32,
    pub origin: (f32, f32),
    pub stretch: (f32, f32),
}

impl StickerAnnotation {
    pub fn new(origin: (f32, f32), scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        Self {
            texture: StickerTexture::Glass,
            opacity: 95,
            texture_size: 12. * scale,
            diameter: 12. * scale,
            creation_scale: scale,
            seed: 0x6c65_7869,
            origin,
            stretch: (1., 1.),
        }
    }

    /// Affine mapping keeps the original texture attached to its coverage.
    pub fn resize(&mut self, before: Bounds, after: Bounds) {
        let x = after.width() / before.width().max(1.);
        let y = after.height() / before.height().max(1.);
        self.origin = (
            after.left + (self.origin.0 - before.left) * x,
            after.top + (self.origin.1 - before.top) * y,
        );
        self.stretch.0 *= x;
        self.stretch.1 *= y;
    }

    pub fn displayed_size(&self) -> f32 {
        self.texture_size * (self.stretch.0 * self.stretch.1).sqrt() / self.creation_scale
    }

    pub fn set_displayed_size(&mut self, size: f32) {
        self.texture_size = size.clamp(1., 64.) * self.creation_scale
            / (self.stretch.0 * self.stretch.1).sqrt().max(0.001);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        annotation::{EditScope, Extra, Hit, Kind, Object, Session, Style},
        annotation_erase::EraseRegion,
    };

    fn object(kind: Kind, scale: f32) -> Object {
        let mut o = Object {
            erased: vec![],
            id: Default::default(),
            kind,
            bounds: Bounds::from_corners((-100., -50.), (0., 50.)),
            style: Style::default(),
            points: if kind == Kind::StickerBrush {
                vec![(-80., 0.), (-20., 10.)]
            } else {
                vec![]
            },
            extra: Extra::Sticker(StickerAnnotation::new((-100., -50.), scale)),
        };
        if kind == Kind::StickerBrush {
            o.refresh_bounds();
        }
        o
    }

    #[test]
    fn sticker_scope_geometry_texture_and_undo_are_independent_from_live_mosaic() {
        for scale in [1., 1.25] {
            for kind in [Kind::StickerArea, Kind::StickerBrush] {
                let mut o = object(kind, scale);
                assert_eq!(EditScope::Sticker.allows(kind), kind == Kind::StickerArea);
                assert!(!EditScope::Mosaic.allows(kind));
                assert_eq!(kind.tool(), 12);
                let Extra::Sticker(s) = &o.extra else {
                    panic!()
                };
                assert_eq!(s.opacity, 95);
                assert_eq!(s.displayed_size(), 12.);
                let mut session = Session::new();
                session.set_edit_scope(EditScope::Sticker);
                assert!(session.add(o.clone()));
                let center = o.bounds.center();
                session.selected = None;
                assert_eq!(
                    session.hit(center, scale),
                    (kind == Kind::StickerArea).then_some(Hit::Object(0))
                );
                session.set_edit_scope(EditScope::Mosaic);
                assert_eq!(session.hit(center, scale), None);
                assert_eq!(
                    o.edit_handles(scale).len(),
                    if kind == Kind::StickerArea { 12 } else { 0 }
                );
                if kind == Kind::StickerBrush {
                    let before = o.clone();
                    o.move_by(20., -10.);
                    o.resize_to(Bounds::from_corners((0., 0.), (200., 200.)));
                    assert_eq!(o, before);
                    assert!(session.undo());
                    assert!(session.objects.is_empty());
                    continue;
                }
                let before = o.bounds;
                o.move_by(20., -10.);
                let Extra::Sticker(s) = &o.extra else {
                    panic!()
                };
                assert_eq!(s.origin, (-80., -60.));
                let b = o.bounds;
                o.resize_to(Bounds {
                    right: b.left + b.width() * 2.,
                    bottom: b.top + b.height() * 2.,
                    ..b
                });
                let Extra::Sticker(s) = &mut o.extra else {
                    panic!()
                };
                assert_eq!(s.stretch, (2., 2.));
                assert_eq!(s.displayed_size(), 24.);
                s.set_displayed_size(8.);
                assert_eq!(s.displayed_size(), 8.);
                if kind == Kind::StickerBrush {
                    assert_eq!(s.diameter, 24. * scale);
                }
                assert!(o.bounds.width() > before.width());
                assert!(session.undo());
                assert!(session.objects.is_empty());
            }
        }
    }

    #[test]
    fn erased_sticker_holes_follow_affine_motion_and_clear_is_undoable() {
        use crate::domain::annotation_erase::EraseTarget;
        let mut session = Session::new();
        assert!(session.add(object(Kind::StickerArea, 1.25)));
        let id = session.objects[0].id;
        session.erase(
            EraseRegion::Rectangle(Bounds::from_corners((-95., -45.), (-70., -20.))),
            &[EraseTarget::Object(id)],
        );
        session.selected = None;
        assert_eq!(session.hit((-85., -35.), 1.25), None);
        session.selected = Some(0);
        session.begin_drag();
        session.update_selected(|o| o.move_by(100., 100.));
        session.finish_drag();
        assert!(session.objects[0].erased[0].contains((15., 65.)));
        let before = session.objects.clone();
        assert!(session.clear_all());
        assert!(session.objects.is_empty());
        assert!(session.undo());
        assert_eq!(session.objects, before);
        assert!(session.undo());
        assert!(session.objects[0].erased[0].contains((-85., -35.)));
    }
}
