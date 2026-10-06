//! Cached local effect workspaces. Captured pixels never leave the device.
use super::{
    blur,
    content::{
        Dependency, Output, PresentationInputs, SampleRule, SourceChanges, TileInput, TileSettings,
    },
    graphics::{Blit, Graphics, Texture},
    lens,
};
use lexift_core::{
    domain::{
        annotation::{Extra, MosaicAnnotation, MosaicEffect},
        geometry::Rect,
    },
    ports::annotation_render::*,
};
use std::collections::{HashMap, HashSet};

pub(super) struct CaptureLayer {
    #[cfg(test)]
    pub source: Option<super::super::screen_capture::diagnostics::Published>,
    pub bounds: Rect,
    pub opacity: u8,
    pub texture: Texture,
}

struct Captured<'a> {
    layers: &'a [CaptureLayer],
    changes: SourceChanges<'a>,
}

struct Tile {
    bounds: Rect,
    sample: Rect,
    block: u32,
    source: Texture,
    grid: Option<Texture>,
    blur: Option<blur::Workspace>,
    filtered: Texture,
    output: Texture,
    valid: bool,
}

#[derive(Default)]
pub(super) struct Effects {
    #[cfg(test)]
    source_frames: HashMap<Output, Vec<super::super::screen_capture::diagnostics::Published>>,
    tiles: HashMap<u64, Tile>,
    plans: HashMap<u32, blur::Plan>,
    lenses: HashMap<u64, lens::Tile>,
    inputs: HashMap<Output, TileInput>,
    versions: HashMap<Output, u64>,
    source_revision: u64,
    source_versions: HashMap<Output, u64>,
    output_revision: u64,
}

/// A paused effect still owns its last output; zero strength owns no effect work.
fn active_mosaic(layer: &RenderLayer) -> Option<(&MosaicLayer, &MosaicAnnotation)> {
    let RenderLayer::Mosaic(layer) = layer else {
        return None;
    };
    let Extra::Mosaic(settings) = &layer.object.extra else {
        return None;
    };
    (settings.strength != 0 && !layer.masks.is_empty()).then_some((layer, settings))
}

fn block(layer: &MosaicLayer) -> u32 {
    match &layer.object.extra {
        Extra::Mosaic(settings) => settings.block_size(),
        _ => 0,
    }
}

pub(super) fn sample(bounds: Rect, block: u32) -> Rect {
    let b = i64::from(block.max(1));
    let align = |value: i32, ceil: bool| {
        let value = i64::from(value);
        let cell = if ceil {
            -(-value).div_euclid(b)
        } else {
            value.div_euclid(b)
        };
        (cell * b).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    Rect {
        left: align(bounds.left, false),
        top: align(bounds.top, false),
        right: align(bounds.right, true),
        bottom: align(bounds.bottom, true),
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

impl Effects {
    fn dependencies(
        &self,
        scene: &AnnotationScene,
        rule: SampleRule,
        sample: Rect,
    ) -> Vec<Dependency> {
        let mut dependencies = Vec::new();
        for layer in rule.layers(scene) {
            let mut raster = |layer: &RasterLayer| {
                if overlaps(sample, layer.bounds) {
                    dependencies.push(Dependency::Raster(layer.key, layer.bounds));
                }
            };
            match layer {
                RenderLayer::Raster(layer) => raster(layer),
                RenderLayer::Mosaic(layer) if block(layer) != 0 => {
                    for mask in &*layer.masks {
                        if let Some(tile) = self
                            .tiles
                            .get(&mask.key.asset)
                            .filter(|tile| tile.valid && overlaps(sample, tile.bounds))
                        {
                            let output = Output::Mosaic(mask.key.asset);
                            dependencies.push(Dependency::Effect(
                                output,
                                self.versions.get(&output).copied().unwrap_or(0),
                                tile.bounds,
                            ));
                        }
                    }
                }
                RenderLayer::Magnifier(layer) => {
                    for decoration in &*layer.decorations {
                        raster(decoration);
                    }
                    for mask in &*layer.masks {
                        if let Some(tile) = self
                            .lenses
                            .get(&mask.key.asset)
                            .filter(|tile| tile.valid && overlaps(sample, tile.bounds))
                        {
                            let output = Output::Lens(mask.key.asset);
                            dependencies.push(Dependency::Effect(
                                output,
                                self.versions.get(&output).copied().unwrap_or(0),
                                tile.bounds,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        dependencies
    }

    fn input(
        &self,
        scene: &AnnotationScene,
        object: &lexift_core::domain::annotation::Object,
        mask: &RasterLayer,
        sample: Rect,
        dependencies: Vec<Dependency>,
    ) -> TileInput {
        TileInput {
            session: scene.stamp.session,
            source_revision: self
                .source_versions
                .get(&if matches!(object.extra, Extra::Magnifier { .. }) {
                    Output::Lens(mask.key.asset)
                } else {
                    Output::Mosaic(mask.key.asset)
                })
                .copied()
                .unwrap_or(0),
            owner: object.id,
            geometry: matches!(object.extra, Extra::Magnifier { .. }).then_some(object.bounds),
            settings: TileSettings::from_object(object),
            mask: (mask.key, mask.bounds),
            sample,
            dependencies,
        }
    }

    fn completed(&mut self, output: Output, input: TileInput) {
        self.output_revision = self
            .output_revision
            .checked_add(1)
            .expect("effect output revision exhausted");
        self.versions.insert(output, self.output_revision);
        self.inputs.insert(output, input);
    }
    pub fn ready(&self, scene: &AnnotationScene) -> bool {
        scene.layers.iter().all(|layer| match layer {
            RenderLayer::Mosaic(layer) if matches!(&layer.object.extra, Extra::Mosaic(settings) if settings.strength > 0) => layer.masks.iter().all(|mask| self.tiles.get(&mask.key.asset).is_some_and(|tile| tile.valid && tile.bounds == mask.bounds)),
            RenderLayer::Magnifier(layer) => layer.masks.iter().all(|mask| self.lenses.get(&mask.key.asset).is_some_and(|tile| tile.valid && tile.bounds == mask.bounds)),
            _ => true,
        })
    }
    /// Only initialized outputs can confirm a visible effect. A new paused
    /// tile owns textures but has never sampled or filtered their contents.
    pub fn completed_regions(&self, scene: &AnnotationScene) -> Vec<Rect> {
        scene
            .layers
            .iter()
            .flat_map(|layer| match layer {
                RenderLayer::Mosaic(layer) => layer
                    .masks
                    .iter()
                    .filter_map(|mask| {
                        self.tiles
                            .get(&mask.key.asset)
                            .filter(|tile| tile.valid && tile.bounds == mask.bounds)
                            .map(|tile| tile.bounds)
                    })
                    .collect::<Vec<_>>(),
                RenderLayer::Magnifier(layer) => layer
                    .masks
                    .iter()
                    .filter_map(|mask| {
                        self.lenses
                            .get(&mask.key.asset)
                            .filter(|tile| tile.valid && tile.bounds == mask.bounds)
                            .map(|tile| tile.bounds)
                    })
                    .collect(),
                RenderLayer::Raster(_) => Vec::new(),
            })
            .collect()
    }

    fn bounds(&mut self, bounds: Rect, settings: &MosaicAnnotation) -> (Rect, Option<blur::Plan>) {
        if settings.effect == MosaicEffect::Pixelate {
            return (sample(bounds, settings.block_size()), None);
        }
        let sigma = settings.blur_sigma();
        let plan = self
            .plans
            .entry(sigma.to_bits())
            .or_insert_with(|| blur::Plan::new(sigma))
            .clone();
        let padding = plan.padding();
        let expanded = Rect {
            left: bounds.left.saturating_sub(padding),
            top: bounds.top.saturating_sub(padding),
            right: bounds.right.saturating_add(padding),
            bottom: bounds.bottom.saturating_add(padding),
        };
        (sample(expanded, plan.alignment() as u32), Some(plan))
    }

    fn retire_plans(&mut self, scene: &AnnotationScene) {
        let used: HashSet<_> = scene
            .layers
            .iter()
            .filter_map(active_mosaic)
            .filter(|(_, settings)| settings.effect == MosaicEffect::Blur)
            .map(|(_, settings)| settings.blur_sigma().to_bits())
            .collect();
        self.plans.retain(|sigma, _| used.contains(sigma));
    }

    pub fn regions(&mut self, scene: &AnnotationScene) -> Vec<Rect> {
        self.retire_plans(scene);
        let mut regions = Vec::new();
        for layer in &*scene.layers {
            if let RenderLayer::Magnifier(layer) = layer {
                regions.extend(
                    layer
                        .masks
                        .iter()
                        .map(|mask| lens::source_for(&layer.object, mask.bounds))
                        .filter(|source| {
                            !scene
                                .paused_regions
                                .iter()
                                .any(|paused| overlaps(*paused, *source))
                        }),
                );
                continue;
            }
            if let Some((layer, settings)) = active_mosaic(layer) {
                for mask in &*layer.masks {
                    let source = self.bounds(mask.bounds, settings).0;
                    if !scene
                        .paused_regions
                        .iter()
                        .any(|paused| overlaps(*paused, source))
                    {
                        regions.push(source);
                    }
                }
            }
        }
        regions
    }

    pub fn update<'a>(
        &mut self,
        graphics: &Graphics,
        scene: &AnnotationScene,
        raster: &HashMap<RasterKey, Texture>,
        captured: &[CaptureLayer],
        source_changed: impl Into<SourceChanges<'a>>,
    ) -> windows::core::Result<bool> {
        self.retire_plans(scene);
        let source_changed = source_changed.into();
        if source_changed.any() {
            self.source_revision = self
                .source_revision
                .checked_add(1)
                .expect("effect source revision exhausted");
        }
        let mut updated = false;
        let used: HashSet<_> = scene
            .layers
            .iter()
            .filter_map(active_mosaic)
            .flat_map(|(layer, _)| layer.masks.iter().map(|mask| mask.key.asset))
            .collect();
        self.tiles.retain(|key, _| used.contains(key));
        let lenses: HashSet<_> = scene
            .layers
            .iter()
            .flat_map(|layer| match layer {
                RenderLayer::Magnifier(layer) => {
                    layer.masks.iter().map(|mask| mask.key.asset).collect()
                }
                _ => Vec::new(),
            })
            .collect();
        self.lenses.retain(|key, _| lenses.contains(key));
        let retained = |output: &Output| match output {
            Output::Mosaic(asset) => used.contains(asset),
            Output::Lens(asset) => lenses.contains(asset),
        };
        self.inputs.retain(|output, _| retained(output));
        self.versions.retain(|output, _| retained(output));
        self.source_versions.retain(|output, _| retained(output));
        #[cfg(test)]
        self.source_frames.retain(|output, _| retained(output));
        for (index, layer) in scene.layers.iter().enumerate() {
            if let RenderLayer::Magnifier(layer) = layer {
                updated |= self.update_lens(
                    graphics,
                    scene,
                    index,
                    layer,
                    raster,
                    Captured {
                        layers: captured,
                        changes: source_changed,
                    },
                )?;
                continue;
            }
            let RenderLayer::Mosaic(layer) = layer else {
                continue;
            };
            let Extra::Mosaic(settings) = &layer.object.extra else {
                continue;
            };
            if settings.strength == 0 {
                continue;
            }
            for mask in &*layer.masks {
                let b = if settings.effect == MosaicEffect::Pixelate {
                    block(layer)
                } else {
                    0
                };
                let (bounds, plan) = self.bounds(mask.bounds, settings);
                let mut tile = self
                    .tiles
                    .remove(&mask.key.asset)
                    .and_then(|mut tile| {
                        tile.reuse(mask.bounds, bounds, b, plan.as_ref())
                            .then_some(tile)
                    })
                    .map(Ok)
                    .unwrap_or_else(|| {
                        Tile::new(graphics, mask.bounds, bounds, b, plan.as_ref())
                    })?;
                let output = Output::Mosaic(mask.key.asset);
                if source_changed.intersects(bounds) {
                    self.source_versions.insert(output, self.source_revision);
                }
                // A covered tile retains its previous complete result. Never
                // publish an uninitialized texture during popup interaction.
                if scene
                    .paused_regions
                    .iter()
                    .any(|paused| overlaps(*paused, bounds))
                {
                    self.tiles.insert(mask.key.asset, tile);
                    continue;
                }
                let input = self.input(
                    scene,
                    &layer.object,
                    mask,
                    bounds,
                    self.dependencies(scene, SampleRule::Below(index), bounds),
                );
                if tile.valid && self.inputs.get(&output) == Some(&input) {
                    self.tiles.insert(mask.key.asset, tile);
                    continue;
                }
                graphics.clear(&tile.source);
                draw_capture(graphics, captured, &tile.source, bounds);
                for lower in SampleRule::Below(index).layers(scene) {
                    self.draw_layer(graphics, lower, raster, &tile.source, bounds);
                }
                if let Some(workspace) = &tile.blur {
                    workspace.render(graphics, &tile.source, &tile.filtered);
                } else {
                    graphics.pixelate(
                        &tile.source,
                        bounds,
                        b,
                        tile.grid.as_ref().unwrap(),
                        &tile.filtered,
                    );
                }
                graphics.mask(
                    &tile.filtered,
                    bounds,
                    &raster[&mask.key],
                    mask.bounds,
                    &tile.output,
                );
                tile.valid = true;
                #[cfg(test)]
                self.remember_sources(output, &input, captured);
                self.completed(output, input);
                updated = true;
                self.tiles.insert(mask.key.asset, tile);
            }
        }
        Ok(updated)
    }

    fn update_lens(
        &mut self,
        graphics: &Graphics,
        scene: &AnnotationScene,
        index: usize,
        layer: &MagnifierLayer,
        raster: &HashMap<RasterKey, Texture>,
        capture: Captured<'_>,
    ) -> windows::core::Result<bool> {
        let Captured {
            layers: captured,
            changes: source_changed,
        } = capture;
        let mut updated = false;
        let Extra::Magnifier {
            erase_annotations, ..
        } = layer.object.extra
        else {
            unreachable!()
        };
        for mask in &*layer.masks {
            let source = lens::source_for(&layer.object, mask.bounds);
            let output = Output::Lens(mask.key.asset);
            if source_changed.intersects(source) {
                self.source_versions.insert(output, self.source_revision);
            }
            let mut tile = self
                .lenses
                .remove(&mask.key.asset)
                .and_then(|mut tile| tile.reuse(mask.bounds, source).then_some(tile))
                .map(Ok)
                .unwrap_or_else(|| lens::Tile::new(graphics, mask.bounds, source))?;
            if scene
                .paused_regions
                .iter()
                .any(|paused| overlaps(*paused, source))
            {
                self.lenses.insert(mask.key.asset, tile);
                continue;
            }
            let input = self.input(
                scene,
                &layer.object,
                mask,
                source,
                self.dependencies(
                    scene,
                    SampleRule::Lens {
                        index,
                        erase: erase_annotations,
                    },
                    source,
                ),
            );
            if tile.valid && self.inputs.get(&output) == Some(&input) {
                self.lenses.insert(mask.key.asset, tile);
                continue;
            }
            graphics.clear(&tile.source);
            draw_capture(graphics, captured, &tile.source, source);
            if !erase_annotations {
                // The existing lens displays all ordinary marks, earlier
                // privacy effects, and no other lens. Preserve that rule while
                // keeping caret/selection out of the captured image.
                for content in (SampleRule::Lens {
                    index,
                    erase: erase_annotations,
                })
                .layers(scene)
                {
                    self.draw_layer(graphics, content, raster, &tile.source, source);
                }
            }
            tile.render(graphics, &layer.object, mask, &raster[&mask.key]);
            #[cfg(test)]
            self.remember_sources(output, &input, captured);
            self.completed(output, input);
            updated = true;
            self.lenses.insert(mask.key.asset, tile);
        }
        Ok(updated)
    }

    /// The command stream is shared by display invalidation and actual composition.
    fn commands<'a>(&'a self, layer: &'a RenderLayer) -> impl Iterator<Item = Draw<'a>> {
        let (raster, masks, decorations, lens): (
            Option<&RasterLayer>,
            &[RasterLayer],
            &[RasterLayer],
            bool,
        ) = match layer {
            RenderLayer::Raster(layer) => (Some(layer), &[], &[], false),
            RenderLayer::Mosaic(layer) if block(layer) != 0 => (None, &layer.masks, &[], false),
            RenderLayer::Magnifier(layer) => (None, &layer.masks, &layer.decorations, true),
            _ => (None, &[], &[], false),
        };
        raster
            .into_iter()
            .map(Draw::Raster)
            .chain(masks.iter().filter_map(move |mask| {
                let output = if lens {
                    Output::Lens(mask.key.asset)
                } else {
                    Output::Mosaic(mask.key.asset)
                };
                let (texture, bounds) = if lens {
                    self.lenses
                        .get(&mask.key.asset)
                        .filter(|tile| tile.valid)
                        .map(|tile| (&tile.output, tile.bounds))?
                } else {
                    self.tiles
                        .get(&mask.key.asset)
                        .filter(|tile| tile.valid)
                        .map(|tile| (&tile.output, tile.bounds))?
                };
                Some(Draw::Effect(
                    output,
                    self.versions[&output],
                    texture,
                    bounds,
                ))
            }))
            .chain(decorations.iter().map(Draw::Raster))
    }

    /// Freeze only sources actually sampled by a recomputed effect. A paused
    /// lower effect keeps its own provenance instead of inheriting newer frames.
    #[cfg(test)]
    fn remember_sources(&mut self, output: Output, input: &TileInput, captured: &[CaptureLayer]) {
        let mut sources: Vec<_> = captured
            .iter()
            .filter(|layer| overlaps(input.sample, layer.bounds))
            .filter_map(|layer| layer.source)
            .collect();
        for dependency in &input.dependencies {
            if let Dependency::Effect(lower, _, _) = dependency {
                sources.extend(self.source_frames.get(lower).into_iter().flatten().copied());
            }
        }
        sources.sort_by_key(|source| (source.token, source.sequence));
        sources.dedup();
        self.source_frames.insert(output, sources);
    }

    pub fn presentation_inputs(&self, scene: &AnnotationScene) -> PresentationInputs {
        let commands: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|layer| self.commands(layer))
            .chain(scene.interaction.iter().map(Draw::Raster))
            .filter(|command| {
                scene
                    .surfaces
                    .iter()
                    .any(|surface| overlaps(command.bounds(), surface.bounds))
            })
            .map(|command| command.key())
            .collect();
        #[cfg(test)]
        let sources = {
            let mut sources: Vec<_> = commands
                .iter()
                .flat_map(|command| match command {
                    Dependency::Effect(output, _, _) => self
                        .source_frames
                        .get(output)
                        .into_iter()
                        .flatten()
                        .copied()
                        .collect(),
                    _ => Vec::new(),
                })
                .collect();
            sources.sort_by_key(|source| (source.token, source.sequence));
            sources.dedup();
            sources
        };
        PresentationInputs {
            #[cfg(test)]
            sources,
            session: scene.stamp.session,
            surfaces: scene.surfaces.clone(),
            commands,
        }
    }

    pub fn draw_layer(
        &self,
        graphics: &Graphics,
        layer: &RenderLayer,
        raster: &HashMap<RasterKey, Texture>,
        target: &Texture,
        bounds: Rect,
    ) {
        for command in self.commands(layer) {
            let texture = match command {
                Draw::Raster(layer) => &raster[&layer.key],
                Draw::Effect(_, _, texture, _) => texture,
            };
            blit(graphics, texture, command.bounds(), target, bounds, 1.);
        }
    }
}

#[derive(Clone, Copy)]
enum Draw<'a> {
    Raster(&'a RasterLayer),
    Effect(Output, u64, &'a Texture, Rect),
}
impl Draw<'_> {
    fn bounds(self) -> Rect {
        match self {
            Self::Raster(layer) => layer.bounds,
            Self::Effect(_, _, _, bounds) => bounds,
        }
    }
    fn key(self) -> Dependency {
        match self {
            Self::Raster(layer) => Dependency::Raster(layer.key, layer.bounds),
            Self::Effect(output, version, _, bounds) => Dependency::Effect(output, version, bounds),
        }
    }
}

fn draw_capture(graphics: &Graphics, captured: &[CaptureLayer], target: &Texture, bounds: Rect) {
    for capture in captured {
        graphics.blit(
            &capture.texture,
            target,
            bounds,
            Blit {
                destination: capture.bounds,
                source_rect: Rect {
                    left: 0,
                    top: 0,
                    right: capture.bounds.right - capture.bounds.left,
                    bottom: capture.bounds.bottom - capture.bounds.top,
                },
                opacity: f32::from(capture.opacity) / 255.,
                blend: true,
            },
        );
    }
}

impl Tile {
    fn reuse(&mut self, bounds: Rect, sample: Rect, block: u32, plan: Option<&blur::Plan>) -> bool {
        if !self.source.matches_bounds(sample) || !self.output.matches_bounds(bounds) {
            return false;
        }
        match (&mut self.blur, plan) {
            (Some(workspace), Some(plan)) => {
                if !workspace.reuse_plan(plan) {
                    return false;
                }
            }
            (None, None)
                if block != 0
                    && self.grid.as_ref().is_some_and(|grid| {
                        grid.width == self.source.width.div_ceil(block)
                            && grid.height == self.source.height.div_ceil(block)
                    }) => {}
            _ => return false,
        }
        if self.bounds != bounds || self.sample != sample {
            self.valid = false;
        }
        self.bounds = bounds;
        self.sample = sample;
        self.block = block;
        true
    }
    fn new(
        graphics: &Graphics,
        bounds: Rect,
        sample: Rect,
        block: u32,
        plan: Option<&blur::Plan>,
    ) -> windows::core::Result<Self> {
        let invalid =
            || windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG);
        let width = sample
            .right
            .checked_sub(sample.left)
            .filter(|n| *n > 0)
            .ok_or_else(invalid)? as u32;
        let height = sample
            .bottom
            .checked_sub(sample.top)
            .filter(|n| *n > 0)
            .ok_or_else(invalid)? as u32;
        let create = |w, h| Texture::create(&graphics.device, w, h, None);
        Ok(Self {
            bounds,
            sample,
            block,
            source: create(width, height)?,
            grid: (block != 0)
                .then(|| create(width.div_ceil(block), height.div_ceil(block)))
                .transpose()?,
            blur: plan
                .map(|plan| blur::Workspace::new(graphics, width, height, plan))
                .transpose()?,
            filtered: if plan.is_some() {
                Texture::filter_texture(&graphics.device, width, height)?
            } else {
                create(width, height)?
            },
            output: create(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            )?,
            valid: false,
        })
    }
}

pub(super) fn blit(
    graphics: &Graphics,
    source: &Texture,
    destination: Rect,
    target: &Texture,
    bounds: Rect,
    opacity: f32,
) {
    graphics.blit(
        source,
        target,
        bounds,
        Blit {
            destination,
            source_rect: Rect {
                left: 0,
                top: 0,
                right: source.width as i32,
                bottom: source.height as i32,
            },
            opacity,
            blend: true,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{Bounds, Kind, MosaicAnnotation, Object, Style};
    use std::sync::Arc;

    fn cache_scene(settings: &[(u64, MosaicEffect, u8)]) -> AnnotationScene {
        let bounds = Rect {
            left: -4,
            top: -4,
            right: 4,
            bottom: 4,
        };
        AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([]),
            layers: settings
                .iter()
                .map(|&(asset, effect, strength)| {
                    RenderLayer::Mosaic(MosaicLayer {
                        object: Arc::new(Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            bounds: Bounds::from_corners((-4., -4.), (4., 4.)),
                            kind: Kind::MosaicArea,
                            style: Style::default(),
                            points: Vec::new(),
                            extra: Extra::Mosaic(MosaicAnnotation::new(
                                effect, strength, 12., 1.25,
                            )),
                        }),
                        masks: vec![RasterLayer {
                            key: RasterKey { asset, revision: 1 },
                            bounds,
                            pixels: vec![255; 8 * 8 * 4].into(),
                        }]
                        .into(),
                    })
                })
                .collect::<Vec<_>>()
                .into(),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        }
    }

    #[test]
    fn blur_plans_follow_current_visible_effects_including_paused_ones() {
        let mut effects = Effects::default();
        for strength in [1, 4, 10, 30, 50, 75, 100] {
            let scene = cache_scene(&[(1, MosaicEffect::Blur, strength)]);
            assert_eq!(effects.regions(&scene).len(), 1);
            assert_eq!(effects.plans.len(), 1);
            let sigma = MosaicAnnotation::new(MosaicEffect::Blur, strength, 12., 1.25)
                .blur_sigma()
                .to_bits();
            assert!(effects.plans.contains_key(&sigma));
        }
        let mut scene = cache_scene(&[(1, MosaicEffect::Blur, 50), (2, MosaicEffect::Blur, 50)]);
        effects.regions(&scene);
        assert_eq!(effects.plans.len(), 1, "equal sigmas share calibration");
        scene.paused_regions = vec![Rect {
            left: -1000,
            top: -1000,
            right: 1000,
            bottom: 1000,
        }]
        .into();
        assert!(effects.regions(&scene).is_empty());
        assert_eq!(effects.plans.len(), 1, "pause retains the live plan");
        effects.regions(&cache_scene(&[
            (1, MosaicEffect::Blur, 0),
            (2, MosaicEffect::Pixelate, 50),
        ]));
        assert!(effects.plans.is_empty());
        let mut invisible = cache_scene(&[(1, MosaicEffect::Blur, 50)]);
        if let RenderLayer::Mosaic(layer) = &mut Arc::make_mut(&mut invisible.layers)[0] {
            layer.masks = Arc::from([]);
        }
        effects.regions(&invisible);
        assert!(effects.plans.is_empty(), "offscreen masks need no plan");
        effects.regions(&scene);
        effects.regions(&cache_scene(&[]));
        assert!(effects.plans.is_empty(), "deletion retires calibration");
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_effect_cache_retires_disabled_outputs_and_preserves_paused_pixels() {
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let scene = cache_scene(&[(7, MosaicEffect::Blur, 50)]);
        let mask = scene.layers[0].rasters().next().unwrap();
        let raster = HashMap::from([(
            mask.key,
            Texture::create(&graphics.device, 8, 8, Some(&mask.pixels)).unwrap(),
        )]);
        let captured = [CaptureLayer {
            source: None,
            bounds: Rect {
                left: -64,
                top: -64,
                right: 64,
                bottom: 64,
            },
            opacity: 255,
            texture: Texture::create(
                &graphics.device,
                128,
                128,
                Some(&[20, 40, 80, 255].repeat(128 * 128)),
            )
            .unwrap(),
        }];
        let output = Output::Mosaic(7);
        let mut effects = Effects::default();
        assert!(
            effects
                .update(&graphics, &scene, &raster, &captured, true)
                .unwrap()
        );
        let original =
            super::super::graphics::tests::readback(&graphics, &effects.tiles[&7].output);
        let version = effects.versions[&output];
        let mut paused = scene.clone();
        paused.paused_regions = vec![captured[0].bounds].into();
        assert!(
            !effects
                .update(&graphics, &paused, &raster, &captured, true)
                .unwrap()
        );
        assert!(effects.tiles[&7].valid);
        assert_eq!(effects.versions[&output], version);
        assert_eq!(
            super::super::graphics::tests::readback(&graphics, &effects.tiles[&7].output),
            original
        );
        assert_eq!(effects.plans.len(), 1);

        let disabled = cache_scene(&[(7, MosaicEffect::Blur, 0)]);
        effects
            .update(&graphics, &disabled, &raster, &captured, false)
            .unwrap();
        assert!(effects.tiles.is_empty());
        assert!(effects.plans.is_empty());
        assert!(effects.inputs.is_empty());
        assert!(effects.versions.is_empty());
        assert!(effects.source_versions.is_empty());
        assert!(effects.source_frames.is_empty());
        assert!(effects.commands(&disabled.layers[0]).next().is_none());
        assert!(
            effects
                .update(&graphics, &scene, &raster, &captured, false)
                .unwrap()
        );
        assert_eq!(
            super::super::graphics::tests::readback(&graphics, &effects.tiles[&7].output),
            original
        );
        assert!(effects.versions[&output] > version);
        effects
            .update(
                &graphics,
                &cache_scene(&[(7, MosaicEffect::Pixelate, 50)]),
                &raster,
                &captured,
                false,
            )
            .unwrap();
        assert!(effects.plans.is_empty());
        assert!(effects.tiles[&7].blur.is_none());
        assert!(effects.tiles[&7].valid);
        effects
            .update(&graphics, &cache_scene(&[]), &raster, &captured, false)
            .unwrap();
        assert!(effects.tiles.is_empty());
        assert!(effects.inputs.is_empty());
        assert!(effects.versions.is_empty());
        assert!(effects.source_versions.is_empty());
        assert!(effects.source_frames.is_empty());
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_lenses_preserve_erase_nonrecursive_sampling_and_effect_order() {
        use lexift_core::domain::annotation::MagnifierConnector;
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let source = Rect {
            left: -4,
            top: -4,
            right: 0,
            bottom: 0,
        };
        let first_output = Rect {
            left: 8,
            top: 0,
            right: 16,
            bottom: 8,
        };
        let second_source = Rect {
            left: 8,
            top: 0,
            right: 12,
            bottom: 4,
        };
        let second_output = Rect {
            left: 20,
            top: 0,
            right: 28,
            bottom: 8,
        };
        let raster = |asset, bounds: Rect, color: [u8; 4]| RasterLayer {
            key: RasterKey { asset, revision: 1 },
            bounds,
            pixels: color
                .repeat(((bounds.right - bounds.left) * (bounds.bottom - bounds.top)) as usize)
                .into(),
        };
        let bounds = |rect: Rect| {
            Bounds::from_corners(
                (rect.left as f32, rect.top as f32),
                (rect.right as f32, rect.bottom as f32),
            )
        };
        let mosaic = |asset, body: Rect| {
            RenderLayer::Mosaic(MosaicLayer {
                object: Arc::new(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    bounds: bounds(body),
                    kind: Kind::MosaicArea,
                    style: Style::default(),
                    points: Vec::new(),
                    extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 12., 1.)),
                }),
                masks: vec![raster(asset, body, [255; 4])].into(),
            })
        };
        let lens = |asset, source: Rect, output: Rect, decorations| {
            RenderLayer::Magnifier(MagnifierLayer {
                object: Arc::new(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    bounds: bounds(source),
                    kind: Kind::Magnifier,
                    style: Style::default(),
                    points: Vec::new(),
                    extra: Extra::Magnifier {
                        output: bounds(output),
                        zoom: 2.,
                        ellipse: true,
                        connector: MagnifierConnector::None,
                        erase_annotations: false,
                        antialias: false,
                        shadow: false,
                    },
                }),
                masks: vec![raster(asset, output, [255; 4])].into(),
                decorations,
            })
        };
        let decoration = raster(
            7,
            Rect {
                right: 9,
                bottom: 1,
                ..first_output
            },
            [200, 200, 0, 255],
        );
        let mut scene = AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([]),
            layers: vec![
                RenderLayer::Raster(raster(
                    1,
                    Rect {
                        right: -2,
                        ..source
                    },
                    [200, 0, 0, 255],
                )),
                mosaic(2, source),
                lens(3, source, first_output, vec![decoration.clone()].into()),
                RenderLayer::Raster(raster(4, Rect { left: -2, ..source }, [0, 200, 0, 255])),
                lens(5, second_source, second_output, Arc::from([])),
                mosaic(6, first_output),
            ]
            .into(),
            interaction: vec![raster(8, source, [200, 0, 200, 255])].into(),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        scene.validate().unwrap();
        let mut uploaded = HashMap::new();
        for layer in scene
            .layers
            .iter()
            .flat_map(RenderLayer::rasters)
            .chain(scene.interaction.iter())
        {
            uploaded.insert(
                layer.key,
                Texture::create(
                    &graphics.device,
                    (layer.bounds.right - layer.bounds.left) as u32,
                    (layer.bounds.bottom - layer.bounds.top) as u32,
                    Some(&layer.pixels),
                )
                .unwrap(),
            );
        }
        let captured: Vec<_> = [source, second_source]
            .into_iter()
            .map(|bounds| CaptureLayer {
                source: None,
                bounds,
                opacity: 255,
                texture: Texture::create(
                    &graphics.device,
                    4,
                    4,
                    Some(&[0, 0, 200, 255].repeat(16)),
                )
                .unwrap(),
            })
            .collect();
        let mut effects = Effects::default();
        let mut paused = scene.clone();
        paused.paused_regions = vec![Rect {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        }]
        .into();
        effects
            .update(&graphics, &paused, &uploaded, &captured, true)
            .unwrap();
        assert!(effects.completed_regions(&paused).is_empty());
        assert!(!effects.ready(&paused));
        assert!(effects.regions(&paused).is_empty());
        assert!(effects.tiles.values().all(|tile| !tile.valid));
        assert!(effects.lenses.values().all(|tile| !tile.valid));
        effects
            .update(&graphics, &scene, &uploaded, &captured, true)
            .unwrap();
        assert_eq!(effects.completed_regions(&scene).len(), 4);
        let revisions = effects.versions.clone();
        let mut decoration_only = scene.clone();
        let mut layers = decoration_only.layers.to_vec();
        for layer in &mut layers {
            if let RenderLayer::Magnifier(layer) = layer
                && let Extra::Magnifier { shadow, .. } = &mut Arc::make_mut(&mut layer.object).extra
            {
                *shadow = !*shadow;
            }
        }
        decoration_only.layers = layers.into();
        assert!(
            !effects
                .update(&graphics, &decoration_only, &uploaded, &captured, false)
                .unwrap(),
            "lens decorations must not invalidate sampled lens pixels"
        );
        assert_eq!(effects.versions, revisions);
        assert!(effects.ready(&scene));
        let read = |texture: &Texture| super::super::graphics::tests::readback(&graphics, texture);
        let first = read(&effects.lenses[&3].output);
        assert_eq!(
            &first[(4 * 8 + 2) * 4..(4 * 8 + 2) * 4 + 4],
            &[200, 0, 0, 255]
        );
        assert_eq!(
            &first[(4 * 8 + 6) * 4..(4 * 8 + 6) * 4 + 4],
            &[0, 200, 0, 255],
            "later ordinary marks preserve existing lens behavior"
        );
        assert_eq!(
            &read(&effects.lenses[&5].output)[..4],
            &[0, 0, 200, 255],
            "lenses never sample one another or their decorations"
        );
        let above = read(&effects.tiles[&6].output);
        assert_eq!(
            &above[(4 * 8 + 6) * 4..(4 * 8 + 6) * 4 + 4],
            &[0, 200, 0, 255],
            "effect above the lens must include enlarged marks"
        );
        let mut layers = scene.layers.to_vec();
        if let RenderLayer::Magnifier(layer) = &mut layers[2]
            && let Extra::Magnifier {
                erase_annotations, ..
            } = &mut Arc::make_mut(&mut layer.object).extra
        {
            *erase_annotations = true;
        }
        scene.layers = layers.into();
        scene.stamp.revision = 2;
        effects
            .update(&graphics, &scene, &uploaded, &captured, true)
            .unwrap();
        assert_eq!(
            &read(&effects.lenses[&3].output)[..4],
            &[0, 0, 200, 255],
            "erase shows only clean source pixels"
        );
        let target = Texture::create(&graphics.device, 8, 8, None).unwrap();
        graphics.clear(&target);
        effects.draw_layer(
            &graphics,
            &scene.layers[2],
            &uploaded,
            &target,
            first_output,
        );
        assert_eq!(
            &read(&target)[..4],
            &[200, 200, 0, 255],
            "decorations draw after lens content even in erase mode"
        );
    }

    #[test]
    fn grid_alignment_covers_negative_mask_edges_without_coordinate_overflow() {
        assert_eq!(
            sample(
                Rect {
                    left: -13,
                    top: -7,
                    right: 10,
                    bottom: 10
                },
                3
            ),
            Rect {
                left: -15,
                top: -9,
                right: 12,
                bottom: 12
            }
        );
        let extreme = sample(
            Rect {
                left: i32::MIN,
                top: i32::MIN,
                right: i32::MAX,
                bottom: i32::MAX,
            },
            80,
        );
        assert_eq!(extreme.left, i32::MIN);
        assert_eq!(extreme.right, i32::MAX);
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_effect_samples_lower_marks_and_keeps_upper_marks_clear() {
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 8,
            bottom: 8,
        };
        let raster_layer = |asset, bounds: Rect, color: [u8; 4]| RasterLayer {
            key: RasterKey { asset, revision: 1 },
            bounds,
            pixels: color
                .repeat(((bounds.right - bounds.left) * (bounds.bottom - bounds.top)) as usize)
                .into(),
        };
        let lower = raster_layer(1, Rect { right: 4, ..bounds }, [200, 0, 0, 255]);
        let mask = raster_layer(2, bounds, [128; 4]);
        let upper = raster_layer(3, Rect { left: 6, ..bounds }, [0, 200, 0, 255]);
        let interaction = raster_layer(
            4,
            Rect {
                right: 1,
                bottom: 1,
                ..bounds
            },
            [200, 200, 0, 255],
        );
        let scene = AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([]),
            layers: vec![
                RenderLayer::Raster(lower.clone()),
                RenderLayer::Mosaic(MosaicLayer {
                    object: Arc::new(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        bounds: Bounds::from_corners((0., 0.), (8., 8.)),
                        kind: Kind::MosaicArea,
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::Mosaic(MosaicAnnotation::new(
                            MosaicEffect::Pixelate,
                            1,
                            12.,
                            1.,
                        )),
                    }),
                    masks: vec![mask.clone()].into(),
                }),
                RenderLayer::Raster(upper.clone()),
            ]
            .into(),
            interaction: vec![interaction.clone()].into(),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        scene.validate().unwrap();
        let mut raster = HashMap::new();
        for layer in [&lower, &mask, &upper, &interaction] {
            raster.insert(
                layer.key,
                Texture::create(
                    &graphics.device,
                    (layer.bounds.right - layer.bounds.left) as u32,
                    (layer.bounds.bottom - layer.bounds.top) as u32,
                    Some(&layer.pixels),
                )
                .unwrap(),
            );
        }
        let captured = [CaptureLayer {
            source: None,
            bounds,
            opacity: 255,
            texture: Texture::create(&graphics.device, 8, 8, Some(&[0, 0, 200, 255].repeat(64)))
                .unwrap(),
        }];
        let mut effects = Effects::default();
        let mut paused = scene.clone();
        paused.paused_regions = vec![bounds].into();
        effects
            .update(&graphics, &paused, &raster, &captured, true)
            .unwrap();
        assert!(effects.completed_regions(&paused).is_empty());
        assert!(!effects.ready(&paused));
        assert!(effects.regions(&paused).is_empty());
        assert!(!effects.tiles[&mask.key.asset].valid);
        effects
            .update(&graphics, &scene, &raster, &captured, true)
            .unwrap();
        assert_eq!(effects.completed_regions(&scene), vec![bounds]);
        assert!(effects.ready(&scene));
        let effect_pixels = super::super::graphics::tests::readback(
            &graphics,
            &effects.tiles[&mask.key.asset].output,
        );
        assert_eq!(
            &effect_pixels[..4],
            &[100, 0, 0, 128],
            "lower red mark is sampled; caret is absent"
        );
        assert_eq!(
            &effect_pixels[4 * 6..4 * 7],
            &[0, 0, 100, 128],
            "upper green mark is absent from sampling"
        );
        let target = Texture::create(&graphics.device, 8, 8, None).unwrap();
        graphics.clear(&target);
        for layer in &*scene.layers {
            effects.draw_layer(&graphics, layer, &raster, &target, bounds);
        }
        blit(
            &graphics,
            &raster[&interaction.key],
            interaction.bounds,
            &target,
            bounds,
            1.,
        );
        let pixels = super::super::graphics::tests::readback(&graphics, &target);
        assert_eq!(&pixels[..4], &[200, 200, 0, 255]);
        assert_eq!(&pixels[4 * 6..4 * 7], &[0, 200, 0, 255]);
        let original_version = effects.versions[&Output::Mosaic(mask.key.asset)];
        let mut changed_upper = scene.clone();
        let mut layers = changed_upper.layers.to_vec();
        if let RenderLayer::Raster(upper) = &mut layers[2] {
            upper.key.revision += 1;
        }
        changed_upper.layers = layers.into();
        assert!(
            !effects
                .update(&graphics, &changed_upper, &raster, &captured, false)
                .unwrap(),
            "marks above a mosaic must not invalidate its sampled pixels"
        );
        assert_eq!(
            effects.versions[&Output::Mosaic(mask.key.asset)],
            original_version
        );

        let distant = raster_layer(
            7,
            Rect {
                left: 24,
                top: 0,
                right: 32,
                bottom: 8,
            },
            [220, 0, 0, 255],
        );
        let mut outside = scene.clone();
        let mut layers = outside.layers.to_vec();
        layers.insert(0, RenderLayer::Raster(distant));
        outside.layers = layers.into();
        assert!(
            !effects
                .update(&graphics, &outside, &raster, &captured, false)
                .unwrap(),
            "a lower mark outside the filter sample footprint must not invalidate it"
        );

        let RenderLayer::Mosaic(base) = &scene.layers[1] else {
            unreachable!()
        };
        let mut far = base.clone();
        let far_mask = raster_layer(
            5,
            Rect {
                left: 16,
                top: 0,
                right: 24,
                bottom: 8,
            },
            [128; 4],
        );
        far.masks = vec![far_mask.clone()].into();
        let mut chained = base.clone();
        let chained_mask = raster_layer(6, bounds, [128; 4]);
        chained.masks = vec![chained_mask.clone()].into();
        for mask in [&far_mask, &chained_mask] {
            raster.insert(
                mask.key,
                Texture::create(&graphics.device, 8, 8, Some(&mask.pixels)).unwrap(),
            );
        }
        let mut graph = scene.clone();
        graph.layers = vec![
            scene.layers[0].clone(),
            scene.layers[1].clone(),
            RenderLayer::Mosaic(far),
            RenderLayer::Mosaic(chained),
        ]
        .into();
        assert!(
            effects
                .update(&graphics, &graph, &raster, &captured, false)
                .unwrap()
        );
        let far_version = effects.versions[&Output::Mosaic(5)];
        let chain_version = effects.versions[&Output::Mosaic(6)];
        let mut layers = graph.layers.to_vec();
        if let RenderLayer::Raster(lower) = &mut layers[0] {
            lower.key.revision += 1;
            lower.pixels = [0, 200, 0, 255].repeat(32).into();
            raster.insert(
                lower.key,
                Texture::create(&graphics.device, 4, 8, Some(&lower.pixels)).unwrap(),
            );
        }
        graph.layers = layers.into();
        assert!(
            effects
                .update(&graphics, &graph, &raster, &captured, false)
                .unwrap()
        );
        assert!(effects.versions[&Output::Mosaic(2)] > original_version);
        assert!(
            effects.versions[&Output::Mosaic(6)] > chain_version,
            "changed lower output must propagate into overlapping later effects"
        );
        assert_eq!(
            effects.versions[&Output::Mosaic(5)],
            far_version,
            "disjoint effect work must retain its output revision"
        );
        let refreshed =
            super::super::graphics::tests::readback(&graphics, &effects.tiles[&2].output);
        assert_eq!(&refreshed[..4], &[0, 100, 0, 128]);
        let near_version = effects.versions[&Output::Mosaic(2)];
        let chain_version = effects.versions[&Output::Mosaic(6)];
        let source_change = [Rect {
            left: 6,
            top: 0,
            right: 8,
            bottom: 8,
        }];
        let mut source_pixels = [0, 0, 200, 255].repeat(64);
        for y in 0..8 {
            for x in 6..8 {
                source_pixels[(y * 8 + x) * 4..(y * 8 + x + 1) * 4]
                    .copy_from_slice(&[200, 0, 0, 255]);
            }
        }
        captured[0]
            .texture
            .upload(&graphics.context, &source_pixels);
        assert!(
            effects
                .update(
                    &graphics,
                    &graph,
                    &raster,
                    &captured,
                    SourceChanges::Regions(&source_change)
                )
                .unwrap()
        );
        assert!(effects.versions[&Output::Mosaic(2)] > near_version);
        assert!(effects.versions[&Output::Mosaic(6)] > chain_version);
        assert_eq!(effects.versions[&Output::Mosaic(5)], far_version);
        let changed_pixels =
            super::super::graphics::tests::readback(&graphics, &effects.tiles[&2].output);
        assert_eq!(&changed_pixels[7 * 4..8 * 4], &[100, 0, 0, 128]);

        let near_version = effects.versions[&Output::Mosaic(2)];
        let mut paused_source = graph.clone();
        paused_source.paused_regions = vec![bounds].into();
        assert!(
            !effects
                .update(
                    &graphics,
                    &paused_source,
                    &raster,
                    &captured,
                    SourceChanges::Regions(&source_change)
                )
                .unwrap()
        );
        assert_eq!(effects.versions[&Output::Mosaic(2)], near_version);
        assert!(
            effects
                .update(&graphics, &graph, &raster, &captured, false)
                .unwrap(),
            "paused source changes must be remembered until resume"
        );
        assert!(effects.versions[&Output::Mosaic(2)] > near_version);
        assert_eq!(effects.versions[&Output::Mosaic(5)], far_version);
        use windows::core::Interface;
        let resource = effects.tiles[&mask.key.asset].source._resource.as_raw();
        let mut moved = scene.clone();
        moved.stamp.revision = 2;
        let mut layers = moved.layers.to_vec();
        if let RenderLayer::Mosaic(layer) = &mut layers[1] {
            Arc::make_mut(&mut layer.object).bounds = Bounds::from_corners((2., 0.), (10., 8.));
            let mut masks = layer.masks.to_vec();
            masks[0].key.revision = 2;
            masks[0].bounds = Rect {
                left: 2,
                right: 10,
                ..bounds
            };
            raster.insert(
                masks[0].key,
                Texture::create(&graphics.device, 8, 8, Some(&masks[0].pixels)).unwrap(),
            );
            layer.masks = masks.into();
        }
        moved.layers = layers.into();
        effects
            .update(&graphics, &moved, &raster, &captured, true)
            .unwrap();
        assert_eq!(
            effects.tiles[&mask.key.asset].source._resource.as_raw(),
            resource,
            "moving a same-sized mask must reuse its device workspace across revisions"
        );
        let pixels = super::super::graphics::tests::readback(
            &graphics,
            &effects.tiles[&mask.key.asset].output,
        );
        assert_eq!(
            &pixels[..4],
            &[100, 0, 0, 128],
            "reused workspace must sample the moved desktop coordinates"
        );
    }
}
