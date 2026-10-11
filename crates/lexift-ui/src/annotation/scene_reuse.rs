//! Reuse an assembled scene before pruning, raster checks or layer allocation.
//! Object snapshots share existing asset caches; missing snapshots conservatively
//! disable reuse rather than retaining a second document or hashing correctness.
use super::{Document, EditorOverlay, SceneInput};
use lexift_core::{
    domain::{
        annotation::{Object, Watermark},
        geometry::Rect,
    },
    ports::annotation_render::AnnotationScene,
};
use std::sync::Arc;

pub(super) struct Compiled {
    pub scene: AnnotationScene,
    objects: Vec<Arc<Object>>,
    draft: bool,
    selected: Option<usize>,
    hovered: Option<usize>,
    reference: Option<String>,
    editor: Option<EditorOverlay>,
    watermark: Option<Watermark>,
    watermark_erased: Vec<lexift_core::domain::annotation_erase::ErasedCoverage>,
    spotlight_erased: Vec<lexift_core::domain::annotation_erase::ErasedCoverage>,
    displays: Vec<Rect>,
    spotlight_opacity: f32,
}

impl Compiled {
    pub fn new(
        input: &SceneInput<'_>,
        scene: &AnnotationScene,
        mut snapshot: impl FnMut(usize, &Object) -> Option<Arc<Object>>,
    ) -> Option<Self> {
        let document = Document {
            session: input.session,
            edit: input.edit,
        };
        let objects = document
            .objects()
            .chain(input.draft)
            .enumerate()
            .map(|(index, object)| snapshot(index, object))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            scene: scene.clone(),
            objects,
            draft: input.draft.is_some(),
            selected: document.selected(),
            hovered: input.hovered,
            reference: input.reference.map(str::to_owned),
            editor: input.editor.cloned(),
            watermark: input.session.watermark.clone(),
            watermark_erased: input.session.watermark_erased.clone(),
            spotlight_erased: input.session.spotlight_erased.clone(),
            displays: input.session.displays.clone(),
            spotlight_opacity: input.session.spotlight_opacity,
        })
    }

    /// Compare full object values and input policy, never request revision or
    /// ephemeral UI defaults. Invalid/nonfinite inputs cannot equal a valid scene.
    pub fn matches(&self, input: &SceneInput<'_>) -> bool {
        // Exhaustive input destructuring makes a future render boundary require
        // an explicit reuse decision rather than silently keeping stale scenes.
        let SceneInput {
            edit,
            stamp,
            session,
            surfaces,
            draft,
            hovered,
            reference,
            editor,
            paused_regions,
            excluded_windows,
        } = input;
        let document = Document {
            session,
            edit: *edit,
        };
        self.scene.stamp.session == stamp.session
            && self.scene.surfaces.len() == surfaces.len()
            && self.scene.surfaces.iter().zip(*surfaces).all(|(a, b)| {
                a.input_window == b.input_window && a.bounds == b.bounds && a.scale == b.scale
            })
            && self.scene.paused_regions.as_ref() == *paused_regions
            && self.scene.excluded_windows.as_ref() == *excluded_windows
            && self.draft == draft.is_some()
            && self.selected == document.selected()
            && self.hovered == *hovered
            && self.reference.as_deref() == *reference
            && self.editor.as_ref() == *editor
            && self.watermark_erased == session.watermark_erased
            && self.spotlight_erased == session.spotlight_erased
            && self.watermark == session.watermark
            && self.displays == session.displays
            && self.spotlight_opacity == session.spotlight_opacity
            && self.objects.len() == document.len() + usize::from(draft.is_some())
            && self
                .objects
                .iter()
                .zip(document.objects().chain(*draft))
                .all(|(saved, object)| saved.as_ref() == object)
    }
}
