//! Frame-scoped fingerprints over an immutable borrowed document, never a document version.
use crate::{annotation_fingerprint, annotation_scene::Document};
use lexift_core::domain::{
    annotation::{Bounds, Extra, Kind, Object, Style},
    geometry::Rect,
};
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
};

#[derive(Clone, Copy)]
pub(crate) struct Content<'a> {
    pub kind: Kind,
    pub bounds: Bounds,
    pub points: &'a [(f32, f32)],
    pub style: &'a Style,
    pub erased: &'a [lexift_core::domain::annotation_erase::ErasedCoverage],
    pub extra: &'a Extra,
}
impl<'a> From<&'a Object> for Content<'a> {
    fn from(object: &'a Object) -> Self {
        let Object {
            erased,
            id: _,
            kind,
            bounds,
            points,
            style,
            extra,
        } = object;
        Self {
            kind: *kind,
            bounds: *bounds,
            points,
            style,
            extra,
            erased,
        }
    }
}
impl Content<'_> {
    fn key(self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        hash_erased(self.erased, &mut hash);
        self.kind.hash(&mut hash);
        for value in [
            self.bounds.left,
            self.bounds.top,
            self.bounds.right,
            self.bounds.bottom,
        ] {
            value.to_bits().hash(&mut hash);
        }
        self.points.len().hash(&mut hash);
        for (x, y) in self.points {
            x.to_bits().hash(&mut hash);
            y.to_bits().hash(&mut hash);
        }
        annotation_fingerprint::style(self.style, &mut hash);
        annotation_fingerprint::extra(self.extra, &mut hash);
        hash.finish()
    }
    fn visible(self, monitor: Rect, scale: f32) -> bool {
        let margin = (self.style.width / 2. + 8.) * scale;
        let intersects = |bounds: Bounds| {
            bounds.right + margin >= monitor.left as f32
                && bounds.left - margin <= monitor.right as f32
                && bounds.bottom + margin >= monitor.top as f32
                && bounds.top - margin <= monitor.bottom as f32
        };
        matches!(self.extra, Extra::Text(_) | Extra::Sequence(_))
            || matches!(self.extra, Extra::Magnifier { output, .. } if intersects(*output))
            || !(self.bounds.right + margin < monitor.left as f32
                || self.bounds.left - margin > monitor.right as f32
                || self.bounds.bottom + margin < monitor.top as f32
                || self.bounds.top - margin > monitor.bottom as f32)
    }
}

/// A normalized gesture supplies borrowed geometry and a single precomputed key.
pub(crate) struct Replacement<'a> {
    index: usize,
    content: Content<'a>,
    key: u64,
}
impl<'a> Replacement<'a> {
    pub fn new(index: usize, content: Content<'a>) -> Self {
        Self {
            index,
            content,
            key: content.key(),
        }
    }
}

pub(crate) struct Frame<'a> {
    document: Document<'a>,
    draft: Option<&'a Object>,
    common: u64,
    keys: RefCell<Vec<Option<u64>>>,
}
impl<'a> Frame<'a> {
    pub fn new(document: impl Into<Document<'a>>, draft: Option<&'a Object>) -> Self {
        let document = document.into();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        hash_erased(&document.session.watermark_erased, &mut hash);
        hash_erased(&document.session.spotlight_erased, &mut hash);
        document.session.spotlight_opacity.to_bits().hash(&mut hash);
        annotation_fingerprint::watermark(document.session.watermark.as_ref(), &mut hash);
        document
            .objects()
            .chain(draft)
            .any(|object| object.kind.is_spotlight())
            .hash(&mut hash);
        Self {
            document,
            draft,
            common: hash.finish(),
            keys: RefCell::new(vec![None; document.len() + usize::from(draft.is_some())]),
        }
    }
    pub fn document(&self) -> Document<'a> {
        self.document
    }
    pub fn draft(&self) -> Option<&'a Object> {
        self.draft
    }
    pub fn get(&self, index: usize) -> Option<&'a Object> {
        self.document.get(index).or_else(|| {
            (index == self.document.len())
                .then_some(self.draft)
                .flatten()
        })
    }
    pub fn object_key(&self, index: usize) -> Option<u64> {
        let object = self.get(index)?;
        let mut keys = self.keys.borrow_mut();
        Some(*keys[index].get_or_insert_with(|| Content::from(object).key()))
    }
    pub fn display(&self, monitor: Rect, scale: f32, hovered: Option<usize>) -> u64 {
        self.signature(monitor, scale, self.document.selected(), hovered, None)
    }
    pub fn document_signature(
        &self,
        monitor: Rect,
        scale: f32,
        replacement: Option<&Replacement<'_>>,
    ) -> u64 {
        self.signature(monitor, scale, None, None, replacement)
    }
    fn signature(
        &self,
        monitor: Rect,
        scale: f32,
        selected: Option<usize>,
        hovered: Option<usize>,
        replacement: Option<&Replacement<'_>>,
    ) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.common.hash(&mut hash);
        [monitor.left, monitor.top, monitor.right, monitor.bottom].hash(&mut hash);
        scale.to_bits().hash(&mut hash);
        for (index, object) in self.document.objects().chain(self.draft).enumerate() {
            let replacement = replacement.filter(|replacement| replacement.index == index);
            let content = replacement
                .map_or_else(|| Content::from(object), |replacement| replacement.content);
            if content.visible(monitor, scale) {
                index.hash(&mut hash);
                replacement
                    .map_or_else(
                        || self.object_key(index).unwrap(),
                        |replacement| replacement.key,
                    )
                    .hash(&mut hash);
            }
        }
        if let Some(index) = selected {
            index.hash(&mut hash);
        }
        hovered.hash(&mut hash);
        hash.finish()
    }
    #[cfg(test)]
    pub fn computed_objects(&self) -> usize {
        self.keys
            .borrow()
            .iter()
            .filter(|key| key.is_some())
            .count()
    }
}

fn hash_erased(
    cuts: &[lexift_core::domain::annotation_erase::ErasedCoverage],
    hash: &mut impl Hasher,
) {
    use lexift_core::domain::annotation_erase::EraseRegion;
    cuts.len().hash(hash);
    for cut in cuts {
        cut.part.hash(hash);
        for v in cut.transform {
            v.to_bits().hash(hash);
        }
        match &cut.region {
            EraseRegion::Rectangle(b) => {
                0u8.hash(hash);
                for v in [b.left, b.top, b.right, b.bottom] {
                    v.to_bits().hash(hash);
                }
            }
            EraseRegion::Brush { points, diameter } => {
                1u8.hash(hash);
                diameter.to_bits().hash(hash);
                points.len().hash(hash);
                for (x, y) in points {
                    x.to_bits().hash(hash);
                    y.to_bits().hash(hash);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{MosaicAnnotation, MosaicEffect, Session};
    fn document() -> Session {
        let mut session = Session::new();
        session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-80., -40.), (80., 40.)),
            points: (0..1_000)
                .map(|i| (-80. + 160. * i as f32 / 1_000., 0.))
                .collect(),
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 80, 12., 1.25)),
        });
        session.selected = None;
        session
    }
    fn copy_document(source: &Session) -> Session {
        let mut copy = Session::new();
        copy.objects = source.objects.clone();
        copy.displays = source.displays.clone();
        copy.watermark = source.watermark.clone();
        copy.spotlight_opacity = source.spotlight_opacity;
        copy.selected = source.selected;
        copy
    }
    fn viewport() -> Rect {
        Rect {
            left: -128,
            top: -128,
            right: 128,
            bottom: 128,
        }
    }

    #[test]
    fn borrowed_batch_hashes_visible_objects_once_and_preserves_input_changes() {
        let session = document();
        let frame = Frame::new(&session, None);
        let remote = Rect {
            left: 1_000,
            top: 0,
            right: 1_256,
            bottom: 256,
        };
        frame.display(remote, 1., None);
        assert_eq!(
            frame.computed_objects(),
            0,
            "culled trajectories are not hashed"
        );
        let original = frame.document_signature(viewport(), 1., None);
        for scale in [1., 1.25] {
            assert_eq!(
                frame.document_signature(viewport(), scale, None),
                Frame::new(&session, None).document_signature(viewport(), scale, None)
            );
            frame.display(viewport(), scale, Some(0));
        }
        assert_eq!(frame.computed_objects(), 1);
        assert_ne!(original, frame.display(viewport(), 1., Some(0)));
        assert_ne!(
            original,
            frame.document_signature(
                Rect {
                    right: 127,
                    ..viewport()
                },
                1.,
                None
            )
        );
        let first_key = frame.object_key(0).unwrap();
        let changes: [fn(&mut Object); 8] = [
            |object| object.points[500].1 += 3.,
            |object| object.bounds.left -= 1.,
            |object| object.style.color = [10, 20, 30],
            |object| object.style.width += 1.,
            |object| object.style.rounding += 1.,
            |object| {
                if let Extra::Mosaic(settings) = &mut object.extra {
                    settings.strength = 25;
                }
            },
            |object| {
                if let Extra::Mosaic(settings) = &mut object.extra {
                    settings.effect = MosaicEffect::Pixelate;
                }
            },
            |object| {
                if let Extra::Mosaic(settings) = &mut object.extra {
                    settings.creation_scale = 1.;
                }
            },
        ];
        for change in changes {
            let mut changed = copy_document(&session);
            change(&mut changed.objects[0]);
            assert_ne!(Frame::new(&changed, None).object_key(0).unwrap(), first_key);
        }
        assert_eq!(
            Frame::new(&session, None).object_key(0).unwrap(),
            first_key,
            "restoring a snapshot uses a fresh batch, without cross-frame versions"
        );
    }

    #[test]
    fn gesture_replacement_is_borrowed_and_does_not_pollute_normal_object_keys() {
        let session = document();
        let frame = Frame::new(&session, None);
        let source = &session.objects[0];
        let points = [(-80., -20.), (80., -20.)];
        let replacement = Replacement::new(
            0,
            Content {
                points: &points,
                bounds: Bounds::from_corners((-80., -30.), (80., 0.)),
                ..Content::from(source)
            },
        );
        let mut owned = copy_document(&session);
        owned.objects[0].points = points.to_vec();
        owned.objects[0].bounds = replacement.content.bounds;
        assert_eq!(
            frame.document_signature(viewport(), 1.25, Some(&replacement)),
            Frame::new(&owned, None).document_signature(viewport(), 1.25, None)
        );
        assert_eq!(
            frame.computed_objects(),
            0,
            "a normalized gesture must not overwrite the original object memo"
        );
        let normal = frame.document_signature(viewport(), 1.25, None);
        assert_ne!(
            normal,
            frame.document_signature(viewport(), 1.25, Some(&replacement))
        );
        assert_eq!(
            normal,
            Frame::new(&session, None).document_signature(viewport(), 1.25, None)
        );
        assert_eq!(frame.computed_objects(), 1);
    }

    #[test]
    fn edited_objects_and_drafts_use_distinct_indices_and_complete_source_content() {
        let session = document();
        let mut edited = session.objects[0].clone();
        edited.style.color = [100, 50, 20];
        let mut draft = session.objects[0].clone();
        draft.points[500].1 += 5.;
        let frame = Frame::new(
            Document {
                session: &session,
                edit: Some((Some(0), &edited)),
            },
            Some(&draft),
        );
        let mut owned = copy_document(&session);
        owned.objects = vec![edited.clone(), draft.clone()];
        assert_eq!(
            frame.document_signature(viewport(), 1., None),
            Frame::new(&owned, None).document_signature(viewport(), 1., None)
        );
        assert_ne!(frame.object_key(0), frame.object_key(1));
        assert_eq!(frame.computed_objects(), 2);
        assert!(frame.object_key(2).is_none());
    }
}
