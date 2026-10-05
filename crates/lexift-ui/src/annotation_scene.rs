//! Local immutable document rasters shared by software and native scene assembly.
use crate::annotation_render;
use lexift_core::{
    domain::{
        annotation::{
            Bounds, Extra, Kind, Object, ObjectId, Session, Watermark, rotate_text_point,
        },
        geometry::Rect,
    },
    ports::{
        annotation_render::{
            AnnotationScene, MagnifierLayer, MosaicLayer, RasterKey, RasterLayer, RenderLayer,
            RenderStamp, RenderSurface,
        },
        magnifier::WindowToken,
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tiny_skia::{Pixmap, PixmapPaint, Transform};
#[path = "annotation_scene_content.rs"]
mod content;
pub(crate) use content::same as same_content;
#[path = "annotation_scene_interaction.rs"]
mod interaction;
#[path = "annotation_scene_mosaic.rs"]
mod mosaic_geometry;
#[cfg(test)]
#[path = "annotation_scene_preparation_tests.rs"]
mod preparation_tests;
#[path = "annotation_scene_reuse.rs"]
mod reuse;
#[cfg(test)]
#[path = "annotation_scene_reuse_tests.rs"]
mod reuse_tests;
pub(crate) use interaction::EditorOverlay;

pub(crate) struct RenderInput<'a> {
    pub edit: Option<(Option<usize>, &'a Object)>,
    pub session: &'a Session,
    pub viewport: Rect,
    pub scale: f32,
    pub draft: Option<&'a Object>,
    pub hovered: Option<usize>,
    pub reference: Option<&'a str>,
    pub editor: Option<&'a EditorOverlay>,
}

/// Borrow the committed document and substitute only the active edit. No undo
/// history or unrelated trajectory is copied into the preview.
#[derive(Clone, Copy)]
pub(crate) struct Document<'a> {
    pub session: &'a Session,
    pub edit: Option<(Option<usize>, &'a Object)>,
}
impl<'a> From<&'a Session> for Document<'a> {
    fn from(session: &'a Session) -> Self {
        Self {
            session,
            edit: None,
        }
    }
}
impl<'a> Document<'a> {
    pub fn len(self) -> usize {
        self.session.objects.len()
            + usize::from(self.edit.is_some_and(|(index, _)| index.is_none()))
    }
    pub fn object_index(self, id: ObjectId) -> Option<usize> {
        self.objects().position(|object| object.id == id)
    }
    pub fn selected(self) -> Option<usize> {
        self.edit.map_or(self.session.selected, |(index, _)| {
            Some(index.unwrap_or(self.session.objects.len()))
        })
    }
    pub fn get(self, index: usize) -> Option<&'a Object> {
        if let Some((edited, object)) = self.edit
            && index == edited.unwrap_or(self.session.objects.len())
        {
            return Some(object);
        }
        self.session.objects.get(index)
    }
    pub fn objects(self) -> impl ExactSizeIterator<Item = &'a Object> {
        let len = self.len();
        (0..len).map(move |index| self.get(index).expect("document view index"))
    }
}

pub(crate) struct SceneInput<'a> {
    pub edit: Option<(Option<usize>, &'a Object)>,
    pub stamp: RenderStamp,
    pub session: &'a Session,
    pub surfaces: &'a [RenderSurface],
    pub draft: Option<&'a Object>,
    pub hovered: Option<usize>,
    pub reference: Option<&'a str>,
    pub editor: Option<&'a EditorOverlay>,
    pub paused_regions: &'a [Rect],
    pub excluded_windows: &'a [WindowToken],
}

type Slot = (ObjectId, usize, [i32; 4]);
type EffectSlot = (Slot, bool, usize);
type Identity = (ObjectId, usize);

struct ObjectRaster {
    object: Arc<Object>,
    scale: f32,
    reference: Option<String>,
    raster: RasterLayer,
}

/// One assembly owns this preparation; view-dependent padding and clipping are
/// applied later. Cache hits never allocate or rebuild the trajectory bounds.
struct PreparedObject {
    object: Option<Arc<Object>>,
    bounds: Bounds,
    line_points: Option<Vec<(f32, f32)>>,
}
impl PreparedObject {
    fn new(object: &Object) -> Self {
        Self {
            object: None,
            bounds: geometric_bounds(object),
            line_points: None,
        }
    }
    fn snapshot(&mut self, object: &Object) -> Arc<Object> {
        self.object
            .get_or_insert_with(|| Arc::new(object.clone()))
            .clone()
    }
    fn line_points(&mut self, object: &Object) -> Option<&[(f32, f32)]> {
        matches!(object.kind, Kind::Arrow | Kind::Polyline).then(|| {
            self.line_points
                .get_or_insert_with(|| object.stroke_points())
                .as_slice()
        })
    }
}

struct EffectRaster {
    object: Arc<Object>,
    scale: f32,
    raster: RasterLayer,
}

#[derive(PartialEq)]
struct Backdrop {
    watermark: Option<Watermark>,
    displays: Vec<Rect>,
    spotlights: Vec<Object>,
    opacity: f32,
    scale: f32,
}

struct BackdropRaster {
    state: Backdrop,
    raster: RasterLayer,
}

#[derive(Default)]
pub(crate) struct SceneCompiler {
    compiled: Option<reuse::Compiled>,
    objects: HashMap<Slot, ObjectRaster>,
    effects: HashMap<EffectSlot, EffectRaster>,
    mosaic_geometry: HashMap<Identity, mosaic_geometry::Geometry>,
    metadata: HashMap<Identity, Arc<Object>>,
    interactions: HashMap<[i32; 4], interaction::CacheEntry>,
    backdrops: HashMap<[i32; 4], BackdropRaster>,
    next_asset: u64,
}

fn rectangle_key(rect: Rect) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn pixmap(bounds: Rect) -> Option<Pixmap> {
    let width = u32::try_from(bounds.right.checked_sub(bounds.left)?).ok()?;
    let height = u32::try_from(bounds.bottom.checked_sub(bounds.top)?).ok()?;
    if u64::from(width) * u64::from(height) > 40_000_000 {
        return None;
    }
    Pixmap::new(width, height)
}

fn include(bounds: &mut Bounds, point: (f32, f32)) {
    bounds.left = bounds.left.min(point.0);
    bounds.right = bounds.right.max(point.0);
    bounds.top = bounds.top.min(point.1);
    bounds.bottom = bounds.bottom.max(point.1);
}

fn include_text(bounds: &mut Bounds, object: &Object) {
    let Extra::Text(text) = &object.extra else {
        return;
    };
    let b = object.bounds;
    for point in [
        (b.left, b.top),
        (b.right, b.top),
        (b.right, b.bottom),
        (b.left, b.bottom),
    ] {
        include(bounds, rotate_text_point(point, b, text.rotation));
    }
}

/// Conservative geometric extent includes stroke joins, endpoints, rotated
/// text and linked arrows. No bitmap scan is used to find allocation bounds.
fn geometric_bounds(object: &Object) -> Bounds {
    let mut bounds = object.bounds;
    include_text(&mut bounds, object);
    for point in object.stroke_points() {
        include(&mut bounds, point);
    }
    if let Some(caption) = object.sequence_caption() {
        include(&mut bounds, (caption.bounds.left, caption.bounds.top));
        include(&mut bounds, (caption.bounds.right, caption.bounds.bottom));
        include_text(&mut bounds, &caption);
    }
    if let Some(arrow) = object.text_link_arrow() {
        for point in arrow.stroke_points() {
            include(&mut bounds, point);
        }
    }
    bounds
}

fn clipped_bounds(bounds: Bounds, width: f32, viewport: Rect, scale: f32) -> Option<Rect> {
    let padding = width.max(1.) * scale * 5. + 40.;
    let visible = Rect {
        left: (bounds.left - padding).floor() as i32,
        top: (bounds.top - padding).floor() as i32,
        right: (bounds.right + padding).ceil() as i32,
        bottom: (bounds.bottom + padding).ceil() as i32,
    };
    annotation_render::mosaic::intersect(visible, viewport)
}

fn content_bounds(object: &Object, viewport: Rect, scale: f32) -> Option<Rect> {
    clipped_bounds(
        geometric_bounds(object),
        object.style.width,
        viewport,
        scale,
    )
}

impl SceneCompiler {
    /// Assemble one immutable document for every display. Interaction pixels
    /// stay separate so native effects never sample selection or IME feedback.
    pub fn compile(&mut self, input: SceneInput<'_>) -> Option<AnnotationScene> {
        if let Some(compiled) = &self.compiled
            && compiled.matches(&input)
        {
            let mut scene = compiled.scene.clone();
            scene.stamp = input.stamp;
            return Some(scene);
        }
        self.compiled = None;
        let scene = self.assemble(&input)?;
        self.compiled = reuse::Compiled::new(&input, &scene, |index, object| {
            let identity = (
                object.id,
                if object.id == ObjectId::DRAFT {
                    index
                } else {
                    0
                },
            );
            // Effect metadata contains the actual object; the white-mask cache
            // canonicalizes parameters and cannot stand in for this snapshot.
            self.metadata
                .get(&identity)
                .filter(|saved| saved.as_ref() == object)
                .cloned()
                .or_else(|| {
                    input.surfaces.iter().find_map(|surface| {
                        self.objects
                            .get(&(identity.0, identity.1, rectangle_key(surface.bounds)))
                            .filter(|saved| saved.object.as_ref() == object)
                            .map(|saved| saved.object.clone())
                    })
                })
        });
        Some(scene)
    }

    fn assemble(&mut self, input: &SceneInput<'_>) -> Option<AnnotationScene> {
        // Reject invalid native surfaces before pruning or rasterizing caches.
        let mut scene = AnnotationScene {
            stamp: input.stamp,
            surfaces: Arc::from(input.surfaces),
            layers: Arc::from([]),
            interaction: Arc::from([]),
            paused_regions: Arc::from(input.paused_regions),
            excluded_windows: Arc::from(input.excluded_windows),
        };
        scene.validate().ok()?;
        let views: Vec<_> = input
            .surfaces
            .iter()
            .map(|surface| (surface.bounds, surface.scale))
            .collect();
        self.retain_viewports(&views.iter().map(|view| view.0).collect::<Vec<_>>());
        let document = Document {
            session: input.session,
            edit: input.edit,
        };
        let layers = self.layers(document, &views, input.draft, input.reference)?;
        let mut interaction = Vec::new();
        for &(viewport, scale) in &views {
            interaction.extend(self.interaction_document(
                &RenderInput {
                    edit: input.edit,
                    session: input.session,
                    viewport,
                    scale,
                    draft: input.draft,
                    hovered: input.hovered,
                    reference: input.reference,
                    editor: input.editor,
                },
                document,
            )?);
        }
        scene.layers = Arc::from(layers);
        scene.interaction = Arc::from(interaction);
        scene.validate().ok()?;
        Some(scene)
    }

    fn raster(&mut self, frame: Pixmap, bounds: Rect, old: Option<RasterKey>) -> RasterLayer {
        let key = old.map_or_else(
            || {
                self.next_asset += 1;
                RasterKey {
                    asset: self.next_asset,
                    revision: 1,
                }
            },
            |old| RasterKey {
                asset: old.asset,
                revision: old.revision + 1,
            },
        );
        RasterLayer {
            key,
            bounds,
            pixels: Arc::from(frame.data()),
        }
    }

    /// Test diagnostics count retained trajectory buffers, not allocator or GPU memory.
    #[cfg(test)]
    pub(crate) fn retained_trajectory_payload(&self) -> (usize, usize) {
        let mut buffers = HashMap::new();
        for object in self
            .objects
            .values()
            .map(|entry| entry.object.as_ref())
            .chain(self.effects.values().map(|entry| entry.object.as_ref()))
            .chain(
                self.mosaic_geometry
                    .values()
                    .map(|entry| entry.object.as_ref()),
            )
            .chain(self.metadata.values().map(Arc::as_ref))
        {
            if !object.points.is_empty() {
                buffers.insert(
                    object.points.as_ptr() as usize,
                    object.points.capacity() * std::mem::size_of::<(f32, f32)>(),
                );
            }
        }
        (buffers.len(), buffers.values().sum())
    }

    pub fn clear(&mut self) {
        self.compiled = None;
        self.objects.clear();
        self.effects.clear();
        self.mosaic_geometry.clear();
        self.metadata.clear();
        self.interactions.clear();
        self.backdrops.clear();
    }

    pub fn retain_viewports(&mut self, viewports: &[Rect]) {
        // Software fallback or viewport retirement must release the assembled
        // scene too. A successful compile installs a new snapshot afterwards.
        self.compiled = None;
        let active: HashSet<_> = viewports.iter().copied().map(rectangle_key).collect();
        self.objects.retain(|slot, _| active.contains(&slot.2));
        self.effects
            .retain(|(slot, _, _), _| active.contains(&slot.2));
        self.backdrops.retain(|view, _| active.contains(view));
        self.interactions.retain(|view, _| active.contains(view));
        self.mosaic_geometry
            .retain(|_, geometry| geometry.retain_viewports(&active));
    }

    /// Compile one viewport without retaining a Session or its undo snapshots.
    /// Effect layers are assembled separately during native backend migration.
    fn ordinary_document(
        &mut self,
        document: Document<'_>,
        viewport: Rect,
        scale: f32,
        draft: Option<&Object>,
        reference: Option<&str>,
        prepared: &mut HashMap<Identity, PreparedObject>,
    ) -> Option<Vec<RasterLayer>> {
        let view = rectangle_key(viewport);
        let mut result = Vec::new();
        let backdrop = Backdrop {
            watermark: document.session.watermark.clone(),
            displays: document.session.displays.clone(),
            spotlights: document
                .objects()
                .chain(draft)
                .filter(|o| o.kind.is_spotlight())
                .cloned()
                .collect(),
            opacity: document.session.spotlight_opacity,
            scale,
        };
        if backdrop.watermark.is_some() || !backdrop.spotlights.is_empty() {
            let entry = self.backdrops.get(&view);
            if entry.is_none_or(|entry| entry.state != backdrop) {
                let old = entry.map(|entry| entry.raster.key);
                let mut frame = pixmap(viewport)?;
                annotation_render::draw_backdrop(
                    &mut frame,
                    document.session,
                    viewport,
                    scale,
                    draft,
                )?;
                let raster = self.raster(frame, viewport, old);
                self.backdrops.insert(
                    view,
                    BackdropRaster {
                        state: backdrop,
                        raster,
                    },
                );
            }
            result.push(self.backdrops[&view].raster.clone());
        } else {
            self.backdrops.remove(&view);
        }

        let mut used = HashSet::new();
        for (index, object) in document.objects().chain(draft).enumerate() {
            if matches!(
                object.kind,
                Kind::MosaicArea | Kind::MosaicBrush | Kind::Magnifier
            ) {
                continue;
            }
            // Unassigned drafts and test fixtures still need distinct cache slots.
            let slot = (
                object.id,
                if object.id == ObjectId::DRAFT {
                    index
                } else {
                    0
                },
                view,
            );
            let reference = reference
                .filter(|_| document.selected() == Some(index) && object.kind == Kind::Text);
            let entry = self.objects.get(&slot);
            // Geometry bounds are deterministic for this object, DPI and viewport.
            // A cache hit must not clone/interpolate a long centerline merely to
            // rediscover the bounds already stored with its raster.
            if let Some(entry) = entry.filter(|entry| {
                entry.object.as_ref() == object
                    && entry.scale == scale
                    && entry.reference.as_deref() == reference
            }) {
                used.insert(slot);
                result.push(entry.raster.clone());
                continue;
            }
            let shared = prepared
                .entry((slot.0, slot.1))
                .or_insert_with(|| PreparedObject::new(object));
            let Some(bounds) = clipped_bounds(shared.bounds, object.style.width, viewport, scale)
            else {
                continue;
            };
            used.insert(slot);
            let old = entry.map(|entry| entry.raster.key);
            let mut frame = pixmap(bounds)?;
            if let Some(points) = shared.line_points(object) {
                annotation_render::draw_line_annotation_prepared(
                    &mut frame, object, bounds, scale, points,
                );
            } else {
                annotation_render::draw_content_object(
                    &mut frame, object, bounds, scale, reference,
                );
            }
            let raster = self.raster(frame, bounds, old);
            self.objects.insert(
                slot,
                ObjectRaster {
                    object: shared.snapshot(object),
                    scale,
                    reference: reference.map(str::to_owned),
                    raster,
                },
            );
            result.push(self.objects[&slot].raster.clone());
        }
        self.objects
            .retain(|slot, _| slot.2 != view || used.contains(slot));
        Some(result)
    }

    /// Retain document order across viewports. Effects carry geometry and
    /// parameters only; desktop pixels are supplied by the render adapter.
    pub fn layers<'a>(
        &mut self,
        document: impl Into<Document<'a>>,
        views: &[(Rect, f32)],
        draft: Option<&Object>,
        reference: Option<&str>,
    ) -> Option<Vec<RenderLayer>> {
        let document = document.into();
        let mut prepared = HashMap::new();
        for &(view, scale) in views {
            self.ordinary_document(document, view, scale, draft, reference, &mut prepared)?;
        }
        let mut result = Vec::new();
        for &(view, _) in views {
            if let Some(backdrop) = self.backdrops.get(&rectangle_key(view)) {
                result.push(RenderLayer::Raster(backdrop.raster.clone()));
            }
        }
        let mut used = HashSet::new();
        let mut metadata_used = HashSet::new();
        let mut geometry_used = HashSet::new();
        for (index, object) in document.objects().chain(draft).enumerate() {
            let identity = (
                object.id,
                if object.id == ObjectId::DRAFT {
                    index
                } else {
                    0
                },
            );
            let mosaic = if matches!(&object.extra, Extra::Mosaic(settings) if settings.strength != 0)
            {
                let geometry = self
                    .mosaic_geometry
                    .entry(identity)
                    .or_insert_with(|| mosaic_geometry::Geometry::new(object));
                if !geometry.matches(object) {
                    *geometry = mosaic_geometry::Geometry::new(object);
                }
                Some(geometry.object.clone())
            } else {
                None
            };
            let mut masks = Vec::new();
            let mut decorations = Vec::new();
            for &(view, scale) in views {
                let slot = (
                    object.id,
                    if object.id == ObjectId::DRAFT {
                        index
                    } else {
                        0
                    },
                    rectangle_key(view),
                );
                match &object.extra {
                    Extra::Mosaic(settings) if settings.strength != 0 => {
                        let geometry = mosaic.as_ref().unwrap();
                        let regions = self
                            .mosaic_geometry
                            .get_mut(&identity)
                            .unwrap()
                            .regions(view)
                            .to_vec();
                        for (part, bounds) in regions.into_iter().enumerate() {
                            let key = (slot, false, part);
                            used.insert(key);
                            masks.push(self.effect_raster(
                                key,
                                geometry.clone(),
                                bounds,
                                1.,
                                false,
                            )?);
                        }
                    }
                    Extra::Magnifier {
                        output, ellipse, ..
                    } => {
                        let mut geometry = object.clone();
                        geometry.bounds = *output;
                        geometry.points.clear();
                        geometry.style = lexift_core::domain::annotation::Style::default();
                        geometry.extra = Extra::Magnifier {
                            output: *output,
                            ellipse: *ellipse,
                            zoom: 1.,
                            connector: lexift_core::domain::annotation::MagnifierConnector::None,
                            erase_annotations: false,
                            antialias: false,
                            shadow: false,
                        };
                        if let Some(bounds) = annotation_render::mosaic::intersect(
                            Rect {
                                left: output.left.floor() as i32,
                                top: output.top.floor() as i32,
                                right: output.right.ceil() as i32,
                                bottom: output.bottom.ceil() as i32,
                            },
                            view,
                        ) {
                            let key = (slot, false, 0);
                            used.insert(key);
                            masks.push(self.effect_raster(
                                key,
                                Arc::new(geometry),
                                bounds,
                                1.,
                                false,
                            )?);
                        }
                        let mut geometry = object.clone();
                        include(&mut geometry.bounds, (output.left, output.top));
                        include(&mut geometry.bounds, (output.right, output.bottom));
                        if let Some(bounds) = content_bounds(&geometry, view, scale) {
                            let key = (slot, true, 0);
                            used.insert(key);
                            let mut decoration = object.clone();
                            if let Extra::Magnifier {
                                zoom,
                                erase_annotations,
                                antialias,
                                ..
                            } = &mut decoration.extra
                            {
                                *zoom = 1.;
                                *erase_annotations = false;
                                *antialias = false;
                            }
                            decorations.push(self.effect_raster(
                                key,
                                Arc::new(decoration),
                                bounds,
                                scale,
                                true,
                            )?);
                        }
                    }
                    _ => {
                        if let Some(raster) = self.objects.get(&slot) {
                            result.push(RenderLayer::Raster(raster.raster.clone()));
                        }
                    }
                }
            }
            if mosaic.is_some() && !masks.is_empty() {
                geometry_used.insert(identity);
            }
            if !masks.is_empty()
                || !decorations.is_empty()
                || matches!(&object.extra, Extra::Mosaic(settings) if settings.strength == 0)
            {
                metadata_used.insert(identity);
                let entry = self
                    .metadata
                    .entry(identity)
                    .or_insert_with(|| Arc::new(object.clone()));
                if entry.as_ref() != object {
                    *entry = Arc::new(object.clone());
                }
            }
            match &object.extra {
                Extra::Mosaic(settings) if !masks.is_empty() || settings.strength == 0 => result
                    .push(RenderLayer::Mosaic(MosaicLayer {
                        object: self.metadata[&identity].clone(),
                        masks: masks.into(),
                    })),
                Extra::Magnifier { .. } if !masks.is_empty() || !decorations.is_empty() => result
                    .push(RenderLayer::Magnifier(MagnifierLayer {
                        object: self.metadata[&identity].clone(),
                        masks: masks.into(),
                        decorations: decorations.into(),
                    })),
                _ => {}
            }
        }
        self.effects.retain(|slot, _| used.contains(slot));
        self.mosaic_geometry
            .retain(|identity, _| geometry_used.contains(identity));
        self.metadata
            .retain(|identity, _| metadata_used.contains(identity));
        Some(result)
    }

    fn effect_raster(
        &mut self,
        slot: EffectSlot,
        object: Arc<Object>,
        bounds: Rect,
        scale: f32,
        decoration: bool,
    ) -> Option<RasterLayer> {
        let entry = self.effects.get(&slot);
        if entry.is_none_or(|entry| {
            !(Arc::ptr_eq(&entry.object, &object) || entry.object.as_ref() == object.as_ref())
                || entry.scale != scale
                || entry.raster.bounds != bounds
        }) {
            let old = entry.map(|entry| entry.raster.key);
            let mut frame = pixmap(bounds)?;
            if decoration {
                annotation_render::draw_magnifier_overlay(&mut frame, &object, bounds, scale);
            } else if matches!(object.extra, Extra::Magnifier { .. }) {
                annotation_render::draw_magnifier_mask(&mut frame, &object, bounds);
            } else {
                let mut white = tiny_skia::Paint {
                    anti_alias: true,
                    ..Default::default()
                };
                white.set_color_rgba8(255, 255, 255, 255);
                annotation_render::mosaic::paint_mask(&mut frame, &object, bounds, &white);
            }
            let raster = self.raster(frame, bounds, old);
            self.effects.insert(
                slot,
                EffectRaster {
                    object,
                    scale,
                    raster,
                },
            );
        }
        Some(self.effects[&slot].raster.clone())
    }

    pub fn document<'a>(
        &mut self,
        document: impl Into<Document<'a>>,
        viewport: Rect,
        scale: f32,
        draft: Option<&Object>,
        reference: Option<&str>,
    ) -> Option<Vec<RasterLayer>> {
        // Software callers without live effects consume the same compiled order.
        Some(
            self.layers(document, &[(viewport, scale)], draft, reference)?
                .into_iter()
                .filter_map(|layer| {
                    if let RenderLayer::Raster(raster) = layer {
                        Some(raster)
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    #[cfg(test)]
    pub fn render(
        &mut self,
        session: &Session,
        viewport: Rect,
        scale: f32,
        draft: Option<&Object>,
        hovered: Option<usize>,
        reference: Option<&str>,
    ) -> Option<Pixmap> {
        self.render_scene(RenderInput {
            edit: None,
            session,
            viewport,
            scale,
            draft,
            hovered,
            reference,
            editor: None,
        })
    }

    pub fn render_scene(&mut self, input: RenderInput<'_>) -> Option<Pixmap> {
        let RenderInput {
            session,
            viewport,
            scale,
            draft,
            reference,
            ..
        } = input;
        let document = Document {
            session,
            edit: input.edit,
        };
        let mut layers = self.document(document, viewport, scale, draft, reference)?;
        layers.extend(self.interaction(&input)?);
        compose(viewport, layers.iter())
    }
}

/// Software presentation consumes the same ordinary scene and final overlays.
/// Effects remain the native/legacy adapter's responsibility.
pub(crate) fn compose_document(scene: &AnnotationScene, viewport: Rect) -> Option<Pixmap> {
    if scene
        .layers
        .iter()
        .any(|layer| !matches!(layer, RenderLayer::Raster(_)))
    {
        return None;
    }
    compose(
        viewport,
        scene
            .layers
            .iter()
            .filter_map(|layer| match layer {
                RenderLayer::Raster(raster) => Some(raster),
                _ => None,
            })
            .chain(scene.interaction.iter()),
    )
}

fn compose<'a>(viewport: Rect, layers: impl Iterator<Item = &'a RasterLayer>) -> Option<Pixmap> {
    let mut frame = pixmap(viewport)?;
    for layer in layers {
        if crate::annotation_path::intersect(layer.bounds, viewport).is_none() {
            continue;
        }
        let raster = tiny_skia::PixmapRef::from_bytes(
            &layer.pixels,
            (layer.bounds.right - layer.bounds.left) as u32,
            (layer.bounds.bottom - layer.bounds.top) as u32,
        )?;
        frame.draw_pixmap(
            layer.bounds.left - viewport.left,
            layer.bounds.top - viewport.top,
            raster,
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{
        Endpoint, Style, TextAnnotation, TextBackground, TextOutline, WatermarkPosition,
    };

    #[test]
    fn borrowed_edit_substitution_matches_owned_preview_without_copying_other_trajectories() {
        for scale in [1., 1.25] {
            for append in [false, true] {
                for caption in [false, true] {
                    let mut session = Session::new();
                    let mut brush = object(Kind::Pencil);
                    brush.points = vec![(-1000., -1000.); 20_000];
                    brush.refresh_bounds();
                    assert!(session.add(brush));
                    let mut badge = crate::annotation_sequence::tests::badge(scale);
                    if let Extra::Sequence(sequence) = &mut badge.extra {
                        sequence.caption.content = "原文".into();
                        sequence.caption_bounds = crate::annotation_text::text_bounds(
                            (40., 20.),
                            &sequence.caption,
                            scale,
                        );
                    }
                    crate::annotation_sequence::layout(&mut badge);
                    let original = if caption {
                        badge
                    } else {
                        badge.sequence_caption().unwrap()
                    };
                    if !append {
                        assert!(session.add(original.clone()));
                    }
                    let mut edited = original;
                    match &mut edited.extra {
                        Extra::Text(text) => text.content = "中文草稿 Jf".into(),
                        Extra::Sequence(sequence) => {
                            sequence.caption.content = "中文草稿 Jf".into()
                        }
                        _ => unreachable!(),
                    }
                    let index = (!append).then_some(1);
                    let document = Document {
                        session: &session,
                        edit: Some((index, &edited)),
                    };
                    assert!(std::ptr::eq(document.get(0).unwrap(), &session.objects[0]));
                    assert_eq!(
                        document.get(0).unwrap().points.as_ptr(),
                        session.objects[0].points.as_ptr()
                    );
                    assert_eq!(document.objects().len(), 2);
                    assert!(std::ptr::eq(document.get(1).unwrap(), &edited));
                    assert_eq!(document.selected(), Some(1));
                    // An owned preview is the visual oracle, not the production path.
                    let mut expected = Session::new();
                    expected.objects = session.objects.clone();
                    if append {
                        expected.objects.push(edited.clone());
                    } else {
                        expected.objects[1] = edited.clone();
                    }
                    expected.selected = Some(1);
                    let viewport = Rect {
                        left: -256,
                        top: -128,
                        right: 256,
                        bottom: 256,
                    };
                    let surfaces = [RenderSurface {
                        input_window: WindowToken(1),
                        bounds: viewport,
                        scale,
                    }];
                    let scene = SceneCompiler::default()
                        .compile(SceneInput {
                            edit: document.edit,
                            stamp: RenderStamp {
                                session: 1,
                                revision: 1,
                            },
                            session: &session,
                            surfaces: &surfaces,
                            draft: None,
                            hovered: None,
                            reference: None,
                            editor: None,
                            paused_regions: &[],
                            excluded_windows: &[],
                        })
                        .unwrap();
                    let actual = compose_document(&scene, viewport).unwrap();
                    let reference = SceneCompiler::default()
                        .render(&expected, viewport, scale, None, None, None)
                        .unwrap();
                    let maximum = actual
                        .data()
                        .iter()
                        .zip(reference.data())
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        maximum <= 1,
                        "scale={scale} append={append} caption={caption} error={maximum}"
                    );
                    let software = annotation_render::render_document_with_effects(
                        document, viewport, scale, None, None, None, None,
                    )
                    .unwrap();
                    let owned_software = annotation_render::render_editing(
                        &expected, viewport, scale, None, None, None,
                    )
                    .unwrap();
                    assert_eq!(
                        software.data(),
                        owned_software.data(),
                        "legacy software view scale={scale} append={append} caption={caption}"
                    );
                    let compiled_software = SceneCompiler::default()
                        .render_scene(RenderInput {
                            session: &session,
                            edit: document.edit,
                            viewport,
                            scale,
                            draft: None,
                            hovered: None,
                            reference: None,
                            editor: None,
                        })
                        .unwrap();
                    assert_eq!(
                        compiled_software.data(),
                        reference.data(),
                        "cached software view scale={scale} append={append} caption={caption}"
                    );
                    let mut snapshots = annotation_render::mosaic_worker::Snapshot::default();
                    let snapshot = snapshots.get(document, None);
                    assert_eq!(snapshot.objects, expected.objects);
                    assert!(Arc::ptr_eq(&snapshot, &snapshots.get(document, None)));
                    assert_eq!(session.objects.len(), if append { 1 } else { 2 });
                    assert!(session.can_undo());
                }
            }
        }
    }

    #[test]
    fn complete_scene_preserves_display_caches_and_separates_interaction() {
        let surfaces = [
            RenderSurface {
                input_window: WindowToken(1),
                bounds: Rect {
                    left: -256,
                    top: -128,
                    right: 0,
                    bottom: 256,
                },
                scale: 1.,
            },
            RenderSurface {
                input_window: WindowToken(2),
                bounds: Rect {
                    left: 0,
                    top: -128,
                    right: 256,
                    bottom: 256,
                },
                scale: 1.25,
            },
        ];
        let paused = [Rect {
            left: -16,
            top: 16,
            right: 32,
            bottom: 64,
        }];
        let excluded = [WindowToken(3)];
        let mut session = Session::new();
        session.add(object(Kind::Rectangle));
        let mut compiler = SceneCompiler::default();
        let input = |revision| SceneInput {
            edit: None,
            stamp: RenderStamp {
                session: 7,
                revision,
            },
            session: &session,
            surfaces: &surfaces,
            draft: None,
            hovered: None,
            reference: None,
            editor: None,
            paused_regions: &paused,
            excluded_windows: &excluded,
        };
        let first = compiler.compile(input(1)).unwrap();
        assert_eq!(first.surfaces.len(), 2);
        assert_eq!(&*first.paused_regions, &paused);
        assert_eq!(&*first.excluded_windows, &excluded);
        assert!(!first.interaction.is_empty());
        let second = compiler.compile(input(2)).unwrap();
        for (a, b) in first.layers.iter().zip(second.layers.iter()) {
            let (RenderLayer::Raster(a), RenderLayer::Raster(b)) = (a, b) else {
                panic!("ordinary scene")
            };
            assert_eq!(a.key, b.key);
            assert!(Arc::ptr_eq(&a.pixels, &b.pixels));
        }
        for surface in surfaces {
            let actual = compose_document(&second, surface.bounds).unwrap();
            let expected =
                annotation_render::render(&session, surface.bounds, surface.scale, None, None)
                    .unwrap();
            let (index, difference) = actual
                .data()
                .iter()
                .zip(expected.data())
                .enumerate()
                .map(|(index, (a, b))| (index, a.abs_diff(*b)))
                .max_by_key(|(_, difference)| *difference)
                .unwrap();
            assert!(
                difference <= 1,
                "surface {:?}: diff {difference} at byte {index}, actual {} expected {}",
                surface.bounds,
                actual.data()[index],
                expected.data()[index]
            );
        }
        let mut invalid_surfaces = surfaces;
        invalid_surfaces[0].input_window = WindowToken(0);
        let mut invalid = input(3);
        invalid.surfaces = &invalid_surfaces;
        assert!(compiler.compile(invalid).is_none());
        assert_eq!(compiler.interactions.len(), 2);
        let mut one_display = input(4);
        one_display.surfaces = &surfaces[1..];
        compiler.compile(one_display).unwrap();
        assert!(
            compiler
                .objects
                .keys()
                .all(|slot| slot.2 == rectangle_key(surfaces[1].bounds))
        );
        assert_eq!(compiler.interactions.len(), 1);
        let mut empty = input(5);
        empty.surfaces = &[];
        let empty = compiler.compile(empty).unwrap();
        assert!(empty.layers.is_empty() && empty.interaction.is_empty());
        assert!(compiler.objects.is_empty() && compiler.interactions.is_empty());
    }

    pub(super) fn object(kind: Kind) -> Object {
        Object {
            id: ObjectId::DRAFT,
            kind,
            bounds: Bounds::from_corners((-92.5, -80.25), (30.5, 24.75)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::None,
        }
    }

    #[test]
    #[ignore = "Release-only cached scene CPU comparison; no App or display latency measurement"]
    fn cached_scene_viewport_cleanup_costs() {
        use std::{hint::black_box, time::Instant};
        if cfg!(debug_assertions) {
            panic!("run the CPU comparison with --release");
        }
        let surfaces = [
            RenderSurface {
                input_window: WindowToken(1),
                bounds: Rect {
                    left: -128,
                    top: -128,
                    right: 0,
                    bottom: 128,
                },
                scale: 1.,
            },
            RenderSurface {
                input_window: WindowToken(2),
                bounds: Rect {
                    left: 0,
                    top: -128,
                    right: 128,
                    bottom: 128,
                },
                scale: 1.25,
            },
        ];
        for count in [1, 100, 1000] {
            let mut session = Session::new();
            // No undo history: measure the compiler, not fixture construction.
            session.objects = (0..count).map(|_| object(Kind::Rectangle)).collect();
            let mut compiler = SceneCompiler::default();
            let compile = |compiler: &mut SceneCompiler, revision| {
                compiler
                    .assemble(&SceneInput {
                        stamp: RenderStamp {
                            session: 1,
                            revision,
                        },
                        session: &session,
                        edit: None,
                        surfaces: &surfaces,
                        draft: None,
                        hovered: None,
                        reference: None,
                        editor: None,
                        paused_regions: &[],
                        excluded_windows: &[],
                    })
                    .unwrap()
            };
            let reference = compile(&mut compiler, 0);
            let mut samples = [Vec::new(), Vec::new()];
            for revision in 1..=240 {
                // Alternate ordering to avoid consistently favouring the second run.
                for mode in [revision % 2, 1 - revision % 2] {
                    let start = Instant::now();
                    if mode == 0 {
                        compiler.retain_viewports(
                            &surfaces.iter().map(|s| s.bounds).collect::<Vec<_>>(),
                        );
                    }
                    let scene = black_box(compile(&mut compiler, revision as u64));
                    samples[mode].push(start.elapsed().as_secs_f64() * 1000.);
                    assert!(same_content(&reference, &scene));
                }
            }
            for sample in &mut samples {
                sample.sort_by(f64::total_cmp);
            }
            eprintln!(
                "viewport_cleanup_cpu_scope=steady_cached_scene_two_physical_viewports_elapsed_not_input_gpu_or_photon objects={count} samples_per_mode=240 redundant_prune_p95_ms={:.6} compiler_owned_prune_p95_ms={:.6}",
                samples[0][227], samples[1][227]
            );
            compiler.clear();
            assert!(compiler.objects.is_empty());
        }
    }

    #[test]
    fn effect_scene_preserves_order_and_reuses_geometry_when_only_sampling_changes() {
        use lexift_core::domain::annotation::{MagnifierConnector, MosaicAnnotation, MosaicEffect};
        let views = [
            (
                Rect {
                    left: -256,
                    top: -128,
                    right: 0,
                    bottom: 256,
                },
                1.,
            ),
            (
                Rect {
                    left: 0,
                    top: -128,
                    right: 256,
                    bottom: 256,
                },
                1.25,
            ),
        ];
        let mut session = Session::new();
        session.add(object(Kind::Rectangle));
        let mut privacy = object(Kind::MosaicArea);
        privacy.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 50, 12., 1.25));
        session.add(privacy);
        let mut lens = object(Kind::Magnifier);
        lens.extra = Extra::Magnifier {
            output: Bounds::from_corners((-64., 64.), (64., 192.)),
            zoom: 2.,
            ellipse: true,
            connector: MagnifierConnector::Dot,
            erase_annotations: false,
            antialias: true,
            shadow: true,
        };
        session.add(lens);
        session.add(object(Kind::Ellipse));
        let mut compiler = SceneCompiler::default();
        let first = compiler.layers(&session, &views, None, None).unwrap();
        let mosaic = first
            .iter()
            .position(|layer| matches!(layer, RenderLayer::Mosaic(_)))
            .unwrap();
        let lens = first
            .iter()
            .position(|layer| matches!(layer, RenderLayer::Magnifier(_)))
            .unwrap();
        assert!(mosaic > 0 && lens == mosaic + 1 && lens < first.len() - 1);
        assert!(
            first[..mosaic]
                .iter()
                .chain(&first[lens + 1..])
                .all(|layer| matches!(layer, RenderLayer::Raster(_)))
        );
        let surfaces: Vec<_> = views
            .iter()
            .enumerate()
            .map(|(index, &(bounds, scale))| RenderSurface {
                input_window: WindowToken(index as isize + 1),
                bounds,
                scale,
            })
            .collect();
        let scene = compiler
            .compile(SceneInput {
                edit: None,
                stamp: RenderStamp {
                    session: 1,
                    revision: 1,
                },
                session: &session,
                surfaces: &surfaces,
                draft: None,
                hovered: None,
                reference: None,
                editor: None,
                paused_regions: &[],
                excluded_windows: &[],
            })
            .unwrap();
        scene.validate().unwrap();
        assert_eq!(scene.layers.len(), first.len());
        assert!(matches!(scene.layers[mosaic], RenderLayer::Mosaic(_)));
        assert!(matches!(scene.layers[lens], RenderLayer::Magnifier(_)));
        assert!(compose_document(&scene, views[0].0).is_none());
        if let RenderLayer::Magnifier(layer) = &first[lens] {
            let whole = Rect {
                left: -256,
                top: -128,
                right: 256,
                bottom: 256,
            };
            let mut actual = pixmap(whole).unwrap();
            for mask in &*layer.masks {
                let pixels = tiny_skia::PixmapRef::from_bytes(
                    &mask.pixels,
                    (mask.bounds.right - mask.bounds.left) as u32,
                    (mask.bounds.bottom - mask.bounds.top) as u32,
                )
                .unwrap();
                actual.draw_pixmap(
                    mask.bounds.left - whole.left,
                    mask.bounds.top - whole.top,
                    pixels,
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
            let mut expected = pixmap(whole).unwrap();
            annotation_render::draw_magnifier_mask(&mut expected, &session.objects[2], whole);
            assert_eq!(
                actual.data(),
                expected.data(),
                "cross-viewport lens coverage must match whole output"
            );
        }
        let repeated = compiler.layers(&session, &views, None, None).unwrap();
        if let (RenderLayer::Mosaic(a), RenderLayer::Mosaic(b)) =
            (&first[mosaic], &repeated[mosaic])
        {
            assert!(Arc::ptr_eq(&a.object, &b.object));
            assert_eq!(a.masks.len(), 2);
        } else {
            unreachable!();
        }
        if let Extra::Mosaic(settings) = &mut session.objects[1].extra {
            settings.strength = 100;
            settings.effect = MosaicEffect::Blur;
        }
        if let Extra::Magnifier {
            zoom,
            antialias,
            erase_annotations,
            ..
        } = &mut session.objects[2].extra
        {
            *zoom = 4.;
            *antialias = false;
            *erase_annotations = true;
        }
        let changed = compiler.layers(&session, &views, None, None).unwrap();
        for index in [mosaic, lens] {
            let a = first[index].rasters().collect::<Vec<_>>();
            let b = changed[index].rasters().collect::<Vec<_>>();
            assert_eq!(a.len(), b.len());
            assert!(
                a.iter()
                    .zip(b)
                    .all(|(a, b)| a.key == b.key && Arc::ptr_eq(&a.pixels, &b.pixels)),
                "sampling parameters do not repaint masks or decorations"
            );
        }
        session.objects.clear();
        compiler.layers(&session, &views, None, None).unwrap();
        assert!(compiler.effects.is_empty() && compiler.metadata.is_empty());
    }

    #[test]
    fn mosaic_geometry_shares_trajectories_and_invalidates_complete_masks() {
        use lexift_core::domain::annotation::{MosaicAnnotation, MosaicEffect};
        let views = [
            (
                Rect {
                    left: -512,
                    top: -128,
                    right: 0,
                    bottom: 128,
                },
                1.,
            ),
            (
                Rect {
                    left: 0,
                    top: -128,
                    right: 512,
                    bottom: 128,
                },
                1.25,
            ),
        ];
        let whole = Rect {
            left: -512,
            top: -128,
            right: 512,
            bottom: 128,
        };
        let mut brush = object(Kind::MosaicBrush);
        brush.points = vec![
            (-450.25, -60.5),
            (-90.75, 50.25),
            (100.5, -60.75),
            (460.25, 70.5),
        ];
        brush.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 100, 32., 1.25));
        brush.refresh_bounds();
        let mut session = Session::new();
        session.add(brush);
        let baseline = session.objects[0].clone();
        let identity = (baseline.id, 0);
        let mut compiler = SceneCompiler::default();
        let check = |compiler: &mut SceneCompiler, session: &Session| {
            let layers = compiler.layers(session, &views, None, None).unwrap();
            let RenderLayer::Mosaic(layer) = &layers[0] else {
                unreachable!()
            };
            let mut actual = pixmap(whole).unwrap();
            for mask in &*layer.masks {
                let source = tiny_skia::PixmapRef::from_bytes(
                    &mask.pixels,
                    (mask.bounds.right - mask.bounds.left) as u32,
                    (mask.bounds.bottom - mask.bounds.top) as u32,
                )
                .unwrap();
                actual.draw_pixmap(
                    mask.bounds.left - whole.left,
                    mask.bounds.top - whole.top,
                    source,
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
            let mut expected = pixmap(whole).unwrap();
            let mut white = tiny_skia::Paint {
                anti_alias: true,
                ..Default::default()
            };
            white.set_color_rgba8(255, 255, 255, 255);
            annotation_render::mosaic::paint_mask(
                &mut expected,
                &session.objects[0],
                whole,
                &white,
            );
            assert_eq!(
                actual.data(),
                expected.data(),
                "all mask pixels must match the uncached whole path"
            );
            layer.masks.iter().map(|mask| mask.key).collect::<Vec<_>>()
        };
        let first_keys = check(&mut compiler, &session);
        let geometry = compiler.mosaic_geometry[&identity].object.clone();
        assert!(
            compiler
                .effects
                .values()
                .all(|mask| Arc::ptr_eq(&mask.object, &geometry))
        );
        assert_eq!(
            compiler.retained_trajectory_payload().0,
            2,
            "one normalized mask trajectory and one sampling metadata trajectory"
        );
        session.objects[0].style.color = [10, 20, 30];
        session.objects[0].style.width = 80.;
        if let Extra::Mosaic(settings) = &mut session.objects[0].extra {
            settings.effect = MosaicEffect::Pixelate;
            settings.strength = 25;
        }
        assert_eq!(check(&mut compiler, &session), first_keys);
        assert!(Arc::ptr_eq(
            &geometry,
            &compiler.mosaic_geometry[&identity].object
        ));
        let parameter_state = session.objects[0].clone();
        for change in 0..5 {
            session.objects[0] = parameter_state.clone();
            match change {
                0 => session.objects[0].points[1].1 += 15.,
                1 => {
                    if let Extra::Mosaic(settings) = &mut session.objects[0].extra {
                        settings.diameter *= 1.5;
                    }
                }
                2 => {
                    if let Extra::Mosaic(settings) = &mut session.objects[0].extra {
                        settings.creation_scale = 1.;
                    }
                }
                3 => session.objects[0].style.rounding += 3.,
                _ => {
                    session.objects[0].kind = Kind::MosaicArea;
                    session.objects[0].points.clear();
                    session.objects[0].bounds = Bounds::from_corners((-60., -50.), (60., 50.));
                }
            }
            if change < 3 {
                session.objects[0].refresh_bounds();
            }
            check(&mut compiler, &session);
            assert!(
                !Arc::ptr_eq(&geometry, &compiler.mosaic_geometry[&identity].object),
                "geometry change {change} must replace the normalized mask"
            );
        }
        // Restoring an earlier snapshot must rebuild coverage even with the same object id.
        session.objects[0] = baseline;
        assert_eq!(check(&mut compiler, &session).len(), first_keys.len());
        compiler.retain_viewports(&[views[0].0]);
        assert!(
            compiler
                .effects
                .keys()
                .all(|slot| slot.0.2 == rectangle_key(views[0].0))
        );
        compiler.layers(&session, &[views[0]], None, None).unwrap();
        let weak = Arc::downgrade(&compiler.mosaic_geometry[&identity].object);
        compiler.retain_viewports(&[]);
        assert!(compiler.mosaic_geometry.is_empty() && compiler.effects.is_empty());
        assert!(
            weak.upgrade().is_none(),
            "removed views release their mask geometry"
        );
        compiler.layers(&session, &views, None, None).unwrap();
        if let Extra::Mosaic(settings) = &mut session.objects[0].extra {
            settings.strength = 0;
        }
        compiler.layers(&session, &views, None, None).unwrap();
        assert!(compiler.mosaic_geometry.is_empty() && compiler.effects.is_empty());
        session.objects.clear();
        compiler.layers(&session, &views, None, None).unwrap();
        assert!(compiler.metadata.is_empty());
        compiler.clear();
        assert_eq!(compiler.retained_trajectory_payload(), (0, 0));
    }

    #[test]
    fn partitioned_brush_coverage_matches_whole_path_across_negative_cells() {
        use lexift_core::domain::annotation::{MosaicAnnotation, MosaicEffect};
        let view = Rect {
            left: -512,
            top: -256,
            right: 512,
            bottom: 256,
        };
        let mut brush = object(Kind::MosaicBrush);
        brush.points = vec![
            (-450.25, -120.5),
            (-90.75, 50.25),
            (100.5, -60.75),
            (460.25, 90.5),
        ];
        brush.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 100, 80., 1.25));
        brush.refresh_bounds();
        let mut session = Session::new();
        session.add(brush);
        let mut compiler = SceneCompiler::default();
        let layers = compiler
            .layers(&session, &[(view, 1.25)], None, None)
            .unwrap();
        let RenderLayer::Mosaic(layer) = &layers[0] else {
            unreachable!();
        };
        assert!(layer.masks.len() > 1);
        let mut actual = pixmap(view).unwrap();
        for mask in &*layer.masks {
            let source = tiny_skia::PixmapRef::from_bytes(
                &mask.pixels,
                (mask.bounds.right - mask.bounds.left) as u32,
                (mask.bounds.bottom - mask.bounds.top) as u32,
            )
            .unwrap();
            actual.draw_pixmap(
                mask.bounds.left - view.left,
                mask.bounds.top - view.top,
                source,
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        let mut expected = pixmap(view).unwrap();
        let mut white = tiny_skia::Paint::default();
        white.set_color_rgba8(255, 255, 255, 255);
        white.anti_alias = true;
        annotation_render::mosaic::paint_mask(&mut expected, &session.objects[0], view, &white);
        let maximum = actual
            .data()
            .iter()
            .zip(expected.data())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        let at = actual
            .data()
            .iter()
            .zip(expected.data())
            .position(|(a, b)| a.abs_diff(*b) == maximum)
            .unwrap()
            / 4;
        assert!(
            maximum <= 1,
            "partitioned coverage differs by {maximum} at ({}, {}), actual {:?} expected {:?}, tiles {:?}",
            at % actual.width() as usize,
            at / actual.width() as usize,
            &actual.data()[at * 4..at * 4 + 4],
            &expected.data()[at * 4..at * 4 + 4],
            layer
                .masks
                .iter()
                .map(|mask| mask.bounds)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn local_layers_match_existing_shapes_brushes_lines_and_text_at_both_dpis() {
        let viewport = Rect {
            left: -128,
            top: -128,
            right: 128,
            bottom: 128,
        };
        for scale in [1., 1.25] {
            for kind in [
                Kind::Rectangle,
                Kind::Ellipse,
                Kind::HighlightRectangle,
                Kind::Pencil,
                Kind::HighlightLine,
                Kind::Arrow,
                Kind::Polyline,
                Kind::SpotlightRectangle,
                Kind::SpotlightEllipse,
                Kind::Text,
                Kind::Sequence,
            ] {
                let mut o = object(kind);
                if matches!(
                    kind,
                    Kind::Pencil | Kind::HighlightLine | Kind::Arrow | Kind::Polyline
                ) {
                    o.points = vec![(-100.25, -80.5), (-30.25, 32.5), (80.75, 44.25)];
                    o.refresh_bounds();
                    if kind == Kind::Arrow {
                        o.extra = Extra::Arrow {
                            curved: true,
                            head: true,
                            start: Endpoint::OpenCircle,
                            end: Endpoint::FilledArrow,
                        };
                    }
                }
                if kind == Kind::Text {
                    let text = TextAnnotation {
                        content: "中文 Jf 12\n第二行".into(),
                        font_family: "Microsoft YaHei".into(),
                        font_size: 24.,
                        bold: true,
                        italic: true,
                        outline: Some(TextOutline {
                            color: [20, 100, 40],
                            width: 3.,
                        }),
                        background: Some(TextBackground {
                            color: [255, 255, 255],
                            opacity: 160,
                            padding: 4.,
                            rounding: 3.,
                        }),
                        rotation: 25.,
                        linked_arrow: Some((100., 100.)),
                        linked_arrow_control: None,
                        layout_scale: scale,
                    };
                    o.bounds = crate::annotation_text::text_bounds((-100., -70.), &text, scale);
                    o.extra = Extra::Text(text);
                }
                if kind == Kind::Sequence {
                    o = crate::annotation_sequence::tests::badge(scale);
                }
                let mut session = Session::new();
                session.add(o);
                let mut compiler = SceneCompiler::default();
                for hovered in [None, Some(0)] {
                    let actual = compiler
                        .render(&session, viewport, scale, None, hovered, None)
                        .unwrap();
                    let expected = annotation_render::render_editing(
                        &session, viewport, scale, None, hovered, None,
                    )
                    .unwrap();
                    let maximum = actual
                        .data()
                        .iter()
                        .zip(expected.data())
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        maximum <= 1,
                        "{kind:?} dpi {scale}: max channel error {maximum}"
                    );
                }
            }
        }
    }

    #[test]
    fn overlays_reuse_rasters_single_object_changes_are_local_and_deletion_releases_cache() {
        let viewport = Rect {
            left: -512,
            top: -512,
            right: 512,
            bottom: 512,
        };
        let mut session = Session::new();
        session.add(object(Kind::Rectangle));
        session.add(object(Kind::Ellipse));
        let mut compiler = SceneCompiler::default();
        let first = compiler
            .document(&session, viewport, 1., None, None)
            .unwrap();
        assert!(first.iter().all(|r| r.pixels.len() < 1024 * 1024 * 4));
        session.selected = Some(0);
        let selected = compiler
            .document(&session, viewport, 1., None, None)
            .unwrap();
        assert!(
            first
                .iter()
                .zip(&selected)
                .all(|(a, b)| a.key == b.key && Arc::ptr_eq(&a.pixels, &b.pixels))
        );
        session.objects[0].style.color = [0, 0, 255];
        let changed = compiler
            .document(&session, viewport, 1., None, None)
            .unwrap();
        assert_eq!(first[0].key.asset, changed[0].key.asset);
        assert!(changed[0].key.revision > first[0].key.revision);
        assert!(Arc::ptr_eq(&first[1].pixels, &changed[1].pixels));
        session.objects.remove(1);
        compiler
            .document(&session, viewport, 1., None, None)
            .unwrap();
        assert_eq!(compiler.objects.len(), 1);
        let second_view = Rect {
            left: -256,
            top: -256,
            right: 256,
            bottom: 256,
        };
        compiler
            .document(&session, second_view, 1., None, None)
            .unwrap();
        assert_eq!(compiler.objects.len(), 2);
        compiler.retain_viewports(&[viewport]);
        assert_eq!(compiler.objects.len(), 1);
        compiler.clear();
        assert!(compiler.objects.is_empty());
    }

    #[test]
    fn watermark_backdrop_is_cached_and_matches_reference_with_spotlight() {
        let viewport = Rect {
            left: -128,
            top: -128,
            right: 128,
            bottom: 128,
        };
        let mut session = Session::new();
        session.displays = vec![viewport];
        session.watermark = Some(Watermark {
            template: "水印".into(),
            content: "水印".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 20.,
            opacity: 100,
            color: [200, 30, 40],
            position: WatermarkPosition::Center,
        });
        let mut compiler = SceneCompiler::default();
        for spotlight in [false, true] {
            if spotlight {
                session.add(object(Kind::SpotlightEllipse));
            }
            let actual = compiler
                .render(&session, viewport, 1.25, None, None, None)
                .unwrap();
            assert!(
                actual.pixels().iter().any(|p| p.red() > 100),
                "a black spotlight mask must not erase the red watermark"
            );
            let expected =
                annotation_render::render_editing(&session, viewport, 1.25, None, None, None)
                    .unwrap();
            assert!(
                actual
                    .data()
                    .iter()
                    .zip(expected.data())
                    .all(|(a, b)| a.abs_diff(*b) <= 1)
            );
            let first = compiler
                .document(&session, viewport, 1.25, None, None)
                .unwrap();
            let repeated = compiler
                .document(&session, viewport, 1.25, None, None)
                .unwrap();
            assert!(Arc::ptr_eq(&first[0].pixels, &repeated[0].pixels));
        }
    }

    #[test]
    fn watermark_spotlight_cache_tracks_creation_drafts_edits_and_undo() {
        let viewport = Rect {
            left: -160,
            top: -120,
            right: 160,
            bottom: 120,
        };
        for scale in [1., 1.25] {
            for watermark_first in [false, true] {
                let mut session = Session::new();
                session.displays = vec![viewport];
                let watermark = Watermark {
                    template: "VISIBLE".into(),
                    content: "VISIBLE".into(),
                    font_family: "Arial".into(),
                    font_size: 24.,
                    opacity: 65,
                    color: [20, 240, 60],
                    position: WatermarkPosition::Center,
                };
                let mut spotlight = object(Kind::SpotlightRectangle);
                spotlight.style.outline = false;
                spotlight.style.rounding = 16.;
                if watermark_first {
                    session.set_watermark(Some(watermark.clone()));
                }
                session.add(spotlight.clone());
                if !watermark_first {
                    session.set_watermark(Some(watermark));
                }
                let mut compiler = SceneCompiler::default();
                let mut check = |session: &Session, draft: Option<&Object>| {
                    let actual = compiler
                        .render(session, viewport, scale, draft, None, None)
                        .unwrap();
                    let expected = annotation_render::render_editing(
                        session, viewport, scale, draft, None, None,
                    )
                    .unwrap();
                    assert!(
                        actual
                            .data()
                            .iter()
                            .zip(expected.data())
                            .all(|(a, b)| a.abs_diff(*b) <= 1)
                    );
                    assert!(
                        actual.pixels().iter().filter(|p| p.green() > 50).count() > 50,
                        "watermark ink must survive every cached scene transition"
                    );
                };
                check(&session, None);
                spotlight.kind = Kind::SpotlightEllipse;
                check(&session, Some(&spotlight));
                check(&session, None); // Cancelling the draft restores the prior backdrop.
                session.begin_drag();
                session
                    .update_selected(|o| o.bounds = Bounds::from_corners((50., 40.), (140., 110.)));
                check(&session, None);
                session.finish_drag();
                assert!(session.undo());
                check(&session, None);
                session.add(spotlight);
                check(&session, None);
                session.delete_selected();
                check(&session, None);
                assert!(session.undo());
                check(&session, None);
                session.begin_drag();
                session.update_selected(|o| {
                    o.bounds = Bounds::from_corners((-150., -110.), (150., 110.))
                });
                check(&session, None);
                session.cancel_drag();
                check(&session, None);
            }
        }
    }

    #[test]
    fn large_rotated_editing_text_keeps_reference_layout_across_viewport_seam() {
        let views = [
            Rect {
                left: -640,
                top: -320,
                right: 0,
                bottom: 320,
            },
            Rect {
                left: 0,
                top: -320,
                right: 640,
                bottom: 320,
            },
        ];
        for scale in [1., 1.25] {
            for size in [96., 400.] {
                let text = TextAnnotation {
                    content: "甲乙 Jf".into(),
                    font_family: "Microsoft YaHei".into(),
                    font_size: size,
                    bold: false,
                    italic: true,
                    outline: Some(TextOutline {
                        color: [20, 180, 90],
                        width: 30.,
                    }),
                    background: Some(TextBackground {
                        color: [240, 220, 200],
                        opacity: 170,
                        padding: 20.,
                        rounding: 21.,
                    }),
                    rotation: 0.4,
                    layout_scale: scale,
                    linked_arrow: None,
                    linked_arrow_control: None,
                };
                let mut draft = object(Kind::Text);
                draft.bounds = crate::annotation_text::text_bounds((-360., -220.), &text, scale);
                draft.extra = Extra::Text(text);
                let mut session = Session::new();
                session.add(draft);
                let mut compiler = SceneCompiler::default();
                for viewport in views {
                    let actual = compiler
                        .render(&session, viewport, scale, None, None, Some("甲乙"))
                        .unwrap();
                    let expected = annotation_render::render_editing(
                        &session,
                        viewport,
                        scale,
                        None,
                        None,
                        Some("甲乙"),
                    )
                    .unwrap();
                    let max = actual
                        .data()
                        .iter()
                        .zip(expected.data())
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    let at = actual
                        .data()
                        .iter()
                        .zip(expected.data())
                        .position(|(a, b)| a.abs_diff(*b) == max)
                        .unwrap();
                    assert!(
                        max <= 1,
                        "size {size} dpi {scale} viewport {viewport:?}: max error {max} at ({}, {}) actual {:?} expected {:?} bounds {:?}",
                        at / 4 % actual.width() as usize,
                        at / 4 / actual.width() as usize,
                        &actual.data()[at / 4 * 4..at / 4 * 4 + 4],
                        &expected.data()[at / 4 * 4..at / 4 * 4 + 4],
                        compiler
                            .document(&session, viewport, scale, None, Some("甲乙"))
                            .unwrap()[0]
                            .bounds
                    );
                }
            }
        }
    }
}
