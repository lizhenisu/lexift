//! Session-local mosaic mask geometry and per-viewport coverage partitions.
use lexift_core::domain::{
    annotation::{Extra, MosaicAnnotation, MosaicEffect, Object, Style},
    geometry::Rect,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(super) struct Geometry {
    pub object: Arc<Object>,
    regions: HashMap<[i32; 4], Vec<Rect>>,
}

impl Geometry {
    pub fn new(source: &Object) -> Self {
        let mut object = source.clone();
        object.style.color = [255; 3];
        object.style.width = 0.;
        if let Extra::Mosaic(settings) = &mut object.extra {
            settings.id = 0;
            settings.strength = 1;
            settings.effect = MosaicEffect::Pixelate;
        }
        Self {
            object: Arc::new(object),
            regions: HashMap::new(),
        }
    }

    /// Compare every non-canonical field. Sampling strength/effect and paint color
    /// do not change the white mask; exhaustive patterns keep new fields visible.
    pub fn matches(&self, source: &Object) -> bool {
        let Object {
            erased,
            id,
            kind,
            bounds,
            points,
            style,
            extra,
        } = source;
        let Style {
            color: _,
            width: _,
            rounding,
            fill,
            dash,
            outline,
        } = style;
        let Extra::Mosaic(MosaicAnnotation {
            id: _,
            effect: _,
            strength: _,
            diameter,
            creation_scale,
        }) = extra
        else {
            return false;
        };
        let Extra::Mosaic(cached) = &self.object.extra else {
            return false;
        };
        self.object.erased == *erased
            && self.object.id == *id
            && self.object.kind == *kind
            && self.object.bounds == *bounds
            && self.object.points == *points
            && self.object.style.rounding == *rounding
            && self.object.style.fill == *fill
            && self.object.style.dash == *dash
            && self.object.style.outline == *outline
            && cached.diameter == *diameter
            && cached.creation_scale == *creation_scale
    }

    pub fn regions(&mut self, viewport: Rect) -> &[Rect] {
        self.regions
            .entry(super::rectangle_key(viewport))
            .or_insert_with(|| {
                crate::annotation_render::mosaic::work_regions(&self.object, &[viewport])
                    .into_iter()
                    .map(|(bounds, _)| bounds)
                    .collect()
            })
    }

    pub fn retain_viewports(&mut self, active: &HashSet<[i32; 4]>) -> bool {
        self.regions.retain(|viewport, _| active.contains(viewport));
        !self.regions.is_empty()
    }
}
