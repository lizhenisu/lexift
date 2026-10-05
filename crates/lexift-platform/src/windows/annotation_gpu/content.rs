//! Effect inputs exclude transient overlays and presentation-only revisions.
use super::super::screen_capture::regions::ChangedRegions;
use lexift_core::{domain::geometry::Rect, ports::annotation_render::*};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Output {
    Mosaic(u64),
    Lens(u64),
}

/// Conservative desktop bounds of changed captured pixels, never CPU pixel data.
#[derive(Clone, Copy)]
pub(super) enum SourceChanges<'a> {
    All,
    Regions(&'a [Rect]),
}

impl SourceChanges<'_> {
    pub fn any(self) -> bool {
        match self {
            Self::All => true,
            Self::Regions(regions) => !regions.is_empty(),
        }
    }
    pub fn intersects(self, sample: Rect) -> bool {
        match self {
            Self::All => true,
            Self::Regions(regions) => regions.iter().any(|region| {
                sample.left < region.right
                    && region.left < sample.right
                    && sample.top < region.bottom
                    && region.top < sample.bottom
            }),
        }
    }
}

impl From<bool> for SourceChanges<'_> {
    fn from(changed: bool) -> Self {
        if changed {
            Self::All
        } else {
            Self::Regions(&[])
        }
    }
}

/// Capture acknowledges pixels before the display can accept them. Keep bounded
/// invalidation metadata until effects consume it, and a separate display debt
/// until native submission succeeds. No captured frame queue is retained.
#[derive(Default)]
pub(super) struct PendingFrame {
    regions: ChangedRegions,
    display: bool,
}

impl PendingFrame {
    pub fn capture(&mut self, regions: impl IntoIterator<Item = Rect>) {
        for region in regions {
            self.display = true;
            self.regions.extend([region]);
        }
    }
    pub fn changes(&self) -> SourceChanges<'_> {
        SourceChanges::Regions(self.regions.as_slice())
    }
    pub fn applied(&mut self) {
        self.regions.clear();
    }
    pub fn needs_display(&self) -> bool {
        self.display
    }
    pub fn presented(&mut self) {
        debug_assert!(self.regions.as_slice().is_empty());
        self.display = false;
    }
}

/// Both dependency tracking and texture composition must use the same sampling order.
#[derive(Clone, Copy)]
pub(super) enum SampleRule {
    Below(usize),
    Lens { index: usize, erase: bool },
}

impl SampleRule {
    pub fn layers(self, scene: &AnnotationScene) -> impl Iterator<Item = &RenderLayer> {
        scene
            .layers
            .iter()
            .enumerate()
            .filter(move |(position, layer)| match self {
                Self::Below(index) => *position < index,
                Self::Lens { index, erase } => {
                    !erase
                        && (matches!(layer, RenderLayer::Raster(_))
                            || (*position < index && matches!(layer, RenderLayer::Mosaic(_))))
                }
            })
            .map(|(_, layer)| layer)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Dependency {
    Raster(RasterKey, Rect),
    Effect(Output, u64, Rect),
}

/// Identity of the ordered commands actually visible on the display surfaces.
#[derive(Clone)]
pub(super) struct PresentationInputs {
    #[cfg(test)]
    pub sources: Vec<super::super::screen_capture::diagnostics::Published>,
    pub session: u64,
    pub surfaces: Arc<[RenderSurface]>,
    pub commands: Vec<Dependency>,
}

impl PartialEq for PresentationInputs {
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session
            && self.commands == other.commands
            && self.surfaces.len() == other.surfaces.len()
            && self
                .surfaces
                .iter()
                .zip(other.surfaces.iter())
                .all(|(a, b)| a.bounds == b.bounds && a.input_window == b.input_window)
    }
}

/// Exact metadata of the content drawn into one effect's sample footprint.
/// Pixel buffers and object trajectories are deliberately not copied here.
#[derive(PartialEq)]
pub(super) struct TileInput {
    pub session: u64,
    pub source_revision: u64,
    pub owner: lexift_core::domain::annotation::ObjectId,
    pub geometry: Option<lexift_core::domain::annotation::Bounds>,
    pub settings: TileSettings,
    pub mask: (RasterKey, Rect),
    pub sample: Rect,
    pub dependencies: Vec<Dependency>,
}

#[derive(PartialEq)]
pub(super) enum TileSettings {
    Mosaic {
        effect: lexift_core::domain::annotation::MosaicEffect,
        strength: u8,
        scale: f32,
    },
    Lens {
        output: lexift_core::domain::annotation::Bounds,
        zoom: f32,
        erase: bool,
        antialias: bool,
    },
}

impl TileSettings {
    pub fn from_object(object: &lexift_core::domain::annotation::Object) -> Self {
        use lexift_core::domain::annotation::Extra;
        match &object.extra {
            Extra::Mosaic(settings) => Self::Mosaic {
                effect: settings.effect,
                strength: settings.strength,
                scale: settings.creation_scale,
            },
            Extra::Magnifier {
                output,
                zoom,
                erase_annotations,
                antialias,
                ..
            } => Self::Lens {
                output: *output,
                zoom: *zoom,
                erase: *erase_annotations,
                antialias: *antialias,
            },
            _ => unreachable!("only screen effects own sampled tile inputs"),
        }
    }
}

#[derive(Default)]
pub(super) struct EffectInputs {
    layers: Option<Arc<[RenderLayer]>>,
    paused: Arc<[Rect]>,
}

impl EffectInputs {
    pub fn matches(&self, scene: &AnnotationScene) -> bool {
        self.paused == scene.paused_regions
            && self.layers.as_ref().is_some_and(|layers| {
                Arc::ptr_eq(layers, &scene.layers)
                    || (layers.len() == scene.layers.len()
                        && layers
                            .iter()
                            .zip(scene.layers.iter())
                            .all(|(a, b)| same_layer(a, b)))
            })
    }

    pub fn retain(&mut self, scene: &AnnotationScene) {
        self.layers = Some(scene.layers.clone());
        self.paused = scene.paused_regions.clone();
    }
}

fn same_raster(a: &RasterLayer, b: &RasterLayer) -> bool {
    // Assets are immutable within a revision. Never compare full pixel buffers
    // in the frame loop; the compiler must revise keys when their pixels change.
    a.key == b.key && a.bounds == b.bounds
}

fn same_rasters(a: &[RasterLayer], b: &[RasterLayer]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_raster(a, b))
}

fn same_layer(a: &RenderLayer, b: &RenderLayer) -> bool {
    match (a, b) {
        (RenderLayer::Raster(a), RenderLayer::Raster(b)) => same_raster(a, b),
        (RenderLayer::Mosaic(a), RenderLayer::Mosaic(b)) => {
            (Arc::ptr_eq(&a.object, &b.object) || a.object == b.object)
                && same_rasters(&a.masks, &b.masks)
        }
        (RenderLayer::Magnifier(a), RenderLayer::Magnifier(b)) => {
            (Arc::ptr_eq(&a.object, &b.object) || a.object == b.object)
                && same_rasters(&a.masks, &b.masks)
                && same_rasters(&a.decorations, &b.decorations)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{
        Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, Style,
    };

    #[test]
    fn pending_changes_survive_effect_and_display_waits_without_a_frame_queue() {
        let a = Rect {
            left: -20,
            top: 0,
            right: -10,
            bottom: 10,
        };
        let b = Rect {
            left: 30,
            top: 0,
            right: 40,
            bottom: 10,
        };
        let mut pending = PendingFrame::default();
        pending.capture([a]);
        pending.capture([]); // The next sample has already acknowledged a.
        assert!(pending.changes().intersects(a));
        assert!(!pending.changes().intersects(b));
        pending.applied();
        assert!(!pending.changes().any());
        assert!(
            pending.needs_display(),
            "failed presentation must still retry"
        );
        pending.capture([b]);
        assert!(pending.changes().intersects(b));
        pending.applied();
        pending.presented();
        assert!(!pending.needs_display());
    }

    #[test]
    fn indefinite_display_wait_bounds_metadata_and_keeps_all_changed_areas() {
        let mut pending = PendingFrame::default();
        for x in 0..10_000 {
            pending.capture([Rect {
                left: x * 3,
                top: 0,
                right: x * 3 + 1,
                bottom: 1,
            }]);
            assert!(pending.regions.as_slice().len() <= 64);
        }
        for x in 0..10_000 {
            assert!(pending.changes().intersects(Rect {
                left: x * 3,
                top: 0,
                right: x * 3 + 1,
                bottom: 1
            }));
        }
    }

    fn scene() -> AnnotationScene {
        let bounds = Rect {
            left: -4,
            top: 0,
            right: 0,
            bottom: 4,
        };
        let mask = RasterLayer {
            key: RasterKey {
                asset: 2,
                revision: 1,
            },
            bounds,
            pixels: vec![255; 64].into(),
        };
        AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([]),
            interaction: Arc::from([]),
            excluded_windows: Arc::from([]),
            paused_regions: Arc::from([]),
            layers: vec![
                RenderLayer::Raster(RasterLayer {
                    key: RasterKey {
                        asset: 1,
                        revision: 1,
                    },
                    bounds,
                    pixels: [20, 30, 40, 255].repeat(16).into(),
                }),
                RenderLayer::Mosaic(MosaicLayer {
                    object: Arc::new(Object {
                        id: Default::default(),
                        kind: Kind::MosaicArea,
                        bounds: Bounds::from_corners((-4., 0.), (0., 4.)),
                        points: Vec::new(),
                        style: Style::default(),
                        extra: Extra::Mosaic(MosaicAnnotation::new(
                            MosaicEffect::Pixelate,
                            1,
                            12.,
                            1.,
                        )),
                    }),
                    masks: vec![mask].into(),
                }),
            ]
            .into(),
        }
    }

    #[test]
    fn interaction_and_presentation_changes_keep_document_effect_inputs() {
        let original = scene();
        let mut inputs = EffectInputs::default();
        assert!(!inputs.matches(&original));
        inputs.retain(&original);
        let mut overlay = original.clone();
        overlay.stamp.revision += 1;
        overlay.interaction = vec![original.layers[0].rasters().next().unwrap().clone()].into();
        // New allocations with identical metadata must also share effect work.
        overlay.layers = original.layers.to_vec().into();
        assert!(inputs.matches(&overlay));
    }

    #[test]
    fn source_document_geometry_parameters_and_pause_invalidate_effects() {
        let original = scene();
        let mut inputs = EffectInputs::default();
        inputs.retain(&original);
        for change in 0..5 {
            let mut changed = original.clone();
            let mut layers = original.layers.to_vec();
            match change {
                0 => {
                    if let RenderLayer::Raster(layer) = &mut layers[0] {
                        layer.key.revision += 1;
                    }
                }
                1 => {
                    if let RenderLayer::Mosaic(layer) = &mut layers[1] {
                        let mut masks = layer.masks.to_vec();
                        masks[0].bounds.left -= 1;
                        layer.masks = masks.into();
                    }
                }
                2 => {
                    if let RenderLayer::Mosaic(layer) = &mut layers[1]
                        && let Extra::Mosaic(settings) = &mut Arc::make_mut(&mut layer.object).extra
                    {
                        settings.strength += 1;
                    }
                }
                3 => layers.reverse(),
                _ => {
                    changed.paused_regions = vec![Rect {
                        left: -4,
                        top: 0,
                        right: 0,
                        bottom: 4,
                    }]
                    .into()
                }
            }
            changed.layers = layers.into();
            assert!(
                !inputs.matches(&changed),
                "change {change} must invalidate effects"
            );
        }
    }
}
