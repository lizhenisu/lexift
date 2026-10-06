//! Compare immutable scene content without scanning pixel buffers or undo state.
use lexift_core::ports::annotation_render::{
    AnnotationScene, MagnifierLayer, MosaicLayer, RasterLayer, RenderLayer, RenderSurface,
};
use std::sync::Arc;

fn rasters(a: &[RasterLayer], b: &[RasterLayer]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            let RasterLayer {
                key,
                bounds,
                pixels: _,
            } = a;
            // Core's immutable-key contract identifies pixels; translations remain
            // observable even when they reuse a key.
            *key == b.key && *bounds == b.bounds
        })
}
fn layers(a: &[RenderLayer], b: &[RenderLayer]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (RenderLayer::Raster(a), RenderLayer::Raster(b)) => {
                rasters(std::slice::from_ref(a), std::slice::from_ref(b))
            }
            (RenderLayer::Mosaic(a), RenderLayer::Mosaic(b)) => {
                let MosaicLayer { object, masks } = a;
                (Arc::ptr_eq(object, &b.object) || object == &b.object) && rasters(masks, &b.masks)
            }
            (RenderLayer::Magnifier(a), RenderLayer::Magnifier(b)) => {
                let MagnifierLayer {
                    object,
                    masks,
                    decorations,
                } = a;
                (Arc::ptr_eq(object, &b.object) || object == &b.object)
                    && rasters(masks, &b.masks)
                    && rasters(decorations, &b.decorations)
            }
            _ => false,
        })
}
/// Submission revisions are requests, not pixel changes. Session identity and
/// every content field still participate, including coverage and capture policy.
pub(crate) fn same(a: &AnnotationScene, b: &AnnotationScene) -> bool {
    let AnnotationScene {
        stamp,
        surfaces,
        layers: document,
        interaction,
        paused_regions,
        excluded_windows,
    } = a;
    stamp.session == b.stamp.session
        && surfaces.len() == b.surfaces.len()
        && surfaces.iter().zip(b.surfaces.iter()).all(|(a, b)| {
            let RenderSurface {
                input_window,
                bounds,
                scale,
            } = a;
            *input_window == b.input_window
                && *bounds == b.bounds
                && scale.to_bits() == b.scale.to_bits()
        })
        && (Arc::ptr_eq(document, &b.layers) || layers(document, &b.layers))
        && (Arc::ptr_eq(interaction, &b.interaction) || rasters(interaction, &b.interaction))
        && paused_regions == &b.paused_regions
        && excluded_windows == &b.excluded_windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::{
            annotation::{
                Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, ObjectId, Style,
            },
            geometry::Rect,
        },
        ports::{
            annotation_render::{RasterKey, RenderStamp},
            magnifier::WindowToken,
        },
    };
    fn fixture() -> AnnotationScene {
        let bounds = Rect {
            left: -8,
            top: -4,
            right: -6,
            bottom: -2,
        };
        let raster = RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 1,
            },
            bounds,
            pixels: Arc::from([0u8; 16]),
        };
        let object = Arc::new(Object {
            erased: Vec::new(),
            id: ObjectId::DRAFT,
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((-8., -4.), (-6., -2.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Mosaic(MosaicAnnotation {
                id: 1,
                effect: MosaicEffect::Pixelate,
                strength: 50,
                diameter: 12.,
                creation_scale: 1.25,
            }),
        });
        let mut magnifier = (*object).clone();
        magnifier.kind = Kind::Magnifier;
        magnifier.extra = Extra::Magnifier {
            output: magnifier.bounds,
            ellipse: false,
            zoom: 2.,
            connector: lexift_core::domain::annotation::MagnifierConnector::None,
            erase_annotations: false,
            antialias: false,
            shadow: false,
        };
        AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([RenderSurface {
                input_window: WindowToken(1),
                bounds,
                scale: 1.25,
            }]),
            layers: Arc::from([
                RenderLayer::Raster(raster.clone()),
                RenderLayer::Mosaic(MosaicLayer {
                    object: object.clone(),
                    masks: Arc::from([raster.clone()]),
                }),
                RenderLayer::Magnifier(MagnifierLayer {
                    object: Arc::new(magnifier),
                    masks: Arc::from([raster.clone()]),
                    decorations: Arc::from([raster.clone()]),
                }),
            ]),
            interaction: Arc::from([raster]),
            paused_regions: Arc::from([bounds]),
            excluded_windows: Arc::from([WindowToken(2)]),
        }
    }
    #[test]
    fn equality_covers_every_scene_field_and_layer_order_without_pixel_scans() {
        let first = fixture();
        first.validate().unwrap();
        let mut repeated = fixture();
        repeated.stamp.revision = 100;
        assert!(
            same(&first, &repeated),
            "equivalent separately allocated metadata must match"
        );
        for change in 0..12 {
            let mut changed = first.clone();
            match change {
                0 => changed.stamp.session += 1,
                1 => Arc::make_mut(&mut changed.surfaces)[0].input_window = WindowToken(3),
                2 => Arc::make_mut(&mut changed.surfaces)[0].bounds.left -= 1,
                3 => Arc::make_mut(&mut changed.surfaces)[0].scale = 1.,
                4 => changed.layers = Arc::from([]),
                5 => Arc::make_mut(&mut changed.layers).swap(0, 1),
                6 => Arc::make_mut(&mut changed.interaction)[0].key.revision += 1,
                7 => Arc::make_mut(&mut changed.interaction)[0].bounds.left -= 1,
                8 => changed.paused_regions = Arc::from([]),
                9 => changed.excluded_windows = Arc::from([]),
                10 => {
                    if let RenderLayer::Mosaic(layer) = &mut Arc::make_mut(&mut changed.layers)[1]
                        && let Extra::Mosaic(settings) = &mut Arc::make_mut(&mut layer.object).extra
                    {
                        settings.strength = 80;
                    }
                }
                _ => {
                    if let RenderLayer::Magnifier(layer) =
                        &mut Arc::make_mut(&mut changed.layers)[2]
                    {
                        Arc::make_mut(&mut layer.decorations)[0].key.revision += 1;
                    }
                }
            }
            assert!(
                !same(&first, &changed),
                "scene field {change} must invalidate"
            );
        }
        let mut masks = first.clone();
        if let RenderLayer::Mosaic(layer) = &mut Arc::make_mut(&mut masks.layers)[1] {
            Arc::make_mut(&mut layer.masks)[0].bounds.left -= 1;
        }
        assert!(!same(&first, &masks));
    }
}
