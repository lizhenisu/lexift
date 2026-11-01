//! Build capture-visible ink separately from capture-excluded editor feedback.
//! Source namespaces prevent the suspended desktop and recording documents from
//! aliasing cached rasters without modifying either document. Effect output
//! identity is its namespaced mask asset; document object identities stay intact.
use super::*;
use crate::annotation_scene::{SceneCompiler, SceneInput};
use lexift_core::ports::annotation_render::{
    AnnotationScene, RasterKey, RasterLayer, RenderLayer, RenderStamp, RenderSurface,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Compiler {
    desktop: SceneCompiler,
    recording: SceneCompiler,
    assets: HashMap<(bool, u64), u64>,
    next_asset: u64,
}
pub(super) struct Input<'a> {
    pub desktop: &'a Session,
    pub recording: &'a Session,
    pub draft: Option<&'a Object>,
    pub hovered: Option<usize>,
    pub stamp: RenderStamp,
    pub surface: RenderSurface,
    pub excluded: &'a [lexift_core::ports::magnifier::WindowToken],
}
pub(super) struct Frame {
    pub content: AnnotationScene,
    pub interaction: AnnotationScene,
    pub effects: Vec<(ObjectId, Vec<Rect>)>,
}
impl Compiler {
    fn asset(&mut self, desktop: bool, layer: &RasterLayer) -> RasterLayer {
        let asset = *self
            .assets
            .entry((desktop, layer.key.asset))
            .or_insert_with(|| {
                self.next_asset += 1;
                self.next_asset
            });
        RasterLayer {
            key: RasterKey {
                asset,
                revision: layer.key.revision,
            },
            bounds: layer.bounds,
            pixels: layer.pixels.clone(),
        }
    }
    fn layer(&mut self, desktop: bool, layer: &RenderLayer) -> RenderLayer {
        match layer {
            RenderLayer::Raster(layer) => RenderLayer::Raster(self.asset(desktop, layer)),
            RenderLayer::Mosaic(layer) => {
                RenderLayer::Mosaic(lexift_core::ports::annotation_render::MosaicLayer {
                    object: layer.object.clone(),
                    masks: layer.masks.iter().map(|l| self.asset(desktop, l)).collect(),
                })
            }
            RenderLayer::Magnifier(layer) => {
                RenderLayer::Magnifier(lexift_core::ports::annotation_render::MagnifierLayer {
                    object: layer.object.clone(),
                    masks: layer.masks.iter().map(|l| self.asset(desktop, l)).collect(),
                    decorations: layer
                        .decorations
                        .iter()
                        .map(|l| self.asset(desktop, l))
                        .collect(),
                })
            }
        }
    }
    pub fn compile(&mut self, input: Input<'_>) -> Option<Frame> {
        let surfaces = [input.surface];
        let scene = |session, draft, hovered| SceneInput {
            edit: None,
            stamp: input.stamp,
            session,
            surfaces: &surfaces,
            draft,
            hovered,
            reference: None,
            editor: None,
            paused_regions: &[],
            excluded_windows: input.excluded,
        };
        let desktop = self.desktop.compile(scene(input.desktop, None, None))?;
        // Unconfirmed text belongs to the excluded native input window. Other
        // drawing previews are visible ink and therefore belong in the video.
        let draft = input.draft.filter(|o| o.kind != Kind::Text);
        let recording = self
            .recording
            .compile(scene(input.recording, draft, input.hovered))?;
        // Completion describes output coverage, not the larger filter input.
        // Take it from the submitted masks, including brush partition/clipping.
        let effects = recording.layers.iter().filter_map(|layer| match layer {
            RenderLayer::Mosaic(layer) if matches!(&layer.object.extra, Extra::Mosaic(m) if m.strength > 0) =>
                Some((layer.object.id, layer.masks.iter().map(|mask| mask.bounds).collect())),
            _ => None,
        }).collect();
        let mut layers = Vec::with_capacity(desktop.layers.len() + recording.layers.len());
        for layer in &*desktop.layers {
            layers.push(self.layer(true, layer));
        }
        for layer in &*recording.layers {
            layers.push(self.layer(false, layer));
        }
        let interaction = AnnotationScene {
            stamp: input.stamp,
            surfaces: Arc::from(surfaces),
            layers: Arc::from([]),
            interaction: recording.interaction.clone(),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from(input.excluded),
        };
        let content = AnnotationScene {
            stamp: input.stamp,
            surfaces: Arc::from(surfaces),
            layers: Arc::from(layers),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from(input.excluded),
        };
        Some(Frame {
            content,
            interaction,
            effects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::ports::magnifier::WindowToken;
    fn document(color: [u8; 3]) -> Session {
        let mut session = Session::new();
        session.add(Object {
            image_basis: Default::default(),
            erased: vec![],
            id: ObjectId::DRAFT,
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((10., 10.), (60., 60.)),
            style: Style {
                color,
                ..Default::default()
            },
            points: vec![],
            extra: Extra::None,
        });
        session
    }
    #[test]
    fn effect_confirmation_uses_output_mask_without_filter_padding() {
        use lexift_core::domain::annotation::{MosaicAnnotation, MosaicEffect};
        let mut recording = Session::new();
        let mut settings = MosaicAnnotation::new(MosaicEffect::Pixelate, 60, 12., 1.);
        settings.id = 1;
        recording.add(Object {
            image_basis: Default::default(),
            erased: vec![],
            id: ObjectId::DRAFT,
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((10., 10.), (60., 60.)),
            style: Style::default(),
            points: vec![],
            extra: Extra::Mosaic(settings),
        });
        let region = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let frame = Compiler::default()
            .compile(Input {
                desktop: &Session::new(),
                recording: &recording,
                draft: None,
                hovered: None,
                stamp: RenderStamp {
                    session: 1,
                    revision: 1,
                },
                surface: RenderSurface {
                    input_window: WindowToken(1),
                    bounds: region,
                    scale: 1.,
                },
                excluded: &[],
            })
            .unwrap();
        let visible = Rect {
            left: 10,
            top: 10,
            right: 60,
            bottom: 60,
        };
        assert_eq!(
            frame.effects,
            vec![(recording.objects[0].id, vec![visible])]
        );
        let sample =
            crate::annotation_render::mosaic::sample_bounds(&recording.objects[0], [region])
                .unwrap();
        assert!(sample.left < visible.left && sample.right > visible.right);
    }
    #[test]
    fn recording_ink_and_desktop_use_distinct_assets_and_feedback_never_enters_content() {
        let desktop = document([255, 0, 0]);
        let recording = document([0, 255, 0]);
        let original = recording.clone();
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let mut compiler = Compiler::default();
        let input = |revision| Input {
            desktop: &desktop,
            recording: &recording,
            draft: None,
            hovered: Some(0),
            stamp: RenderStamp {
                session: 4,
                revision,
            },
            surface: RenderSurface {
                input_window: WindowToken(1),
                bounds,
                scale: 1.25,
            },
            excluded: &[],
        };
        let first = compiler.compile(input(1)).unwrap();
        assert!(first.content.interaction.is_empty());
        assert!(!first.interaction.interaction.is_empty());
        assert!(first.interaction.layers.is_empty());
        let keys: Vec<_> = first
            .content
            .layers
            .iter()
            .flat_map(RenderLayer::rasters)
            .map(|l| l.key)
            .collect();
        let unique: std::collections::HashSet<_> = keys.iter().copied().collect();
        assert_eq!(keys.len(), unique.len());
        let second = compiler.compile(input(2)).unwrap();
        for (a, b) in first
            .content
            .layers
            .iter()
            .flat_map(RenderLayer::rasters)
            .zip(second.content.layers.iter().flat_map(RenderLayer::rasters))
        {
            assert_eq!(a.key, b.key);
            assert!(Arc::ptr_eq(&a.pixels, &b.pixels));
        }
        assert_eq!(recording.objects, original.objects);
        assert_eq!(recording.can_undo(), original.can_undo());
    }
}
