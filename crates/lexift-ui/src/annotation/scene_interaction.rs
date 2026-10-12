//! Transient editing feedback never contributes to document/effect inputs.
use super::{RenderInput, SceneCompiler, include, include_text, pixmap, rectangle_key};
use crate::{annotation_render, annotation_text};
use lexift_core::{
    domain::{
        annotation::{Bounds, Kind, Object, Session},
        geometry::Rect,
    },
    ports::annotation_render::RasterLayer,
};
use std::ops::Range;

#[derive(Clone, PartialEq)]
pub(crate) struct EditorOverlay {
    pub object: Object,
    pub offset: usize,
    pub anchor: usize,
    pub preedit: Range<usize>,
    pub visible: bool,
    pub reference: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{Extra, ObjectId, Session, Style, TextAnnotation};

    #[test]
    #[ignore = "Release CPU benchmark for alternating caret visibility; not operation-to-display latency"]
    fn caret_blink_cache_cpu_costs() {
        use std::time::Instant;
        let view = Rect {
            left: -640,
            top: -360,
            right: 640,
            bottom: 360,
        };
        let mut report = String::new();
        for (count, size) in [(24, 32.), (240, 32.), (24, 240.)] {
            let text = TextAnnotation {
                content: "中Jfa".repeat(count / 4),
                font_family: "Microsoft YaHei".into(),
                font_size: size,
                bold: false,
                italic: true,
                outline: None,
                background: None,
                rotation: 0.,
                layout_scale: 1.25,
                linked_arrow: None,
                linked_arrow_control: None,
            };
            let object = Object {
                erased: Vec::new(),
                id: ObjectId::DRAFT,
                kind: Kind::Text,
                bounds: annotation_text::text_bounds((-500., -100.), &text, 1.25),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text),
            };
            let mut session = Session::new();
            session.add(object.clone());
            let mut editor = EditorOverlay {
                object,
                offset: 3,
                anchor: 0,
                preedit: 0..0,
                visible: true,
                reference: None,
            };
            let compile = |c: &mut SceneCompiler, e: &EditorOverlay| {
                c.interaction(&RenderInput {
                    edit: None,
                    session: &session,
                    viewport: view,
                    scale: 1.25,
                    draft: None,
                    hovered: None,
                    reference: None,
                    editor: Some(e),
                })
                .unwrap()
            };
            let mut cached = SceneCompiler::default();
            let mut uncached = SceneCompiler::default();
            for visible in [true, false] {
                editor.visible = visible;
                compile(&mut cached, &editor);
                compile(&mut uncached, &editor);
            }
            let mut times = [Vec::new(), Vec::new()];
            for tick in 0..120 {
                editor.visible = tick % 2 == 0;
                // Removing only the alternate reproduces the previous blink
                // path while preserving current geometry and state ownership.
                uncached
                    .interactions
                    .get_mut(&rectangle_key(view))
                    .unwrap()
                    .alternate = None;
                for (index, compiler) in [(0, &mut uncached), (1, &mut cached)] {
                    let start = Instant::now();
                    std::hint::black_box(compile(compiler, &editor));
                    times[index].push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            let p95 = |mut v: Vec<f64>| {
                v.sort_by(f64::total_cmp);
                v[(v.len() * 95 / 100).min(v.len() - 1)]
            };
            let entry = cached.interactions.get(&rectangle_key(view)).unwrap();
            let bytes =
                entry.raster.pixels.len() + entry.alternate.as_ref().map_or(0, |r| r.pixels.len());
            report.push_str(&format!("glyphs={count} logical_size={size} dpi=1.25 samples=120 prior_blink_p95_ms={:.6} cached_blink_p95_ms={:.6} retained_interaction_bytes={bytes}\n", p95(times[0].clone()), p95(times[1].clone())));
            let current = std::sync::Arc::downgrade(&entry.raster.pixels);
            let alternate = entry
                .alternate
                .as_ref()
                .map(|r| std::sync::Arc::downgrade(&r.pixels));
            if count == 24 && size == 32. {
                for visible in [true, false] {
                    editor.visible = visible;
                    let frame = cached
                        .render_scene(RenderInput {
                            edit: None,
                            session: &session,
                            viewport: view,
                            scale: 1.25,
                            draft: None,
                            hovered: None,
                            reference: None,
                            editor: Some(&editor),
                        })
                        .unwrap();
                    std::fs::write(
                        format!("../../target/annotation-caret-cache-{visible}.rgba"),
                        frame.data(),
                    )
                    .unwrap();
                }
            }
            cached.clear();
            assert!(current.upgrade().is_none());
            assert!(alternate.unwrap().upgrade().is_none());
        }
        std::fs::write("../../target/annotation-caret-cache-cpu.txt", &report).unwrap();
        println!("{report}");
    }

    #[test]
    fn repeated_caret_blinks_reuse_pixels_and_invalidate_on_edit_changes() {
        let view = Rect {
            left: -160,
            top: -120,
            right: 160,
            bottom: 120,
        };
        for scale in [1., 1.25] {
            let text = TextAnnotation {
                content: "中文 Jf\nabc".into(),
                font_family: "Microsoft YaHei".into(),
                font_size: 24.,
                bold: false,
                italic: true,
                outline: None,
                background: None,
                rotation: 0.2,
                layout_scale: scale,
                linked_arrow: None,
                linked_arrow_control: None,
            };
            let object = Object {
                erased: Vec::new(),
                id: ObjectId::DRAFT,
                kind: Kind::Text,
                bounds: annotation_text::text_bounds((-100., -60.), &text, scale),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text),
            };
            let mut session = Session::new();
            session.add(object.clone());
            let mut editor = EditorOverlay {
                object,
                offset: 3,
                anchor: 0,
                preedit: 0..0,
                visible: true,
                reference: None,
            };
            let mut compiler = SceneCompiler::default();
            let compile = |compiler: &mut SceneCompiler, editor: &EditorOverlay| {
                compiler
                    .interaction(&RenderInput {
                        edit: None,
                        session: &session,
                        viewport: view,
                        scale,
                        draft: None,
                        hovered: None,
                        reference: None,
                        editor: Some(editor),
                    })
                    .unwrap()[0]
                    .clone()
            };
            let shown = compile(&mut compiler, &editor);
            editor.visible = false;
            let hidden = compile(&mut compiler, &editor);
            assert_ne!(shown.pixels, hidden.pixels);
            for tick in 0..12 {
                editor.visible = tick % 2 == 0;
                let result = compile(&mut compiler, &editor);
                let expected = if editor.visible { &shown } else { &hidden };
                assert_eq!(
                    result.key, expected.key,
                    "blink should reuse the original raster key"
                );
                assert!(std::sync::Arc::ptr_eq(&result.pixels, &expected.pixels));
            }
            // Switch back to the older key before editing, exercising the
            // revision-collision hazard of a two-visibility cache.
            editor.visible = true;
            assert_eq!(compile(&mut compiler, &editor).key, shown.key);
            let shown_weak = std::sync::Arc::downgrade(&shown.pixels);
            let hidden_weak = std::sync::Arc::downgrade(&hidden.pixels);
            // Moving the insertion point must not revive an older raster key.
            editor.offset = 6;
            editor.anchor = 6;
            editor.visible = true;
            let changed = compile(&mut compiler, &editor);
            assert!(changed.key.revision > shown.key.revision.max(hidden.key.revision));
            assert_ne!(changed.pixels, shown.pixels);
            drop(shown);
            drop(hidden);
            assert!(shown_weak.upgrade().is_none());
            assert!(hidden_weak.upgrade().is_none());
            // The old pair is discarded and the next pair reuses only new pixels.
            editor.visible = false;
            let changed_hidden = compile(&mut compiler, &editor);
            editor.visible = true;
            assert!(std::sync::Arc::ptr_eq(
                &compile(&mut compiler, &editor).pixels,
                &changed.pixels
            ));
            editor.visible = false;
            assert!(std::sync::Arc::ptr_eq(
                &compile(&mut compiler, &editor).pixels,
                &changed_hidden.pixels
            ));
            // Style, reference baseline and IME changes discard both variants.
            for change in 0..5 {
                editor.visible = true;
                match change {
                    0 => editor.object.style.color = [0, 180, 40],
                    1 => editor.reference = Some("abc".into()),
                    2 => {
                        editor.preedit = 0..3;
                        editor.offset = 3;
                    }
                    3 => {
                        if let Extra::Text(text) = &mut editor.object.extra {
                            text.font_size = 36.;
                            text.font_family = "Arial".into();
                        }
                    }
                    _ => {
                        if let Extra::Text(text) = &mut editor.object.extra {
                            text.content.push('字');
                        }
                    }
                }
                let current = compile(&mut compiler, &editor);
                editor.visible = false;
                let invisible = compile(&mut compiler, &editor);
                editor.visible = true;
                assert!(std::sync::Arc::ptr_eq(
                    &compile(&mut compiler, &editor).pixels,
                    &current.pixels
                ));
                for visible in [true, false] {
                    editor.visible = visible;
                    let input = RenderInput {
                        edit: None,
                        session: &session,
                        viewport: view,
                        scale,
                        draft: None,
                        hovered: None,
                        reference: None,
                        editor: Some(&editor),
                    };
                    let actual = compiler.render_scene(input).unwrap();
                    let mut expected =
                        annotation_render::render_editing(&session, view, scale, None, None, None)
                            .unwrap();
                    editor.draw(&mut expected, view, scale);
                    assert!(
                        actual
                            .data()
                            .iter()
                            .zip(expected.data())
                            .all(|(a, b)| a.abs_diff(*b) <= 1)
                    );
                }
                let _ = invisible;
            }
            compiler.retain_viewports(&[]);
            assert!(compiler.interactions.is_empty());
            compiler.clear();
        }
    }

    #[test]
    fn caret_selection_and_preedit_are_local_and_do_not_invalidate_document_assets() {
        let view = Rect {
            left: -512,
            top: -384,
            right: 512,
            bottom: 384,
        };
        for scale in [1., 1.25] {
            let mut text = TextAnnotation {
                content: "中a文\nJf".into(),
                font_family: "Microsoft YaHei".into(),
                font_size: 32.,
                bold: false,
                italic: false,
                outline: None,
                background: None,
                rotation: 0.25,
                layout_scale: scale,
                linked_arrow: None,
                linked_arrow_control: None,
            };
            let mut object = Object {
                erased: Vec::new(),
                id: ObjectId::DRAFT,
                kind: Kind::Text,
                bounds: annotation_text::text_bounds((-100., -60.), &text, scale),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Text(text.clone()),
            };
            let mut session = Session::new();
            session.add(object.clone());
            let mut compiler = SceneCompiler::default();
            let original = compiler
                .document(&session, view, scale, None, None)
                .unwrap();
            let mut editor = EditorOverlay {
                object: object.clone(),
                offset: 3,
                anchor: 0,
                preedit: 0..0,
                visible: true,
                reference: None,
            };
            for state in 0..3 {
                if state == 1 {
                    editor.visible = false;
                }
                if state == 2 {
                    editor.visible = true;
                    editor.preedit = 3..4;
                    editor.offset = 4;
                    editor.anchor = 4;
                }
                let input = RenderInput {
                    edit: None,
                    session: &session,
                    viewport: view,
                    scale,
                    draft: None,
                    hovered: None,
                    reference: None,
                    editor: Some(&editor),
                };
                let overlays = compiler.interaction(&input).unwrap();
                assert_eq!(overlays.len(), 1);
                assert!(overlays[0].pixels.len() < 1024 * 768 * 4);
                let repeated = compiler.interaction(&input).unwrap();
                assert_eq!(overlays[0].key, repeated[0].key);
                assert!(std::sync::Arc::ptr_eq(
                    &overlays[0].pixels,
                    &repeated[0].pixels
                ));
                let actual = compiler.render_scene(input).unwrap();
                let mut expected =
                    annotation_render::render_editing(&session, view, scale, None, None, None)
                        .unwrap();
                editor.draw(&mut expected, view, scale);
                let maximum = actual
                    .data()
                    .iter()
                    .zip(expected.data())
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    maximum <= 1,
                    "dpi {scale} state {state} maximum difference {maximum}"
                );
                let document = compiler
                    .document(&session, view, scale, None, None)
                    .unwrap();
                assert_eq!(original[0].key, document[0].key);
                assert!(std::sync::Arc::ptr_eq(
                    &original[0].pixels,
                    &document[0].pixels
                ));
            }
            // An empty draft must still generate the caret at its metric baseline.
            text.content.clear();
            object.extra = Extra::Text(text);
            editor.object = object;
            editor.offset = 0;
            editor.anchor = 0;
            editor.preedit = 0..0;
            let mut empty = tiny_skia::Pixmap::new(1024, 768).unwrap();
            editor.draw(&mut empty, view, scale);
            assert!(
                empty
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 0)
            );
            compiler.clear();
            assert!(compiler.interactions.is_empty());
        }
    }
}

impl EditorOverlay {
    pub fn draw(&self, frame: &mut tiny_skia::Pixmap, viewport: Rect, scale: f32) {
        let mut bounds = self.object.bounds;
        include_text(&mut bounds, &self.object);
        let padding = 18. * scale + 2.;
        let geometry = Rect {
            left: (bounds.left - padding).floor() as i32,
            top: (bounds.top - padding).floor() as i32,
            right: (bounds.right + padding).ceil() as i32,
            bottom: (bounds.bottom + padding).ceil() as i32,
        };
        crate::annotation_path::draw_cells(frame, viewport, geometry, |frame, origin| {
            self.draw_content(frame, origin, scale)
        });
    }

    fn draw_content(&self, frame: &mut tiny_skia::Pixmap, viewport: Rect, scale: f32) {
        if self.preedit.is_empty() {
            annotation_text::draw_selection(
                frame,
                &self.object,
                self.anchor.min(self.offset)..self.anchor.max(self.offset),
                viewport,
            );
        }
        annotation_text::draw_composition_underline(
            frame,
            &self.object,
            self.preedit.clone(),
            viewport,
            scale,
        );
        if self.visible {
            annotation_text::draw_caret(
                frame,
                &self.object,
                self.offset,
                viewport,
                scale,
                self.reference.as_deref(),
            );
        }
    }
}

#[derive(PartialEq)]
struct State {
    erase: Option<(
        lexift_core::domain::annotation_erase::RoundedRectangle,
        bool,
    )>,
    selected: Option<Object>,
    hovered: Option<Object>,
    editor: Option<EditorOverlay>,
    scale: f32,
}

impl State {
    fn caret_toggle_of(&self, previous: &Self) -> bool {
        self.erase == previous.erase
            && self.selected == previous.selected
            && self.hovered == previous.hovered
            && self.scale == previous.scale
            && matches!((&self.editor, &previous.editor), (Some(a), Some(b))
                if a.visible != b.visible && a.object == b.object
                    && a.offset == b.offset && a.anchor == b.anchor
                    && a.preedit == b.preedit && a.reference == b.reference)
    }
}

pub(super) struct CacheEntry {
    state: State,
    raster: RasterLayer,
    // At most the other visibility of this exact editing/selection state.
    // Parameter, content, insertion, geometry and viewport changes discard it.
    alternate: Option<RasterLayer>,
}

fn extend_object(bounds: &mut Bounds, object: &Object, scale: f32) {
    include(bounds, (object.bounds.left, object.bounds.top));
    include(bounds, (object.bounds.right, object.bounds.bottom));
    include_text(bounds, object);
    for point in object.edit_handles(scale) {
        include(bounds, point);
    }
}

impl SceneCompiler {
    /// Compile only selected/hovered geometry and active editor feedback. The
    /// cache stores neither the document nor undo history, and owns no timers.
    pub fn interaction(&mut self, input: &RenderInput<'_>) -> Option<Vec<RasterLayer>> {
        self.interaction_document(
            input,
            super::Document {
                session: input.session,
                edit: input.edit,
            },
        )
    }

    pub(super) fn interaction_document(
        &mut self,
        input: &RenderInput<'_>,
        document: super::Document<'_>,
    ) -> Option<Vec<RasterLayer>> {
        let selected = document
            .selected()
            .and_then(|i| document.get(i))
            .filter(|o| o.is_editable())
            .cloned();
        let hovered = input
            .hovered
            .and_then(|i| document.get(i))
            .filter(|o| {
                matches!(
                    o.kind,
                    Kind::Rectangle | Kind::Ellipse | Kind::HighlightRectangle
                )
            })
            .cloned();
        let state = State {
            erase: input
                .session
                .rectangle_erase_geometry()
                .map(|geometry| (geometry, input.session.rectangle_erase_show_handles())),
            selected,
            hovered,
            editor: input.editor.cloned(),
            scale: input.scale,
        };
        let view = rectangle_key(input.viewport);
        let mut objects = state
            .selected
            .iter()
            .chain(state.hovered.iter())
            .chain(state.editor.iter().map(|e| &e.object));
        let mut bounds = if let Some(first) = objects.next() {
            let mut bounds = first.bounds;
            extend_object(&mut bounds, first, input.scale);
            bounds
        } else if let Some((geometry, _)) = state.erase {
            geometry.bounds
        } else {
            self.interactions.remove(&view);
            return Some(Vec::new());
        };
        for object in objects {
            extend_object(&mut bounds, object, input.scale);
        }
        if let Some((geometry, _)) = state.erase {
            include(&mut bounds, (geometry.bounds.left, geometry.bounds.top));
            include(&mut bounds, (geometry.bounds.right, geometry.bounds.bottom));
        }
        let padding = 18. * input.scale + 2.;
        let bounds = crate::annotation_path::intersect(
            Rect {
                left: (bounds.left - padding).floor() as i32,
                top: (bounds.top - padding).floor() as i32,
                right: (bounds.right + padding).ceil() as i32,
                bottom: (bounds.bottom + padding).ceil() as i32,
            },
            input.viewport,
        );
        let Some(bounds) = bounds else {
            self.interactions.remove(&view);
            return Some(Vec::new());
        };
        let blink = self.interactions.get(&view).is_some_and(|entry| {
            entry.raster.bounds == bounds && state.caret_toggle_of(&entry.state)
        });
        if blink
            && let Some(entry) = self.interactions.get_mut(&view)
            && let Some(alternate) = entry.alternate.take()
        {
            entry.alternate = Some(std::mem::replace(&mut entry.raster, alternate));
            entry.state = state;
            return Some(vec![entry.raster.clone()]);
        }
        let entry = self.interactions.get(&view);
        if entry.is_none_or(|entry| entry.state != state || entry.raster.bounds != bounds) {
            // Returning an older visibility key must never let a later content
            // update reuse the key of a different cached pixel buffer.
            let old = entry.map(|entry| {
                entry
                    .alternate
                    .as_ref()
                    .filter(|other| other.key.revision > entry.raster.key.revision)
                    .map_or(entry.raster.key, |other| other.key)
            });
            let alternate = if blink {
                entry.map(|entry| entry.raster.clone())
            } else {
                None
            };
            let mut frame = pixmap(bounds)?;
            let mut interaction = Session::new();
            if let Some(selected) = &state.selected {
                interaction.selected = Some(0);
                interaction.objects.push(selected.clone());
            }
            let hovered = state.hovered.as_ref().map(|object| {
                let index = interaction.objects.len();
                interaction.objects.push(object.clone());
                index
            });
            annotation_render::draw_interaction(
                &mut frame,
                &interaction,
                bounds,
                input.scale,
                hovered,
            );
            if let Some((geometry, handles)) = state.erase {
                annotation_render::draw_erase_interaction(
                    &mut frame,
                    geometry,
                    handles,
                    bounds,
                    input.scale,
                );
            }
            if let Some(editor) = input.editor {
                editor.draw(&mut frame, bounds, input.scale);
            }
            let raster = self.raster(frame, bounds, old);
            self.interactions.insert(
                view,
                CacheEntry {
                    state,
                    raster,
                    alternate,
                },
            );
        }
        Some(vec![self.interactions[&view].raster.clone()])
    }
}
