//! Complete-input invalidation, lifetime and CPU evidence for scene assembly reuse.
use super::tests::object;
use super::*;
use lexift_core::domain::annotation::{TextAnnotation, WatermarkPosition};

fn input<'a>(session: &'a Session, surfaces: &'a [RenderSurface], revision: u64) -> SceneInput<'a> {
    SceneInput {
        session,
        surfaces,
        stamp: RenderStamp {
            session: 7,
            revision,
        },
        edit: None,
        draft: None,
        hovered: None,
        reference: None,
        editor: None,
        paused_regions: &[],
        excluded_windows: &[],
    }
}

fn surfaces() -> [RenderSurface; 2] {
    [
        RenderSurface {
            input_window: WindowToken(1),
            bounds: Rect {
                left: -640,
                top: -180,
                right: 0,
                bottom: 180,
            },
            scale: 1.,
        },
        RenderSurface {
            input_window: WindowToken(2),
            bounds: Rect {
                left: 0,
                top: -180,
                right: 640,
                bottom: 180,
            },
            scale: 1.25,
        },
    ]
}

#[test]
fn effect_metadata_changes_bypass_reuse_without_discarding_unchanged_masks() {
    use lexift_core::domain::annotation::{MosaicAnnotation, MosaicEffect};
    let mut session = Session::new();
    let mut mosaic = object(Kind::MosaicArea);
    mosaic.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 50, 12., 1.25));
    session.add(mosaic);
    let surfaces = surfaces();
    let mut compiler = SceneCompiler::default();
    let first = compiler.compile(input(&session, &surfaces, 1)).unwrap();
    let reused = compiler.compile(input(&session, &surfaces, 2)).unwrap();
    assert!(Arc::ptr_eq(&first.layers, &reused.layers));
    let Extra::Mosaic(settings) = &mut session.objects[0].extra else {
        unreachable!()
    };
    settings.effect = MosaicEffect::Pixelate;
    settings.strength = 90;
    let updated = compiler.compile(input(&session, &surfaces, 3)).unwrap();
    assert!(!Arc::ptr_eq(&first.layers, &updated.layers));
    let (RenderLayer::Mosaic(a), RenderLayer::Mosaic(b)) = (&first.layers[0], &updated.layers[0])
    else {
        panic!("mosaic metadata")
    };
    assert_eq!(b.object.as_ref(), &session.objects[0]);
    assert!(!Arc::ptr_eq(&a.object, &b.object));
    assert!(
        a.masks
            .iter()
            .zip(b.masks.iter())
            .all(|(a, b)| a.key == b.key && Arc::ptr_eq(&a.pixels, &b.pixels))
    );
    let repeated = compiler.compile(input(&session, &surfaces, 4)).unwrap();
    assert!(Arc::ptr_eq(&updated.layers, &repeated.layers));
}

#[test]
#[ignore = "Release CPU comparison of unchanged scene reassembly versus exact-input reuse; not GPU or App latency"]
fn unchanged_assembled_scene_cpu_costs() {
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        panic!("run CPU comparison with --release");
    }
    let p95 = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[(values.len() * 95).div_ceil(100) - 1]
    };
    let surfaces = surfaces();
    for count in [1, 100, 1000] {
        let mut session = Session::new();
        // Fixture construction has no undo snapshots or quadratic document copies.
        session.objects = (0..count)
            .map(|index| {
                let mut mark = object(Kind::Rectangle);
                let left = -90. + (index % 20) as f32;
                mark.bounds = Bounds::from_corners((left, -80.), (30., 25.));
                mark
            })
            .collect();
        session.selected = None;
        let mut baseline = SceneCompiler::default();
        let mut reused = SceneCompiler::default();
        black_box(baseline.assemble(&input(&session, &surfaces, 0)).unwrap());
        let initial = reused.compile(input(&session, &surfaces, 0)).unwrap();
        let mut old_times = Vec::new();
        let mut new_times = Vec::new();
        for revision in 1..=240 {
            let mut scenes = [None, None];
            for mode in [revision % 2, 1 - revision % 2] {
                let start = Instant::now();
                let scene = if mode == 0 {
                    baseline
                        .assemble(&input(&session, &surfaces, revision))
                        .unwrap()
                } else {
                    reused
                        .compile(input(&session, &surfaces, revision))
                        .unwrap()
                };
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                if mode == 0 {
                    old_times.push(elapsed);
                } else {
                    new_times.push(elapsed);
                }
                scenes[mode as usize] = Some(scene);
            }
            let a = scenes[0].as_ref().unwrap();
            let b = scenes[1].as_ref().unwrap();
            assert!(
                same_content(a, b),
                "same seeded asset keys and complete scene metadata"
            );
            assert!(Arc::ptr_eq(&initial.layers, &b.layers));
        }
        println!(
            "scene_reuse_scope=unchanged_rectangle_document_two_surfaces_1x_1.25x_exact_values_no_ui_input_gpu_or_photon objects={count} samples=240 reassembly_p95_ms={:.6} reuse_p95_ms={:.6}",
            p95(old_times),
            p95(new_times)
        );
    }
}

#[test]
fn assembled_scene_reuse_preserves_pixels_and_invalidates_all_input_boundaries() {
    let mut original = Session::new();
    let mut text = object(Kind::Text);
    text.extra = Extra::Text(TextAnnotation {
        content: "中文abc".into(),
        font_family: "Arial".into(),
        font_size: 22.,
        bold: false,
        italic: false,
        outline: None,
        background: None,
        rotation: 0.,
        layout_scale: 1.,
        linked_arrow: None,
        linked_arrow_control: None,
    });
    original.add(text);
    original.add(object(Kind::Rectangle));
    original.selected = Some(0);
    original.watermark = Some(Watermark {
        template: "水印".into(),
        content: "水印".into(),
        font_family: "Arial".into(),
        font_size: 12.,
        opacity: 50,
        color: [10, 20, 30],
        position: WatermarkPosition::Center,
    });
    let original_surfaces = [
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
    original.displays = original_surfaces.iter().map(|s| s.bounds).collect();
    let mut original_draft = object(Kind::Pencil);
    original_draft.points = vec![(-80., 10.), (20., 20.)];
    let original_editor = EditorOverlay {
        object: original.objects[0].clone(),
        offset: 3,
        anchor: 0,
        preedit: 0..0,
        visible: true,
        reference: None,
    };
    for change in 0..28 {
        let mut session = Session::new();
        session.objects = original.objects.clone();
        session.selected = original.selected;
        session.watermark = original.watermark.clone();
        session.displays = original.displays.clone();
        session.spotlight_opacity = original.spotlight_opacity;
        let mut surfaces = original_surfaces;
        let mut draft = Some(original_draft.clone());
        let mut editor = Some(original_editor.clone());
        let mut reference = None;
        let mut hovered = None;
        let mut paused = Vec::new();
        let mut excluded = Vec::new();
        let mut edited = None;
        let mut generation = 7;
        let mut compiler = SceneCompiler::default();
        // Keep each borrow local to a compile call while varying every
        // boundary independently; this constructor is only a test fixture.
        #[allow(clippy::too_many_arguments)]
        fn input<'a>(
            session: &'a Session,
            surfaces: &'a [RenderSurface],
            draft: Option<&'a Object>,
            editor: Option<&'a EditorOverlay>,
            reference: Option<&'a str>,
            hovered: Option<usize>,
            paused: &'a [Rect],
            excluded: &'a [WindowToken],
            edit: Option<(Option<usize>, &'a Object)>,
            generation: u64,
            revision: u64,
        ) -> SceneInput<'a> {
            SceneInput {
                session,
                surfaces,
                draft,
                editor,
                reference,
                hovered,
                paused_regions: paused,
                excluded_windows: excluded,
                edit,
                stamp: RenderStamp {
                    session: generation,
                    revision,
                },
            }
        }
        let first = compiler
            .compile(input(
                &session,
                &surfaces,
                draft.as_ref(),
                editor.as_ref(),
                reference,
                hovered,
                &paused,
                &excluded,
                None,
                generation,
                1,
            ))
            .unwrap();
        assert!(compiler.compiled.is_some());
        let second = compiler
            .compile(input(
                &session,
                &surfaces,
                draft.as_ref(),
                editor.as_ref(),
                reference,
                hovered,
                &paused,
                &excluded,
                None,
                generation,
                2,
            ))
            .unwrap();
        assert_eq!(second.stamp.revision, 2);
        assert!(Arc::ptr_eq(&first.layers, &second.layers));
        assert!(Arc::ptr_eq(&first.interaction, &second.interaction));
        match change {
            0 => session.objects[0].style.color = [40, 50, 60],
            1 => session.objects[0].bounds.left += 4.,
            2 => session.objects[1].kind = Kind::Ellipse,
            3 => draft.as_mut().unwrap().points[1].1 += 10.,
            4 => session.objects.swap(0, 1),
            5 => {
                session.objects.pop();
            }
            6 => surfaces[0].input_window = WindowToken(8),
            7 => surfaces[0].scale = 1.25,
            8 => surfaces[0].bounds.left -= 4,
            9 => surfaces.swap(0, 1),
            10 => generation += 1,
            11 => session.selected = None,
            12 => hovered = Some(1),
            13 => reference = Some("确认"),
            14 => editor.as_mut().unwrap().offset = 6,
            15 => editor.as_mut().unwrap().anchor = 3,
            16 => editor.as_mut().unwrap().preedit = 0..3,
            17 => editor.as_mut().unwrap().visible = false,
            18 => editor.as_mut().unwrap().reference = Some("参考".into()),
            19 => editor.as_mut().unwrap().object.style.color = [70, 80, 90],
            20 => session.watermark.as_mut().unwrap().color = [10, 30, 50],
            21 => session.displays.pop().map(|_| ()).unwrap(),
            22 => session.spotlight_opacity *= 0.5,
            23 => paused.push(surfaces[0].bounds),
            24 => excluded.push(WindowToken(5)),
            25 => {
                let mut replacement = session.objects[0].clone();
                replacement.style.color = [90, 80, 70];
                edited = Some(replacement);
            }
            26 => draft = None,
            27 => editor = None,
            _ => unreachable!(),
        }
        let edit = edited.as_ref().map(|object| (Some(0), object));
        let updated = compiler
            .compile(input(
                &session,
                &surfaces,
                draft.as_ref(),
                editor.as_ref(),
                reference,
                hovered,
                &paused,
                &excluded,
                edit,
                generation,
                3,
            ))
            .unwrap();
        assert!(
            !Arc::ptr_eq(&second.layers, &updated.layers),
            "input change {change} reused stale scene"
        );
        let fresh = SceneCompiler::default()
            .compile(input(
                &session,
                &surfaces,
                draft.as_ref(),
                editor.as_ref(),
                reference,
                hovered,
                &paused,
                &excluded,
                edit,
                generation,
                3,
            ))
            .unwrap();
        for surface in surfaces {
            assert_eq!(
                compose_document(&updated, surface.bounds).unwrap().data(),
                compose_document(&fresh, surface.bounds).unwrap().data(),
                "input change {change} differs from fresh compilation"
            );
            assert_eq!(
                compose(surface.bounds, updated.interaction.iter())
                    .unwrap()
                    .data(),
                compose(surface.bounds, fresh.interaction.iter())
                    .unwrap()
                    .data(),
                "feedback change {change} differs from fresh compilation"
            );
        }
    }
}

#[test]
fn assembled_scene_does_not_copy_trajectories_and_releases_on_fallback_or_failure() {
    let mut session = Session::new();
    let mut stroke = object(Kind::Pencil);
    stroke.points = (0..2000).map(|i| (-20. + (i % 40) as f32, 0.)).collect();
    session.add(stroke);
    let surfaces = [RenderSurface {
        input_window: WindowToken(1),
        bounds: Rect {
            left: -128,
            top: -128,
            right: 128,
            bottom: 128,
        },
        scale: 1.,
    }];
    let input = || SceneInput {
        session: &session,
        surfaces: &surfaces,
        draft: None,
        editor: None,
        reference: None,
        hovered: None,
        paused_regions: &[],
        excluded_windows: &[],
        edit: None,
        stamp: RenderStamp {
            session: 1,
            revision: 1,
        },
    };
    let mut compiler = SceneCompiler::default();
    compiler.compile(input()).unwrap();
    assert_eq!(compiler.retained_trajectory_payload().0, 1);
    let snapshot = &compiler.objects.values().next().unwrap().object;
    assert_eq!(
        Arc::strong_count(snapshot),
        2,
        "assembled state shares existing snapshot"
    );
    let weak = Arc::downgrade(snapshot);
    compiler.retain_viewports(&[]);
    assert!(compiler.compiled.is_none());
    assert!(weak.upgrade().is_none());
    compiler.compile(input()).unwrap();
    let pixels = Arc::downgrade(&compiler.objects.values().next().unwrap().raster.pixels);
    let mut invalid_surfaces = surfaces;
    invalid_surfaces[0].scale = f32::NAN;
    let mut invalid = input();
    invalid.surfaces = &invalid_surfaces;
    assert!(compiler.compile(invalid).is_none());
    assert!(compiler.compiled.is_none());
    compiler.clear();
    assert!(pixels.upgrade().is_none());
    session.objects[0].bounds = Bounds::from_corners((1000., 1000.), (1020., 1020.));
    session.objects[0].points = vec![(1000., 1000.), (1020., 1020.)];
    let scene = compiler
        .compile(SceneInput {
            session: &session,
            surfaces: &surfaces,
            draft: None,
            editor: None,
            reference: None,
            hovered: None,
            paused_regions: &[],
            excluded_windows: &[],
            edit: None,
            stamp: RenderStamp {
                session: 1,
                revision: 2,
            },
        })
        .unwrap();
    assert!(scene.layers.is_empty());
    assert!(
        compiler.compiled.is_none(),
        "uncached offscreen objects conservatively bypass reuse"
    );
}

#[test]
fn magnifier_component_coverage_invalidates_masks_and_decorations_without_reusing_old_holes() {
    use lexift_core::domain::annotation::MagnifierConnector;
    use lexift_core::domain::annotation_erase::{ErasePart, EraseRegion, EraseTarget};
    let surfaces = surfaces();
    let mut session = Session::new();
    let mut lens = object(Kind::Magnifier);
    lens.bounds = Bounds::from_corners((-100., -80.), (-60., -40.));
    lens.extra = Extra::Magnifier {
        output: Bounds::from_corners((20., -80.), (100., 0.)),
        zoom: 2.,
        ellipse: false,
        connector: MagnifierConnector::Frame,
        erase_annotations: true,
        antialias: true,
        shadow: true,
    };
    session.add(lens);
    let id = session.objects[0].id;
    session.erase(
        EraseRegion::Rectangle(Bounds::from_corners((85., -50.), (105., -30.))),
        &[EraseTarget::Object(id)],
    );
    session.selected = None;
    let original = session.objects[0].clone();
    let mut compiler = SceneCompiler::default();
    let mut previous = None;
    for (i, dx) in [0., 10., 20., 30., 20., 10., 0.].into_iter().enumerate() {
        let mut lens = original.clone();
        lens.move_from((50., -20.), dx, 0.);
        session.objects[0] = lens;
        let scene = compiler
            .compile(input(&session, &surfaces, i as u64))
            .unwrap();
        let lens = scene
            .layers
            .iter()
            .find_map(|l| match l {
                RenderLayer::Magnifier(l) => Some(l),
                _ => None,
            })
            .unwrap();
        let mask = &lens.masks[0];
        let at = |x: f32, y: f32| {
            let x = x as i32 - mask.bounds.left;
            let y = y as i32 - mask.bounds.top;
            mask.pixels[((y * (mask.bounds.right - mask.bounds.left) + x) * 4 + 3) as usize]
        };
        assert_eq!(at(90. + dx, -40.), 0, "compiled GPU mask lost its hole");
        assert_eq!(at(50. + dx, -40.), 255);
        let repeated = compiler
            .compile(input(&session, &surfaces, 100 + i as u64))
            .unwrap();
        assert!(Arc::ptr_eq(&scene.layers, &repeated.layers));
        if let Some((key, old)) = previous {
            assert_ne!(key, mask.key);
            assert!(!Arc::ptr_eq(&old, &scene.layers));
        }
        previous = Some((mask.key, scene.layers.clone()));
    }
    // Component association itself must be in the signature, not just region/affine data.
    let before = compiler.compile(input(&session, &surfaces, 300)).unwrap();
    session.objects[0]
        .erased
        .retain(|c| c.part != ErasePart::MagnifierOutput);
    let after = compiler.compile(input(&session, &surfaces, 301)).unwrap();
    assert!(!Arc::ptr_eq(&before.layers, &after.layers));
    let lens = after
        .layers
        .iter()
        .find_map(|l| match l {
            RenderLayer::Magnifier(l) => Some(l),
            _ => None,
        })
        .unwrap();
    let mask = &lens.masks[0];
    let offset = (((-40 - mask.bounds.top) * (mask.bounds.right - mask.bounds.left) + 90
        - mask.bounds.left)
        * 4
        + 3) as usize;
    assert_eq!(mask.pixels[offset], 255);
}

#[test]
fn blank_erase_geometry_and_edit_phase_invalidate_assembled_scene() {
    use lexift_core::domain::annotation_erase::RoundedRectangle;
    let mut session = Session::new();
    let surfaces = surfaces();
    let mut compiler = SceneCompiler::default();
    let mut geometry = RoundedRectangle {
        bounds: Bounds::from_corners((-120., -80.), (-20., 80.)),
        rounding: 21.,
        creation_scale: 1.,
    };
    let empty = compiler.compile(input(&session, &surfaces, 1)).unwrap();
    session.begin_rectangle_erase(geometry);
    let drawing = compiler.compile(input(&session, &surfaces, 2)).unwrap();
    assert!(!same_content(&empty, &drawing));
    session.finish_drag();
    let editing = compiler.compile(input(&session, &surfaces, 3)).unwrap();
    assert!(!same_content(&drawing, &editing));
    let repeat = compiler.compile(input(&session, &surfaces, 4)).unwrap();
    assert!(same_content(&editing, &repeat));
    assert!(Arc::ptr_eq(&editing.interaction, &repeat.interaction));
    geometry.bounds.left -= 20.;
    geometry.bounds.right -= 20.;
    session.begin_drag();
    session.prepare_rectangle_erase(geometry).unwrap();
    let moved = compiler.compile(input(&session, &surfaces, 5)).unwrap();
    assert!(!same_content(&editing, &moved));
    geometry.rounding = 0.;
    session.prepare_rectangle_erase(geometry).unwrap();
    let square = compiler.compile(input(&session, &surfaces, 6)).unwrap();
    assert!(!same_content(&moved, &square));
    session.cancel_drag();
    let cancelled = compiler.compile(input(&session, &surfaces, 7)).unwrap();
    assert!(!same_content(&square, &cancelled));
    session.finish_rectangle_erase();
    let finished = compiler.compile(input(&session, &surfaces, 8)).unwrap();
    assert!(finished.interaction.is_empty());
    assert!(same_content(&empty, &finished));
    assert!(!session.can_undo());
}
