use super::*;
#[test]
fn highlighter_rectangle_mode_survives_deselection_drawing_and_cancellation() {
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
        |_| PassiveWindowPreparation::Ready,
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
    for dpi in [1., 1.25] {
        for explicit in [true, false] {
            let baseline = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close();
                r.canvas_state.pending_displays = vec![Rect {
                    left: -500,
                    top: -300,
                    right: 500,
                    bottom: 500,
                }];
                create_canvas(r, 0);
                r.canvas_state.canvases[0].window.window().dispatch_event(
                    slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                );
                r.canvas_state.annotation_ready = true;
                assert!(r.set_mode(InteractionMode::Tool(1)));
                let defaults = r.parameter_values.defaults(4).clone();
                let kind = if explicit {
                    Kind::HighlightLine
                } else {
                    Kind::HighlightRectangle
                };
                r.session.add(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind,
                    bounds: Bounds::from_corners((-400., -250.), (-350., -200.)),
                    points: if explicit {
                        vec![(-400., -250.), (-350., -200.)]
                    } else {
                        Vec::new()
                    },
                    style: Style {
                        color: [1, 2, 3],
                        width: 37.,
                        ..Style::default()
                    },
                    extra: Extra::None,
                });
                project_object_parameters(r, 0);
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(4);
                panel.set_values(r.values(4).clone());
                r.toolbar_state.panel.open(panel);
                if explicit {
                    let mut values = r.values(4).clone();
                    values.shape = 1;
                    let target = r.parameter_target(4);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    r.finish_style_edit();
                }
                assert_eq!(r.values(4).shape, 1);
                let baseline = r.session.objects[0].clone();
                assert_eq!(
                    r.parameter_values.defaults(4).size,
                    if explicit { 37 } else { defaults.size }
                );
                assert_eq!(
                    r.parameter_values.defaults(4).color_index,
                    if explicit { 8 } else { defaults.color_index }
                );
                baseline
            });
            for n in 0..2 {
                let x = 300. + n as f32 * 180.;
                canvas_pointer(0, 0, x / dpi, 250. / dpi);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.values(4).shape, 1);
                    assert_eq!(r.toolbar_state.panel.get().unwrap().get_values().shape, 1);
                    assert!(matches!(
                        r.gesture(),
                        Some(Gesture::Draw {
                            kind: Kind::HighlightRectangle,
                            ..
                        })
                    ));
                });
                canvas_pointer(0, 1, (x + 120.) / dpi, 350. / dpi);
                canvas_pointer(0, 2, (x + 120.) / dpi, 350. / dpi);
            }
            canvas_pointer(0, 0, 750. / dpi, 500. / dpi);
            canvas_pointer(0, 1, 850. / dpi, 600. / dpi);
            canvas_pointer(0, 3, 850. / dpi, 600. / dpi);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.session.objects.len(), 3);
                assert_eq!(r.session.objects[0], baseline);
                assert!(
                    r.session.objects[1..]
                        .iter()
                        .all(|o| o.kind == Kind::HighlightRectangle)
                );
                assert_eq!(r.parameter_values.defaults(4).shape, 1);
                assert!(r.session.undo());
                assert_eq!(r.session.objects.len(), 2);
                assert!(r.session.undo());
                assert_eq!(r.session.objects, vec![baseline]);
                restore_parameter_projection(r);
                assert!(r.set_mode(InteractionMode::Tool(2)));
                assert!(r.set_mode(InteractionMode::Tool(1)));
                assert_eq!(active_kind(r), Some(Kind::HighlightRectangle));
                let mut values = r.values(4).clone();
                values.shape = 0;
                let target = r.parameter_target(4);
                assert!(apply_geometry_values(r, target, values).is_some());
                r.finish_style_edit();
            });
            canvas_pointer(0, 0, 750. / dpi, 150. / dpi);
            canvas_pointer(0, 1, 850. / dpi, 250. / dpi);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(matches!(
                    r.gesture(),
                    Some(Gesture::Draw {
                        kind: Kind::HighlightLine
                    })
                ));
                assert_eq!(r.values(4).shape, 0);
            });
            canvas_pointer(0, 3, 850. / dpi, 250. / dpi);
        }
    }
    shutdown();
}
#[test]
fn replaced_canvas_at_same_index_rejects_old_input_and_keyboard_callbacks() {
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
        |_| PassiveWindowPreparation::Ready,
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
    let (old, sink, current) = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.canvas_state.pending_displays = vec![Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        }];
        create_canvas(r, 0);
        let old = r.canvas_state.canvases[0].window.clone_strong();
        let old_revision = r.canvas_state.canvases[0].revision();
        let input = r.canvas_state.canvases[0].input();
        old.invoke_escape_requested();
        assert!(input.pending());
        let sink = pointer_canvas(old.as_weak(), r.generation, 0, old_revision, input.clone());
        r.canvas_state.canvases.clear();
        sink(PopupPointerInput::LeftCancelled { x: 0., y: 0. });
        assert!(input.retired() && !input.pending());
        old.invoke_undo_requested();
        assert!(!input.pending());
        create_canvas(r, 0);
        assert_ne!(r.canvas_state.canvases[0].revision(), old_revision);
        r.canvas_state.annotation_ready = true;
        (old, sink, r.canvas_state.canvases[0].window.clone_strong())
    });
    old.invoke_pointer(0, 20., 20.);
    old.invoke_pointer(2, 80., 80.);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.session.objects.is_empty());
        assert!(r.gesture().is_none());
    });
    current.invoke_pointer(0, 20., 20.);
    sink(PopupPointerInput::LeftCancelled { x: 30., y: 30. });
    old.invoke_undo_requested();
    old.invoke_escape_requested();
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.canvas_state.canvases.len(), 1);
        assert!(r.gesture().is_some());
    });
    current.invoke_pointer(1, 80., 80.);
    current.invoke_pointer(2, 80., 80.);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert_eq!(r.session.objects.len(), 1);
        assert!(r.session.undo());
        assert!(!r.session.can_undo());
    });
    shutdown();
}
#[test]
fn retired_toolbar_cannot_operate_or_reposition_a_reopened_session() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestClock(Rc<Cell<Duration>>);
    impl Platform for TestClock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
    slint::platform::update_timers_and_animations();
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
    let placement_queries = Rc::new(Cell::new(0));
    let queries = placement_queries.clone();
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .popup_work_area = Rc::new(move |_| {
            queries.set(queries.get() + 1);
            Some(Rect {
                left: -1000,
                top: -1000,
                right: 1000,
                bottom: 1000,
            })
        });
    });
    toggle(Instant::now());
    let (generation, revision, old) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (
            r.generation,
            r.toolbar_state.main.revision(),
            r.toolbar_state.main.get().unwrap().clone_strong(),
        )
    });
    let retired_input =
        REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().toolbar_state.main.input());
    let old_sink = pointer_main(old.as_weak(), generation, revision, retired_input.clone());
    show_main(generation, revision, 0);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        settle_main_position(
            slot.as_ref().unwrap(),
            generation,
            revision,
            Point::default(),
            2,
        );
    });
    old.invoke_tool_clicked(4, false);
    old.invoke_finish_requested();
    assert!(retired_input.pending());
    schedule_render(generation);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        r.render_state
            .frame_tasks
            .defer_lens(generation, 1, sync_first_magnifier);
        assert!(r.render_state.frame_tasks.render_pending());
        assert!(r.render_state.frame_tasks.lens_pending());
    });
    let teardown_sink = old_sink.clone();
    let teardown_events = Rc::new(Cell::new(0));
    let recorded = teardown_events.clone();
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .annotation_magnifier_sync = Rc::new(move |_, _| {
            recorded.set(recorded.get() + 1);
            teardown_sink(PopupPointerInput::Moved { x: 0., y: 0. });
            false
        });
    });
    close();
    assert_eq!(teardown_events.get(), 1);
    // Native teardown can deliver a late pointer while Registry is mutably borrowed.
    REGISTRY.with(|slot| {
        let _teardown = slot.borrow_mut();
        old_sink(PopupPointerInput::Moved { x: 0., y: 0. });
    });
    assert!(retired_input.retired() && !retired_input.pending());
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(!r.render_state.frame_tasks.render_pending());
        assert!(!r.render_state.frame_tasks.lens_pending());
    });
    toggle(Instant::now());
    let new_revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .main
            .revision()
    });
    assert_ne!(revision, new_revision);
    old.window()
        .set_position(slint::PhysicalPosition::new(-100, -200));
    let old_position = old.window().position();
    old.invoke_escape_requested();
    old.invoke_operate_requested();
    old.invoke_undo_requested();
    assert!(!retired_input.pending());
    old_sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
    clock.set(Duration::from_millis(1000));
    slint::platform::update_timers_and_animations();
    assert_eq!(old.window().position(), old_position);
    assert_eq!(
        placement_queries.get(),
        0,
        "retired placement must not query a work area"
    );
    let current = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.toolbar_state.main.revision(), new_revision);
        assert!(r.toolbar_state.panel.get().is_none());
        assert_eq!(r.mode, InteractionMode::Tool(0));
        assert!(!r.session.can_undo());
        r.toolbar_state.main.get().unwrap().clone_strong()
    });
    current.invoke_tool_clicked(4, false);
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .panel
                .get()
                .is_some()
        )
    });
    shutdown();
}
#[test]
fn document_commands_retire_object_gestures_without_extra_undo_records() {
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
        |_| PassiveWindowPreparation::Ready,
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
    for resize in [false, true] {
        for command in 0..4 {
            let original = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.session.add(Object {
                    erased: Vec::new(),
                    id: ObjectId::DRAFT,
                    kind: Kind::Rectangle,
                    bounds: Bounds::from_corners((10., 20.), (90., 80.)),
                    style: Style::default(),
                    points: Vec::new(),
                    extra: Extra::None,
                }));
                let original = r.session.objects[0].clone();
                let target = original.id;
                r.gesture = (if resize {
                    gestures::GeometryDrag::resizing(&r.session, target, 7)
                } else {
                    gestures::GeometryDrag::moving(&r.session, target, (40., 40.))
                })
                .map(Active::Geometry);
                r.session.begin_drag();
                let mut changed = original.clone();
                if resize {
                    changed.edit_handle(7, (150., 120.), 1.);
                } else {
                    changed.move_by(70., 30.);
                }
                assert!(gestures::write(&mut r.session, target, changed));
                original
            });
            match command {
                0 => undo(),
                1 => delete_selected(),
                2 => REGISTRY.with(|slot| {
                    assert!(
                        slot.borrow_mut()
                            .as_mut()
                            .unwrap()
                            .set_mode(InteractionMode::Tool(1))
                    );
                }),
                _ => escape(),
            }
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.gesture().is_none() && r.text_handle().is_none());
                assert!(r.geometry_drag().is_none());
                if command == 1 {
                    assert!(r.session.objects.is_empty());
                    assert!(r.session.undo());
                }
                assert_eq!(r.session.objects, vec![original]);
                assert!(r.session.undo());
                assert!(
                    r.session.objects.is_empty(),
                    "cancellation must not create an undo record"
                );
                r.close();
            });
        }
    }
    shutdown();
}
#[test]
fn bootstrap_creates_each_canvas_before_advancing_windows() {
    let states = [CanvasBootstrap::Prepare(0), CanvasBootstrap::Prepare(0)];
    assert_eq!(
        bootstrap_action(0, 2, std::iter::empty(), 0),
        BootstrapAction::Create(0)
    );
    assert_eq!(
        bootstrap_action(1, 2, states[..1].iter().copied(), 0),
        BootstrapAction::Create(1)
    );
    assert_eq!(
        bootstrap_action(2, 2, states.iter().copied(), 0),
        BootstrapAction::Advance(0)
    );
}
#[test]
fn bootstrap_skips_ready_screen_and_retries_another() {
    let states = [CanvasBootstrap::Ready, CanvasBootstrap::Prepare(2)];
    assert_eq!(
        bootstrap_action(2, 2, states.iter().copied(), 0),
        BootstrapAction::Advance(1)
    );
    let states = [CanvasBootstrap::Present, CanvasBootstrap::Ready];
    assert_eq!(
        bootstrap_action(2, 2, states.iter().copied(), 1),
        BootstrapAction::Advance(0)
    );
    assert_eq!(
        bootstrap_action(2, 2, [CanvasBootstrap::Ready; 2].into_iter(), 0),
        BootstrapAction::ShowToolbar
    );
}
#[test]
fn arrow_click_preview_commits_once_and_rejects_a_nearby_endpoint() {
    let pending = ArrowPending::new((-120., 40.), Style::default(), 1.25, defaults(5));
    let mut session = Session::new();
    let preview = pending.preview((80., -20.));
    assert_eq!(preview.points, vec![(-120., 40.), (80., -20.)]);
    assert!(matches!(preview.extra, Extra::Arrow { curved: false, .. }));
    let frame = crate::annotation_render::render(
        &session,
        Rect {
            left: -250,
            top: -100,
            right: 150,
            bottom: 100,
        },
        1.,
        Some(&preview),
        None,
    )
    .unwrap();
    assert!(frame.pixel(230, 110).unwrap().alpha() > 0);
    assert!(session.objects.is_empty());
    assert!(!session.can_undo());
    assert!(pending.finish((-116.35, 40.)).is_none());
    assert!(pending.finish((-116.25, 40.)).is_some());
    assert!(session.add(pending.finish((80., -20.)).unwrap()));
    assert_eq!(session.selected, Some(0));
    assert_eq!(session.objects[0].points, preview.points);
    assert!(session.undo());
    assert!(session.objects.is_empty());
    assert!(!session.can_undo());
}
#[test]
fn arrow_drag_uses_maximum_excursion_and_release_endpoint() {
    let start = (-200., -100.);
    for scale in [1., 1.25] {
        assert!(!arrow_drag_commits(start, start, 2.9 * scale, scale));
        assert!(arrow_drag_commits(
            start,
            (start.0 + 4. * scale, start.1),
            0.,
            scale,
        ));
        assert!(arrow_drag_commits(
            start,
            (start.0 - 4. * scale, start.1 - 4. * scale),
            0.,
            scale,
        ));
        assert!(!arrow_drag_commits(start, start, 20. * scale, scale));
    }
}
#[test]
fn polyline_preview_stays_at_confirmed_node_until_pointer_leaves_it() {
    let monitor = Rect {
        left: -120,
        top: -100,
        right: 120,
        bottom: 100,
    };
    let endpoint = (50., 0.);
    for curved in [false, true] {
        let mut polyline = Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: Bounds::from_corners((-80., -80.), endpoint),
            kind: Kind::Polyline,
            style: Style::default(),
            points: vec![(-80., -80.), (-60., 50.), endpoint],
            extra: Extra::Polyline {
                curved,
                head: false,
                start: Endpoint::None,
                end: Endpoint::None,
            },
        };
        polyline.refresh_bounds();
        for scale in [1., 1.25] {
            let close = (endpoint.0 + 2. * scale, endpoint.1);
            let draft = polyline_preview(&polyline, close, scale);
            assert_eq!(draft, polyline);
            assert_eq!(draft.stroke_points().last().copied(), Some(endpoint));
            let committed = crate::annotation_render::render(
                &Session::new(),
                monitor,
                scale,
                Some(&polyline),
                None,
            )
            .unwrap();
            let preview = crate::annotation_render::render(
                &Session::new(),
                monitor,
                scale,
                Some(&draft),
                None,
            )
            .unwrap();
            assert_eq!(preview.data(), committed.data());

            let far = (endpoint.0 + 10. * scale, endpoint.1);
            let extended = polyline_preview(&polyline, far, scale);
            assert_eq!(extended.points.last().copied(), Some(far));
            assert_eq!(extended.points.len(), polyline.points.len() + 1);
            assert_eq!(polyline.points.last().copied(), Some(endpoint));
            let moving = crate::annotation_render::render(
                &Session::new(),
                monitor,
                scale,
                Some(&extended),
                None,
            )
            .unwrap();
            assert_ne!(moving.data(), committed.data());
        }
    }
}
#[test]
fn geometry_hover_uses_existing_hit_regions_without_selecting_handles() {
    for kind in [Kind::Rectangle, Kind::Ellipse, Kind::HighlightRectangle] {
        let mut session = Session::new();
        let bounds = Bounds::from_corners((20., 20.), (80., 70.));
        assert!(session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind,
            bounds,
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::None,
        }));
        session.selected = None;
        assert_eq!(hovered_geometry_at(&session, (50., 20.), 1.), Some(0));
        assert_eq!(hovered_geometry_at(&session, (5., 5.), 1.), None);
        if kind != Kind::HighlightRectangle {
            assert_eq!(hovered_geometry_at(&session, (50., 45.), 1.), None);
            session.objects[0].style.fill = true;
            assert_eq!(hovered_geometry_at(&session, (50., 45.), 1.), Some(0));
        }
        session.selected = Some(0);
        assert_eq!(hovered_geometry_at(&session, (50., 20.), 1.), None);
    }
}
#[test]
fn toolbar_owner_tracks_physical_monitor_boundaries() {
    let monitors = [
        Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        },
        Rect {
            left: 0,
            top: -200,
            right: 2560,
            bottom: 1440,
        },
    ];
    assert_eq!(canvas_at(monitors, Point { x: -1, y: 300 }), Some(0));
    assert_eq!(canvas_at(monitors, Point { x: 0, y: 300 }), Some(1));
    assert_eq!(canvas_at(monitors, Point { x: 100, y: -100 }), Some(1));
    assert_eq!(canvas_at(monitors, Point { x: 3000, y: 300 }), None);
}
#[test]
fn independent_tool_defaults_match_preview() {
    assert_eq!(defaults(4).color_index, 6);
    assert_eq!(defaults(7).size, 2);
    assert_eq!(defaults(10).text_size, 16);
    assert_eq!(defaults(0).rounding, 21);
    for group in GROUPS {
        for &tool in group {
            assert!(!fields(tool).is_empty());
        }
    }
}
