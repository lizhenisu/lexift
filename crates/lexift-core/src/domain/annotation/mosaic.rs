//! Geometry and device-independent parameters for live privacy masks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MosaicEffect {
    #[default]
    Pixelate,
    Blur,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MosaicAnnotation {
    /// Session-local identity, assigned on insertion and preserved in undo snapshots.
    pub id: u64,
    pub effect: MosaicEffect,
    pub strength: u8,
    /// Physical diameter; resizing a brush scales this with its trajectory.
    pub diameter: f32,
    /// Fixed at creation, so dragging between displays does not change the effect.
    pub creation_scale: f32,
}

impl MosaicAnnotation {
    pub fn new(effect: MosaicEffect, strength: u8, logical_diameter: f32, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        Self {
            id: 0,
            effect,
            strength: strength.min(100),
            diameter: logical_diameter.clamp(1., 100.) * scale,
            creation_scale: scale,
        }
    }

    /// Zero is an exact pass-through, without quantization or filtering.
    pub fn block_size(&self) -> u32 {
        if self.strength == 0 {
            return 0;
        }
        ((2. + (self.strength.min(100) as f32 - 1.) * 62. / 99.) * self.creation_scale)
            .round()
            .max(1.) as u32
    }

    pub fn blur_sigma(&self) -> f32 {
        if self.strength == 0 {
            return 0.;
        }
        (0.5 + (self.strength.min(100) as f32 - 1.) * 19.5 / 99.) * self.creation_scale
    }

    /// Capture padding includes all samples used by the effect, including grid cells
    /// straddling a mask edge. The pixel grid is anchored to the desktop origin.
    pub fn sample_padding(&self) -> i32 {
        match self.effect {
            MosaicEffect::Pixelate => self.block_size() as i32,
            MosaicEffect::Blur => (self.blur_sigma() * 3.).ceil() as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::annotation::{Bounds, Extra, Hit, Kind, Object, Session, Style};

    fn brush(points: Vec<(f32, f32)>, scale: f32) -> Object {
        let mut object = Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: Bounds::from_corners((0., 0.), (0., 0.)),
            kind: Kind::MosaicBrush,
            style: Style::default(),
            points,
            extra: Extra::Mosaic(MosaicAnnotation::new(
                MosaicEffect::Pixelate,
                50,
                12.,
                scale,
            )),
        };
        object.refresh_bounds();
        object
    }
    #[test]
    fn single_click_has_round_envelope_and_whole_object_handles() {
        for scale in [1., 1.25] {
            let object = brush(vec![(-200., 100.)], scale);
            assert_eq!(object.bounds.width(), 12. * scale);
            assert_eq!(object.bounds.height(), 12. * scale);
            assert_eq!(object.edit_handles(scale).len(), 8);
            let mut session = Session::new();
            assert!(session.add(object));
            session.selected = None;
            assert_eq!(session.hit((-200., 100.), scale), Some(Hit::Object(0)));
            assert_eq!(session.hit((-250., 100.), scale), None);
        }
        let mut smallest = brush(vec![(-200., 100.)], 1.);
        if let Extra::Mosaic(value) = &mut smallest.extra {
            value.diameter = 1.;
        }
        smallest.refresh_bounds();
        assert!(
            Session::new().add(smallest),
            "one-pixel brush clicks remain valid"
        );
    }
    #[test]
    fn resizing_and_moving_keeps_brush_caps_and_one_undo_transaction() {
        let object = brush(vec![(-200., 100.), (-160., 120.)], 1.25);
        let original = object.clone();
        let mut session = Session::new();
        session.begin_drag();
        assert!(session.add(object));
        session.objects[0].move_by(40., -20.);
        session.finish_drag();
        session.undo();
        assert!(session.objects.is_empty());
        assert!(!session.can_undo());
        let mut resized = original.clone();
        let b = resized.bounds;
        resized.resize_to(Bounds {
            left: b.left,
            top: b.top,
            right: b.left + b.width() * 2.,
            bottom: b.top + b.height() * 2.,
        });
        let Extra::Mosaic(value) = &resized.extra else {
            panic!()
        };
        assert_eq!(value.diameter, 30.);
        assert_eq!(value.creation_scale, 1.25);
        assert_eq!(resized.bounds.width(), b.width() * 2.);
        assert_eq!(resized.bounds.height(), b.height() * 2.);
        assert!(!resized.hit_path((-200., 145.), 1.25));
    }
    #[test]
    fn strength_mapping_and_creation_dpi_are_stable() {
        let mut value = MosaicAnnotation::new(MosaicEffect::Pixelate, 0, 12., 1.25);
        assert_eq!(value.block_size(), 0);
        assert_eq!(value.blur_sigma(), 0.);
        value.strength = 1;
        assert_eq!(value.block_size(), 3);
        assert_eq!(value.blur_sigma(), 0.625);
        value.strength = 100;
        assert_eq!(value.block_size(), 80);
        assert_eq!(value.blur_sigma(), 25.);
        value.effect = MosaicEffect::Blur;
        assert_eq!(value.sample_padding(), 75);
    }
    #[test]
    fn rejecting_first_capture_preserves_later_edits_and_prunes_mask_history() {
        let mut session = Session::new();
        let shape = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((0., 0.), (40., 40.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        };
        session.add(shape.clone());
        let shape = session.objects[0].clone();
        session.add(brush(vec![(80., 80.)], 1.));
        let Extra::Mosaic(m) = &session.objects[1].extra else {
            unreachable!()
        };
        let id = m.id;
        session.update_selected(|o| o.style.rounding = 9.);
        session.selected = Some(0);
        session.begin_drag();
        session.objects[0].move_by(20., 10.);
        session.discard_mosaics(&[id]);
        session.finish_drag();
        assert_eq!(session.objects.len(), 1);
        assert_eq!(session.selected, Some(0));
        assert_eq!(session.objects[0].bounds, shape.bounds.moved(20., 10.));
        assert!(session.undo());
        assert_eq!(session.objects, vec![shape]);
        assert!(session.undo());
        assert!(session.objects.is_empty());
        assert!(
            !session.can_undo(),
            "failed mask and its edits leave no undo entries"
        );
        session.add(brush(vec![(80., 80.)], 1.));
        let Extra::Mosaic(next) = &session.objects[0].extra else {
            unreachable!()
        };
        assert!(next.id > id, "undo never reuses a capture identity");
    }
    #[test]
    fn rejecting_a_deleted_mask_cannot_revive_it_through_undo() {
        let mut session = Session::new();
        session.add(brush(vec![(10., 10.)], 1.));
        let Extra::Mosaic(m) = &session.objects[0].extra else {
            unreachable!()
        };
        let id = m.id;
        session.delete_selected();
        assert!(session.can_undo());
        session.discard_mosaics(&[id]);
        assert!(!session.can_undo());
        assert!(!session.undo());
        session.add(brush(vec![(20., 20.)], 1.));
        let first = session.objects[0].clone();
        session.add(brush(vec![(30., 30.)], 1.));
        let Extra::Mosaic(m) = &session.objects[1].extra else {
            unreachable!()
        };
        session.discard_mosaics(&[m.id]);
        assert_eq!(
            session.selected,
            Some(0),
            "rejected placement restores previous selection"
        );
        assert_eq!(session.objects, vec![first]);
        assert!(session.undo());
        assert!(session.objects.is_empty());
        assert!(!session.can_undo());
    }
}
