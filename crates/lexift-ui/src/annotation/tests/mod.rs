/// Isolated CPU measurements; excludes capture, GPU, native submit and display.
#[test]
#[ignore = "run explicitly in Release without concurrent builds for CPU profiling"]
fn long_trajectory_signature_and_cache_costs() {
    use std::hint::black_box;
    fn p95(mut samples: Vec<Duration>) -> f64 {
        samples.sort_unstable();
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1000.
    }
    let viewport = Rect {
        left: -128,
        top: -128,
        right: 128,
        bottom: 128,
    };
    for points in [1_000, 20_000, 100_000] {
        let mut session = Session::new();
        assert!(
            session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Pencil,
                bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                points: (0..points)
                    .map(|i| (
                        -40. + 80. * i as f32 / points as f32,
                        -30. + 60. * i as f32 / points as f32
                    ))
                    .collect(),
                style: Style::default(),
                extra: Extra::None,
            })
        );
        let reference = session.objects[0].clone();
        let mut compiler = crate::annotation_scene::SceneCompiler::default();
        black_box(
            compiler
                .document(&session, viewport, 1., None, None)
                .unwrap(),
        );
        let mut signature = Vec::new();
        let mut compare = Vec::new();
        let mut copy = Vec::new();
        let mut cached_scene = Vec::new();
        for _ in 0..240 {
            let start = Instant::now();
            black_box(render_signature(
                black_box(&session),
                viewport,
                1.,
                None,
                None,
            ));
            signature.push(start.elapsed());
            let start = Instant::now();
            black_box(black_box(&session.objects[0]) == black_box(&reference));
            compare.push(start.elapsed());
            let start = Instant::now();
            drop(black_box(black_box(&session.objects[0]).clone()));
            copy.push(start.elapsed());
            let start = Instant::now();
            black_box(
                compiler
                    .document(black_box(&session), viewport, 1., None, None)
                    .unwrap(),
            );
            cached_scene.push(start.elapsed());
        }
        eprintln!(
            "trajectory_cpu_scope=one_visible_unchanged_pencil_one_viewport points={points} samples=240 signature_p95_ms={:.6} equality_p95_ms={:.6} clone_drop_p95_ms={:.6} cached_scene_p95_ms={:.6} point_payload_bytes={}",
            p95(signature),
            p95(compare),
            p95(copy),
            p95(cached_scene),
            points * std::mem::size_of::<(f32, f32)>()
        );
    }
}

/// Isolates native scene preparation and shared software inputs; no capture or display work.
#[test]
#[ignore = "run explicitly in Release without concurrent builds for CPU profiling"]
fn long_mosaic_native_preparation_costs() {
    use lexift_core::{
        domain::annotation::{MosaicAnnotation, MosaicEffect},
        ports::annotation_render::{RenderStamp, RenderSurface},
    };
    use std::hint::black_box;
    fn p95(mut samples: Vec<Duration>) -> f64 {
        samples.sort_unstable();
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1000.
    }
    let surfaces = [
        RenderSurface {
            input_window: lexift_core::ports::magnifier::WindowToken(1),
            bounds: Rect {
                left: -512,
                top: -128,
                right: 0,
                bottom: 128,
            },
            scale: 1.,
        },
        RenderSurface {
            input_window: lexift_core::ports::magnifier::WindowToken(2),
            bounds: Rect {
                left: 0,
                top: -128,
                right: 512,
                bottom: 128,
            },
            scale: 1.25,
        },
    ];
    for points in [1_000, 20_000, 100_000] {
        let mut session = Session::new();
        session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-400., -20.), (400., 20.)),
            points: (0..points)
                .map(|i| {
                    let x = -400. + 800. * i as f32 / points as f32;
                    (x, (x / 80.).sin() * 20.)
                })
                .collect(),
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 100, 12., 1.25)),
        });
        session.selected = None;
        let mut compiler = crate::annotation_scene::SceneCompiler::default();
        let compile = |compiler: &mut crate::annotation_scene::SceneCompiler, revision| {
            compiler
                .compile(crate::annotation_scene::SceneInput {
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
        black_box(compile(&mut compiler, 0));
        let mut snapshots = crate::annotation_render::mosaic_worker::Snapshot::default();
        black_box(snapshots.get(&session, None));
        let mut native = Vec::new();
        let mut signatures = Vec::new();
        let mut snapshot = Vec::new();
        for revision in 1..=240 {
            let start = Instant::now();
            black_box(compile(&mut compiler, revision));
            native.push(start.elapsed());
            let start = Instant::now();
            let frame = crate::annotation_signature::Frame::new(&session, None);
            for surface in &surfaces {
                black_box(frame.document_signature(surface.bounds, surface.scale, None));
                black_box(frame.display(surface.bounds, surface.scale, None));
            }
            assert_eq!(frame.computed_objects(), 1);
            signatures.push(start.elapsed());
            let start = Instant::now();
            black_box(snapshots.get(&session, None));
            snapshot.push(start.elapsed());
        }
        let (buffers, bytes) = compiler.retained_trajectory_payload();
        eprintln!(
            "trajectory_cpu_scope=unchanged_mosaic_brush_two_native_surfaces points={points} samples=240 native_prepare_p95_ms={:.6} four_signatures_p95_ms={:.6} snapshot_reuse_p95_ms={:.6} retained_point_buffers={buffers} retained_point_capacity_bytes={bytes}",
            p95(native),
            p95(signatures),
            p95(snapshot)
        );
    }
}

fn exercise_menu_owner_teardown(close_owner: bool, canvas_target: bool) {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    init(
        |_| PassiveWindowPreparation::Pending,
        WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| false,
            |_, _, _| false,
            |_, _| true,
            |_, _| true,
            || false,
        ),
    );
    let native_sink = Rc::new(RefCell::new(None::<crate::bridge::PopupPointerSink>));
    let cleanup_events = Rc::new(Cell::new(0));
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        let sink = native_sink.clone();
        let events = cleanup_events.clone();
        r.lifecycle.set_popup_dismissal = Rc::new(move |_, enabled| {
            if !enabled && let Some(sink) = sink.borrow().as_ref() {
                events.set(events.get() + 1);
                // Removing the native child hook can synchronously deliver input
                // to its owner while Registry is still mutably borrowed.
                sink(PopupPointerInput::LeftPressed { x: 10., y: 10. });
            }
            true
        });
    });
    for watermark_owner in [false, true] {
        let generation = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().generation);
        if watermark_owner {
            open_watermark_input(generation);
        } else {
            open_panel(0, false);
        }
        let (revision, old_input, sink) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            if watermark_owner {
                let revision = r.toolbar_state.watermark.revision();
                let sink = watermark::pointer_sink(
                    r.toolbar_state.watermark.get().unwrap().as_weak(),
                    move || {
                        REGISTRY.with(|slot| {
                            slot.borrow().as_ref().is_some_and(|r| {
                                r.generation == generation
                                    && r.toolbar_state.watermark.revision() == revision
                            })
                        })
                    },
                    r.toolbar_state.watermark.input(),
                    choice::watermark_pointer,
                );
                (revision, r.toolbar_state.watermark.input(), sink)
            } else {
                (
                    r.toolbar_state.panel.revision(),
                    r.toolbar_state.panel.input(),
                    pointer_panel(
                        r.toolbar_state.panel.get().unwrap().as_weak(),
                        generation,
                        r.toolbar_state.panel.revision(),
                        r.toolbar_state.panel.input(),
                    ),
                )
            }
        });
        let (target_input, sink) = if canvas_target {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                if r.canvas_state.canvases.is_empty() {
                    r.canvas_state.canvases.push(CanvasLayer::new(
                        AnnotationCanvas::new().unwrap(),
                        Rect {
                            left: 0,
                            top: 0,
                            right: 200,
                            bottom: 120,
                        },
                    ));
                }
                let layer = &r.canvas_state.canvases[0];
                (
                    layer.input(),
                    pointer_canvas(
                        layer.window.as_weak(),
                        generation,
                        0,
                        layer.revision(),
                        layer.input(),
                    ),
                )
            })
        } else {
            (old_input.clone(), sink)
        };
        let delivered = Rc::new(Cell::new(0));
        let observed = delivered.clone();
        let sink = target_input.pointer_sink(move |event| {
            observed.set(observed.get() + 1);
            sink(event);
        });
        *native_sink.borrow_mut() = Some(sink.clone());
        let queued = Rc::new(Cell::new(0));
        let queued_action = queued.clone();
        old_input.post(move || queued_action.set(queued_action.get() + 1));
        if watermark_owner {
            choice::open_watermark(generation, revision, 0., 0., 180., 34.);
        } else {
            choice::open(
                generation,
                revision,
                crate::AnnotationChoiceRequest {
                    field: 1,
                    options: ModelRc::new(VecModel::from(vec![
                        "Rectangle".into(),
                        "Ellipse".into(),
                    ])),
                    width: 180.,
                    height: 34.,
                    ..Default::default()
                },
            );
        }
        let before = cleanup_events.get();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.toolbar_state.menus.current.is_some());
            if !close_owner {
                choice::close(r, choice::CloseReason::InteractionFinished);
            } else if watermark_owner {
                r.close_watermark_input();
            } else {
                r.close_panel();
            }
            assert!(r.toolbar_state.menus.current.is_none());
        });
        assert!(cleanup_events.get() > before);
        assert_eq!(delivered.get(), 0, "cleanup input must not reach the owner");
        assert_eq!(old_input.retired(), close_owner);
        *native_sink.borrow_mut() = None;
        if close_owner {
            if watermark_owner {
                open_watermark_input(generation);
            } else {
                open_panel(0, false);
            }
        }
        sink(PopupPointerInput::LeftPressed { x: 10., y: 10. });
        slint::platform::update_timers_and_animations();
        assert_eq!(queued.get(), usize::from(!close_owner));
        assert_eq!(delivered.get(), usize::from(!close_owner));
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let (new_revision, new_input) = if watermark_owner {
                (
                    r.toolbar_state.watermark.revision(),
                    r.toolbar_state.watermark.input(),
                )
            } else {
                (
                    r.toolbar_state.panel.revision(),
                    r.toolbar_state.panel.input(),
                )
            };
            assert_eq!(new_revision == revision, !close_owner);
            assert!(!new_input.retired());
            assert!(r.toolbar_state.menus.current.is_none());
        });
    }
    shutdown();
}

use super::*;

mod rendering_and_windows;

mod text_and_focus;

mod parameters_and_panels;

mod interaction;
