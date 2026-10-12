//! Eraser capture and visible-target resolution share the document's raster coverage.
use lexift_core::domain::{
    annotation::{Bounds, Extra, Kind, Session},
    annotation_erase::{
        EraseRegion, EraseTarget, ErasedCoverage, RectangleEraseHit, RoundedRectangle,
    },
    geometry::Rect,
};
use tiny_skia::{Paint, Pixmap};

pub(super) struct Press {
    start: (f32, f32),
    last: (f32, f32),
    pub diameter: f32,
    brush: bool,
    edit: Option<(RoundedRectangle, RectangleEraseHit)>,
    scale: f32,
}

pub(super) fn has_coverage(session: &Session) -> bool {
    !session.watermark_erased.is_empty()
        || !session.spotlight_erased.is_empty()
        || session
            .objects
            .iter()
            .any(|object| !object.erased.is_empty())
}
impl Press {
    pub fn begin(
        session: &mut Session,
        point: (f32, f32),
        brush: bool,
        diameter: f32,
        scale: f32,
        views: &[(Rect, f32)],
    ) -> Self {
        let edit = if !brush {
            session
                .rectangle_erase_geometry()
                .and_then(|geometry| geometry.hit(point, scale).map(|hit| (geometry, hit)))
        } else {
            None
        };
        if edit.is_some() {
            session.begin_drag();
        } else {
            session.finish_rectangle_erase();
            if brush {
                session.begin_drag();
            } else {
                session.begin_rectangle_erase(RoundedRectangle {
                    bounds: Bounds::from_corners(point, point),
                    rounding: lexift_core::domain::annotation::Style::default().rounding,
                    creation_scale: scale,
                });
            }
        }
        let mut press = Self {
            start: point,
            last: point,
            diameter,
            brush,
            edit,
            scale,
        };
        press.update(session, point, views);
        press
    }
    pub fn update(&mut self, session: &mut Session, point: (f32, f32), views: &[(Rect, f32)]) {
        let region = if self.brush {
            EraseRegion::Brush {
                points: if point == self.last {
                    vec![point]
                } else {
                    vec![self.last, point]
                },
                diameter: self.diameter,
            }
        } else {
            let Some(mut geometry) = session.rectangle_erase_geometry() else {
                return;
            };
            if let Some((before, hit)) = self.edit {
                geometry = before;
                match hit {
                    RectangleEraseHit::Body => {
                        let (dx, dy) = (point.0 - self.start.0, point.1 - self.start.1);
                        geometry.bounds = Bounds {
                            left: before.bounds.left + dx,
                            top: before.bounds.top + dy,
                            right: before.bounds.right + dx,
                            bottom: before.bounds.bottom + dy,
                        };
                    }
                    RectangleEraseHit::Handle(handle) => {
                        geometry.edit_handle(handle, point, self.scale)
                    }
                }
            } else {
                geometry.bounds = Bounds::from_corners(self.start, point);
            }
            let Some(region) = session.prepare_rectangle_erase(geometry) else {
                return;
            };
            region
        };
        let targets = visible_targets(session, &region, views);
        if self.brush {
            session.erase(region, &targets);
        } else {
            session.apply_rectangle_erase(region, &targets);
        }
        self.last = point;
    }
    pub fn edit_hit(&self) -> Option<RectangleEraseHit> {
        self.edit.map(|(_, hit)| hit)
    }
}

/// Compare only the affected physical ROI. This rejects blank and repeated cuts,
/// including shape interiors and transparent text pixels, rather than guessing by bounds.
fn changed(frame: &mut Pixmap, roi: Rect, region: &EraseRegion) -> bool {
    let before = frame.data().to_vec();
    crate::annotation_render::erase_coverage(frame, roi, &[ErasedCoverage::new(region.clone())]);
    before != frame.data()
}
pub(super) fn visible_targets(
    session: &Session,
    region: &EraseRegion,
    views: &[(Rect, f32)],
) -> Vec<EraseTarget> {
    let mut targets = Vec::new();
    let b = region.bounds();
    let roi = Rect {
        left: b.left.floor() as i32,
        top: b.top.floor() as i32,
        right: b.right.ceil() as i32,
        bottom: b.bottom.ceil() as i32,
    };
    for &(view, scale) in views {
        let Some(roi) = crate::annotation_render::mosaic::intersect(roi, view) else {
            continue;
        };
        let Some(mut frame) =
            Pixmap::new((roi.right - roi.left) as u32, (roi.bottom - roi.top) as u32)
        else {
            continue;
        };
        for object in &session.objects {
            if object.kind.is_spotlight() {
                continue;
            }
            frame.fill(tiny_skia::Color::TRANSPARENT);
            match &object.extra {
                Extra::Mosaic(m) if m.strength > 0 => {
                    let mut white = Paint::default();
                    white.set_color_rgba8(255, 255, 255, 255);
                    crate::annotation_render::mosaic::paint_mask(&mut frame, object, roi, &white);
                }
                Extra::Magnifier { .. } => {
                    crate::annotation_render::draw_magnifier_mask(&mut frame, object, roi);
                    crate::annotation_render::draw_magnifier_overlay(
                        &mut frame, object, roi, scale,
                    );
                }
                Extra::Mosaic(_) => continue,
                _ => crate::annotation_render::draw_content_object(
                    &mut frame, object, roi, scale, None,
                ),
            }
            if object.kind != Kind::Magnifier {
                crate::annotation_render::erase_coverage(&mut frame, roi, &object.erased);
            }
            let target = EraseTarget::Object(object.id);
            if !targets.contains(&target) && changed(&mut frame, roi, region) {
                targets.push(target);
            }
        }
        // Independent global targets must not include each other's pixels.
        for (target, enabled) in [
            (EraseTarget::Watermark, session.watermark.is_some()),
            (
                EraseTarget::Spotlight,
                session.objects.iter().any(|o| o.kind.is_spotlight()),
            ),
        ] {
            if !enabled || targets.contains(&target) {
                continue;
            }
            frame.fill(tiny_skia::Color::TRANSPARENT);
            let mut backdrop = Session::new();
            backdrop.displays = if session.displays.is_empty() {
                views.iter().map(|(view, _)| *view).collect()
            } else {
                session.displays.clone()
            };
            if target == EraseTarget::Watermark {
                backdrop.watermark = session.watermark.clone();
                backdrop.watermark_erased = session.watermark_erased.clone();
            } else {
                backdrop.objects = session
                    .objects
                    .iter()
                    .filter(|o| o.kind.is_spotlight())
                    .cloned()
                    .collect();
                backdrop.spotlight_opacity = session.spotlight_opacity;
                backdrop.spotlight_erased = session.spotlight_erased.clone();
            }
            let _ =
                crate::annotation_render::draw_backdrop(&mut frame, &backdrop, roi, scale, None);
            if changed(&mut frame, roi, region) {
                targets.push(target);
            }
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{
        EditScope, Kind, MosaicAnnotation, MosaicEffect, Object, ObjectId, Style,
    };
    pub(super) fn object(kind: Kind, scale: f32) -> Object {
        Object {
            id: ObjectId::DRAFT,
            erased: Vec::new(),
            kind,
            bounds: Bounds::from_corners((-80., -60.), (80., 60.)),
            points: vec![(-80., 0.), (80., 0.)],
            style: Style {
                fill: !matches!(kind, Kind::Polyline),
                rounding: 0.,
                width: 12.,
                ..Style::default()
            },
            extra: if matches!(kind, Kind::MosaicArea | Kind::MosaicBrush) {
                Extra::Mosaic(MosaicAnnotation::new(
                    MosaicEffect::Blur,
                    100,
                    12. * scale,
                    scale,
                ))
            } else {
                Extra::None
            },
        }
    }
    fn views(scale: f32) -> [(Rect, f32); 1] {
        [(
            Rect {
                left: -120,
                top: -100,
                right: 120,
                bottom: 100,
            },
            scale,
        )]
    }
    #[test]
    fn annotation_erase_rectangle_four_directions_blank_repeat_cancel_and_undo() {
        for scale in [1., 1.25] {
            for (start, end) in [
                ((-20., -20.), (20., 20.)),
                ((20., 20.), (-20., -20.)),
                ((-20., 20.), (20., -20.)),
                ((20., -20.), (-20., 20.)),
            ] {
                let mut s = Session::new();
                s.add(object(Kind::Rectangle, scale));
                s.set_edit_scope(EditScope::None);
                let mut press =
                    Press::begin(&mut s, start, false, 12. * scale, scale, &views(scale));
                press.update(&mut s, end, &views(scale));
                s.finish_drag();
                assert!(s.objects[0].erased[0].contains((0., 0.)));
                s.finish_rectangle_erase();
                let mut again =
                    Press::begin(&mut s, start, false, 12. * scale, scale, &views(scale));
                again.update(&mut s, end, &views(scale));
                s.finish_drag();
                assert_eq!(s.objects[0].erased.len(), 1);
                assert!(s.undo());
                assert!(s.objects[0].erased.is_empty());
                let mut blank = Press::begin(&mut s, (90., 70.), false, 12., scale, &views(scale));
                blank.update(&mut s, (110., 90.), &views(scale));
                s.finish_drag();
                assert!(s.undo());
                assert!(!s.has_content());
            }
        }
    }
    #[test]
    fn annotation_erase_brush_fast_segments_single_point_overlaps_and_later_drawing() {
        for scale in [1., 1.25] {
            let mut s = Session::new();
            for kind in [
                Kind::Pencil,
                Kind::Rectangle,
                Kind::HighlightLine,
                Kind::HighlightRectangle,
                Kind::Arrow,
                Kind::Polyline,
                Kind::MosaicArea,
                Kind::MosaicBrush,
            ] {
                s.add(object(kind, scale));
            }
            s.set_edit_scope(EditScope::None);
            let before = s.objects.clone();
            let mut press =
                Press::begin(&mut s, (-70., 0.), true, 12. * scale, scale, &views(scale));
            press.update(&mut s, (70., 0.), &views(scale));
            s.finish_drag();
            for o in &s.objects {
                assert!(
                    o.erased.iter().any(|c| c.contains((0., 0.))),
                    "{:?}",
                    o.kind
                );
            }
            s.undo();
            assert_eq!(s.objects, before);
            let _press = Press::begin(&mut s, (0., 0.), true, 12. * scale, scale, &views(scale));
            assert!(!s.objects[0].erased.is_empty());
            s.cancel_drag();
            assert_eq!(s.objects, before);
            let _press = Press::begin(&mut s, (0., 0.), true, 12. * scale, scale, &views(scale));
            s.finish_drag();
            s.add(object(Kind::Rectangle, scale));
            assert!(s.objects.last().unwrap().erased.is_empty());
        }
    }
    #[test]
    fn annotation_erase_clips_effect_masks_and_invalidates_scene_reuse() {
        use crate::annotation_scene::SceneCompiler;
        use lexift_core::ports::annotation_render::RenderLayer;
        for scale in [1., 1.25] {
            for kind in [Kind::MosaicArea, Kind::MosaicBrush, Kind::Magnifier] {
                let mut s = Session::new();
                let mut o = object(kind, scale);
                if kind == Kind::Magnifier {
                    o.extra = Extra::Magnifier {
                        output: o.bounds,
                        ellipse: false,
                        zoom: 2.,
                        connector: lexift_core::domain::annotation::MagnifierConnector::Plain,
                        erase_annotations: false,
                        antialias: true,
                        shadow: true,
                    };
                }
                s.add(o);
                s.selected = None;
                let mut compiler = SceneCompiler::default();
                let before = compiler.layers(&s, &views(scale), None, None).unwrap();
                let cut = EraseRegion::Rectangle(Bounds::from_corners((-20., -20.), (20., 20.)));
                let targets = visible_targets(&s, &cut, &views(scale));
                assert_eq!(targets.len(), 1);
                s.erase(cut, &targets);
                let after = compiler.layers(&s, &views(scale), None, None).unwrap();
                let mask = |layers: Vec<RenderLayer>| {
                    layers
                        .into_iter()
                        .find_map(|l| match l {
                            RenderLayer::Mosaic(m) => Some(m.masks[0].clone()),
                            RenderLayer::Magnifier(m) => Some(m.masks[0].clone()),
                            _ => None,
                        })
                        .unwrap()
                };
                let a = mask(before);
                let b = mask(after);
                assert_ne!(a.key, b.key);
                let i = ((-b.bounds.top) * (b.bounds.right - b.bounds.left) - b.bounds.left)
                    as usize
                    * 4
                    + 3;
                assert_eq!(b.pixels[i], 0);
                s.undo();
                let restored = mask(compiler.layers(&s, &views(scale), None, None).unwrap());
                assert_eq!(restored.pixels.as_ref(), a.pixels.as_ref());
            }
        }
    }
    #[test]
    fn annotation_erase_empty_shape_interiors_have_no_history() {
        let mut s = Session::new();
        let mut o = object(Kind::Rectangle, 1.);
        o.style.fill = false;
        s.add(o);
        s.selected = None;
        let mut press = Press::begin(&mut s, (-10., -10.), false, 12., 1., &views(1.));
        press.update(&mut s, (10., 10.), &views(1.));
        s.finish_drag();
        assert!(s.objects[0].erased.is_empty());
        s.undo();
        assert!(!s.has_content());
    }
    #[test]
    fn annotation_erase_ordinary_layers_and_global_backdrops_match_software_and_gpu_rasters() {
        use lexift_core::domain::annotation::{Watermark, WatermarkPosition};
        for scale in [1., 1.25] {
            let mut s = Session::new();
            s.add(object(Kind::SpotlightRectangle, scale));
            s.objects[0].bounds = Bounds::from_corners((-110., -90.), (-85., -65.));
            s.set_watermark(Some(Watermark {
                template: "WM".into(),
                content: "WM".into(),
                font_family: "Segoe UI".into(),
                font_size: 24.,
                opacity: 255,
                color: [220, 70, 20],
                position: WatermarkPosition::Center,
            }));
            s.selected = None;
            let roi = views(scale)[0].0;
            let cut = EraseRegion::Rectangle(Bounds::from_corners((-50., -50.), (50., 50.)));
            let targets = visible_targets(&s, &cut, &views(scale));
            assert!(
                targets.contains(&EraseTarget::Watermark)
                    && targets.contains(&EraseTarget::Spotlight)
            );
            s.erase(cut, &targets);
            let mut compiler = crate::annotation_scene::SceneCompiler::default();
            let expected = crate::annotation_render::render(&s, roi, scale, None, None).unwrap();
            let actual = compiler.render(&s, roi, scale, None, None, None).unwrap();
            assert_eq!(actual.data(), expected.data());
            assert_eq!(actual.pixel(120, 100).unwrap().alpha(), 0);
            assert!(actual.pixel(190, 150).unwrap().alpha() > 0);
            // A newly drawn object covers the cut while the watermark/backdrop remain cut.
            s.add(object(Kind::Rectangle, scale));
            s.selected = None;
            let actual = compiler.render(&s, roi, scale, None, None, None).unwrap();
            assert_eq!(actual.pixel(120, 100).unwrap().alpha(), 255);
            s.undo();
            s.undo();
            let restored = compiler.render(&s, roi, scale, None, None, None).unwrap();
            assert!(restored.pixel(120, 100).unwrap().alpha() > 0);
        }
    }
    #[test]
    fn annotation_erase_text_sequence_caption_connectors_ellipses_clip_as_whole_targets() {
        use lexift_core::domain::annotation::{
            Endpoint, SequenceAnnotation, SequenceFormat, SequenceShape,
        };
        for scale in [1., 1.25] {
            for kind in [Kind::Text, Kind::Sequence, Kind::Ellipse] {
                let mut s = Session::new();
                let mut o = object(kind, scale);
                let mut text = super::super::parameters::text(
                    &super::super::parameters::defaults(8),
                    "Segoe UI".into(),
                    "ABC".into(),
                );
                text.linked_arrow = Some((90., 70.));
                if kind == Kind::Text {
                    o.extra = Extra::Text(text.clone());
                }
                if kind == Kind::Sequence {
                    o.extra = Extra::Sequence(SequenceAnnotation {
                        value: 12,
                        format: SequenceFormat::Decimal,
                        shape: SequenceShape::Circle,
                        size: 16.,
                        scale,
                        caption: text,
                        caption_bounds: Bounds::from_corners((30., -30.), (100., 20.)),
                        caption_color: [230, 40, 10],
                        caption_custom_color: true,
                        caption_ratio: 2,
                        guide: true,
                        endpoint: Endpoint::FilledArrow,
                    });
                    crate::annotation_sequence::layout(&mut o);
                }
                s.add(o);
                s.selected = None;
                let cut = EraseRegion::Rectangle(Bounds::from_corners((-100., -90.), (105., 80.)));
                let targets = visible_targets(&s, &cut, &views(scale));
                assert_eq!(targets.len(), 1, "{kind:?}");
                s.erase(cut, &targets);
                let mut compiler = crate::annotation_scene::SceneCompiler::default();
                let actual = compiler
                    .render(&s, views(scale)[0].0, scale, None, None, None)
                    .unwrap();
                let expected =
                    crate::annotation_render::render(&s, views(scale)[0].0, scale, None, None)
                        .unwrap();
                assert_eq!(actual.data(), expected.data());
                assert!(actual.data().iter().all(|v| *v == 0), "{kind:?}");
            }
        }
    }
    #[test]
    fn annotation_erase_brush_popup_uses_own_size_protocol_and_keeps_sequence_color_protocol() {
        let request = crate::AnnotationChoiceRequest {
            field: -9,
            options: Default::default(),
            selected: 0,
            lines: false,
            connectors: false,
            x: 0.,
            y: 0.,
            width: 80.,
            height: 38.,
        };
        // The sequence's custom text color is field -8, distinct from brush size.
        assert!(super::super::parameters::ColorTarget::from_field(request.field).is_none());
        assert!(super::super::parameters::ColorTarget::from_field(-8).is_some());
        let mut values = super::super::parameters::defaults(13);
        super::super::parameters::Edit::Numeric(super::super::numeric::Parameter::Size, 65)
            .apply(&mut values);
        assert_eq!(values.size, 65);
        assert_eq!(values.shape, 0);
        assert!(matches!(
            super::super::tools::drawing(13, 0, true),
            super::super::tools::Drawing::Erase
        ));
        assert_eq!(super::super::tools::scope(13), EditScope::None);
    }
    #[test]
    fn annotation_erase_translation_preserves_rgba_of_remaining_strokes() {
        for scale in [1., 1.25] {
            let mut s = Session::new();
            let mut o = object(Kind::Rectangle, scale);
            o.style.fill = false;
            s.add(o);
            s.selected = None;
            let mut compiler = crate::annotation_scene::SceneCompiler::default();
            compiler
                .render(&s, views(scale)[0].0, scale, None, None, None)
                .unwrap();
            for _ in 0..3 {
                s.begin_drag();
                s.erase(
                    EraseRegion::Rectangle(Bounds::from_corners((-50., -70.), (50., -50.))),
                    &[EraseTarget::Object(s.objects[0].id)],
                );
                s.finish_drag();
                compiler
                    .render(&s, views(scale)[0].0, scale, None, None, None)
                    .unwrap();
                s.undo();
                compiler
                    .render(&s, views(scale)[0].0, scale, None, None, None)
                    .unwrap();
            }
            s.clear_all();
            compiler
                .render(&s, views(scale)[0].0, scale, None, None, None)
                .unwrap();
            s.undo();
            s.erase(
                EraseRegion::Brush {
                    points: vec![(-20., -60.)],
                    diameter: 12. * scale,
                },
                &[EraseTarget::Object(s.objects[0].id)],
            );
            s.selected = Some(0);
            s.update_selected(|o| o.move_by(30., 0.));
            s.selected = None;
            let actual = compiler
                .render(&s, views(scale)[0].0, scale, None, None, None)
                .unwrap();
            assert_eq!(s.objects[0].style.color, [217, 71, 43]);
            let pixel = actual.pixel(170, 40).unwrap();
            assert_eq!(
                [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()],
                [217, 71, 43, 255]
            );
        }
    }
}

#[cfg(test)]
mod rectangle_edit_tests {
    use super::*;
    use lexift_core::domain::annotation::{Object, ObjectId, Style};
    fn views(scale: f32) -> [(Rect, f32); 1] {
        [(
            Rect {
                left: -150,
                top: -120,
                right: 150,
                bottom: 120,
            },
            scale,
        )]
    }
    fn document() -> Session {
        let mut s = Session::new();
        s.add(Object {
            id: ObjectId::DRAFT,
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-120., -90.), (120., 90.)),
            points: vec![],
            style: Style {
                fill: true,
                rounding: 0.,
                ..Style::default()
            },
            extra: Extra::None,
            erased: vec![],
        });
        s.selected = None;
        s
    }
    fn create(s: &mut Session, scale: f32) {
        let mut press = Press::begin(s, (-50., -40.), false, 12. * scale, scale, &views(scale));
        press.update(s, (50., 40.), &views(scale));
        s.finish_drag();
    }
    #[test]
    fn rectangle_erase_blank_creation_can_move_into_existing_content() {
        for scale in [1., 1.25] {
            let mut s = document();
            s.objects[0].bounds = Bounds::from_corners((30., 20.), (120., 90.));
            let mut press = Press::begin(&mut s, (-120., -90.), false, 12., scale, &views(scale));
            press.update(&mut s, (-20., -10.), &views(scale));
            s.finish_drag();
            assert!(s.objects[0].erased.is_empty());
            assert!(s.rectangle_erase_show_handles());
            let mut press = Press::begin(&mut s, (-70., -50.), false, 12., scale, &views(scale));
            assert_eq!(press.edit_hit(), Some(RectangleEraseHit::Body));
            press.update(&mut s, (70., 50.), &views(scale));
            s.finish_drag();
            assert!(
                s.objects[0]
                    .erased
                    .iter()
                    .any(|cut| cut.contains((70., 50.)))
            );
            let mut press = Press::begin(&mut s, (70., 50.), false, 12., scale, &views(scale));
            press.update(&mut s, (-70., -50.), &views(scale));
            s.cancel_drag();
            assert!(s.rectangle_erase_show_handles());
            assert!(
                s.objects[0]
                    .erased
                    .iter()
                    .any(|cut| cut.contains((70., 50.)))
            );
            assert!(s.undo());
            assert!(s.objects[0].erased.is_empty());
        }
    }
    #[test]
    fn rectangle_erase_ui_move_resize_round_cancel_and_finalize() {
        for scale in [1., 1.25] {
            let mut s = document();
            create(&mut s, scale);
            let original = s.rectangle_erase_geometry().unwrap();
            let mut press =
                Press::begin(&mut s, (0., 0.), false, 12. * scale, scale, &views(scale));
            assert_eq!(press.edit_hit(), Some(RectangleEraseHit::Body));
            press.update(&mut s, (30., 0.), &views(scale));
            s.finish_drag();
            assert!(!s.objects[0].erased[0].contains((-40., 0.)));
            assert!(s.objects[0].erased[0].contains((60., 0.)));
            let moved = s.rectangle_erase_geometry().unwrap();
            let mut press = Press::begin(
                &mut s,
                moved.handles(scale)[7],
                false,
                12. * scale,
                scale,
                &views(scale),
            );
            assert_eq!(press.edit_hit(), Some(RectangleEraseHit::Handle(7)));
            press.update(&mut s, (100., 70.), &views(scale));
            s.cancel_drag();
            assert_eq!(s.rectangle_erase_geometry(), Some(moved));
            let mut press = Press::begin(
                &mut s,
                moved.handles(scale)[8],
                false,
                12. * scale,
                scale,
                &views(scale),
            );
            press.update(&mut s, (moved.bounds.left, moved.bounds.top), &views(scale));
            s.finish_drag();
            assert_eq!(s.rectangle_erase_geometry().unwrap().rounding, 0.);
            s.finish_rectangle_erase();
            assert!(s.rectangle_erase_geometry().is_none());
            assert_ne!(moved, original);
        }
    }
    #[test]
    fn rectangle_erase_rounded_effect_masks_preserve_corner_pixels() {
        use lexift_core::ports::annotation_render::RenderLayer;
        for scale in [1., 1.25] {
            for kind in [Kind::MosaicArea, Kind::Magnifier] {
                let mut s = Session::new();
                let mut object = super::tests::object(kind, scale);
                if kind == Kind::Magnifier {
                    object.extra = Extra::Magnifier {
                        output: object.bounds,
                        ellipse: false,
                        zoom: 2.,
                        connector: lexift_core::domain::annotation::MagnifierConnector::Plain,
                        erase_annotations: false,
                        antialias: true,
                        shadow: true,
                    };
                }
                s.add(object);
                s.selected = None;
                s.erase(
                    EraseRegion::RoundedRectangle {
                        bounds: Bounds::from_corners((-30., -30.), (30., 30.)),
                        radius: 25.,
                    },
                    &[EraseTarget::Object(s.objects[0].id)],
                );
                let mut compiler = crate::annotation_scene::SceneCompiler::default();
                let layers = compiler.layers(&s, &views(scale), None, None).unwrap();
                let mask = layers
                    .into_iter()
                    .find_map(|layer| match layer {
                        RenderLayer::Mosaic(m) => Some(m.masks[0].clone()),
                        RenderLayer::Magnifier(m) => Some(m.masks[0].clone()),
                        _ => None,
                    })
                    .unwrap();
                let alpha = |x: i32, y: i32| {
                    mask.pixels[((y - mask.bounds.top) * (mask.bounds.right - mask.bounds.left)
                        + (x - mask.bounds.left)) as usize
                        * 4
                        + 3]
                };
                assert_eq!(alpha(0, 0), 0);
                assert!(alpha(-28, -28) > 0);
                s.undo();
                assert!(s.objects[0].erased.is_empty());
            }
        }
    }
    #[test]
    fn rectangle_erase_interaction_cache_and_export_separation() {
        for scale in [1., 1.25] {
            let mut s = document();
            let view = views(scale)[0].0;
            let mut compiler = crate::annotation_scene::SceneCompiler::default();
            let mut press = Press::begin(
                &mut s,
                (-50., -40.),
                false,
                12. * scale,
                scale,
                &views(scale),
            );
            press.update(&mut s, (50., 40.), &views(scale));
            let drawing = compiler.render(&s, view, scale, None, None, None).unwrap();
            let signature =
                crate::annotation_signature::Frame::new(&s, None).display(view, scale, None);
            s.finish_drag();
            assert_ne!(
                signature,
                crate::annotation_signature::Frame::new(&s, None).display(view, scale, None)
            );
            let editing = compiler.render(&s, view, scale, None, None, None).unwrap();
            let expected = crate::annotation_render::render(&s, view, scale, None, None).unwrap();
            assert_ne!(drawing.data(), editing.data());
            let content_signature = crate::annotation_signature::Frame::new(&s, None)
                .document_signature(view, scale, None);
            if let Some(folder) = std::env::var_os("LEXIFT_RECTANGLE_ERASE_SCREENSHOTS") {
                let folder = std::path::PathBuf::from(folder);
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(
                    folder.join(format!("expected-{scale}.rgba")),
                    expected.data(),
                )
                .unwrap();
                std::fs::write(folder.join(format!("drawing-{scale}.rgba")), drawing.data())
                    .unwrap();
                std::fs::write(folder.join(format!("editing-{scale}.rgba")), editing.data())
                    .unwrap();
            }
            // Separate transparent interaction composition adds up to two 8-bit rounding steps.
            assert!(
                editing
                    .data()
                    .iter()
                    .zip(expected.data())
                    .all(|(a, b)| a.abs_diff(*b) <= 2),
                "interaction raster mismatch"
            );
            s.finish_rectangle_erase();
            assert_eq!(
                content_signature,
                crate::annotation_signature::Frame::new(&s, None)
                    .document_signature(view, scale, None)
            );
            let finalized = compiler.render(&s, view, scale, None, None, None).unwrap();
            assert_ne!(editing.data(), finalized.data());
            assert_eq!(
                finalized.data(),
                crate::annotation_render::render(&s, view, scale, None, None)
                    .unwrap()
                    .data()
            );
        }
    }
}
