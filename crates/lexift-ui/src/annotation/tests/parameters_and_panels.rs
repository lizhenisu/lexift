use super::*;
#[test]
fn tool_entries_gate_selection_parameters_and_undo_without_auto_switching() {
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
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let mut object = crate::annotation_sequence::tests::badge(1.);
        object.kind = Kind::Rectangle;
        object.extra = Extra::None;
        object.style.fill = true;
        object.bounds = Bounds::from_corners((-40., -30.), (40., 30.));
        assert!(r.session.add(object));
        let original = r.session.objects[0].clone();
        let target = r.parameter_target(0);
        let mut values = r.values(0).clone();
        values.size = 20;
        assert!(r.select_tool_variant(0, 1));
        assert_eq!(r.session.selected, Some(0), "same family retains selection");
        assert!(r.select_tool_variant(2, 7));
        assert_eq!(r.session.selected, None);
        assert_eq!(r.parameter_object(0), None);
        assert_eq!(r.session.hit(original.bounds.center(), 1.), None);
        assert!(apply_parameter_values(r, target, values).is_none());
        assert_eq!(r.session.objects[0], original);
        assert!(r.select_tool_variant(0, 0));
        assert_eq!(r.session.selected, None);
        assert_eq!(
            r.session.hit(original.bounds.center(), 1.),
            Some(Hit::Object(0))
        );
        r.session.selected = Some(0);
        r.session.update_selected(|object| object.move_by(20., 0.));
        assert!(r.select_tool_variant(2, 7));
    });
    undo();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.mode, InteractionMode::Tool(2));
        assert_eq!(r.selected[2], 7);
        assert_eq!(r.session.selected, None);
    });
    shutdown();
}
#[test]
fn unowned_pointer_completion_does_not_finish_or_cancel_parameter_transactions() {
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
        for cancel_draft in [false, true] {
            let (baseline, opacity) =
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.finish_style_edit();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.selected[0] = 0;
                    r.mode = InteractionMode::Tool(0);
                    r.sync_edit_scope();
                    r.canvas_state.annotation_ready = true;
                    r.canvas_state.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.canvas_state.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -100,
                            top: -100,
                            right: 100,
                            bottom: 100,
                        },
                    ));
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let baseline = r.session.objects[0].clone();
                    let opacity = r.session.spotlight_opacity;
                    let tool = if cancel_draft { 2 } else { 0 };
                    r.selected[0] = tool;
                    r.sync_edit_scope();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(tool);
                    panel.set_values(defaults(tool));
                    r.toolbar_state.panel.open(panel);
                    if cancel_draft {
                        r.session.selected = None;
                    }
                    let mut values = r.values(tool as usize).clone();
                    if cancel_draft {
                        values.strength = 28.;
                    } else {
                        parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33])
                            .apply(&mut values);
                    }
                    let target = r.parameter_target(tool);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    (baseline, opacity)
                });
            if cancel_draft {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    let mut press = drawing::Press::begin(
                        (-50., -50.),
                        Kind::SpotlightRectangle,
                        Style::default(),
                        dpi,
                        &defaults(2),
                    );
                    press.update((40., 30.), dpi);
                    r.gesture = Some(Active::Drawing(press));
                    assert!(cancel_active_gesture(r));
                    assert!(
                        (r.session.spotlight_opacity - 0.28).abs() < 0.0001,
                        "draft cancellation cannot roll back parameter opacity"
                    );
                    r.finish_style_edit();
                    assert!(r.session.undo());
                    assert_eq!(r.session.spotlight_opacity, opacity);
                    assert_eq!(r.session.objects, vec![baseline]);
                });
            } else {
                canvas_pointer(0, 2, 30., 40.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.gesture().is_none());
                    let mut values = r.values(0).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                        .apply(&mut values);
                    let target = r.parameter_target(0);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    r.finish_style_edit();
                    assert!(r.session.undo());
                    assert_eq!(
                        r.session.objects,
                        vec![baseline],
                        "one undo must restore both continuous updates"
                    );
                });
            }
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.session.undo());
                assert!(r.session.objects.is_empty());
                assert!(
                    !r.session.can_undo(),
                    "parameter edits remain a single operation separate from creation"
                );
            });
        }
    }
    shutdown();
}
#[test]
fn geometry_handoff_retires_parameter_idle_without_merging_undo() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Clock(Rc<Cell<Duration>>);
    impl Platform for Clock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
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
        for cancel in [false, true] {
            for close_panel in [false, true] {
                let original = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(0);
                    r.sync_edit_scope();
                    r.canvas_state.annotation_ready = true;
                    r.canvas_state.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.canvas_state.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -100,
                            top: -100,
                            right: 100,
                            bottom: 100,
                        },
                    ));
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let original = r.session.objects[0].clone();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(0);
                    r.toolbar_state.panel.open(panel);
                    restore_parameter_projection(r);
                    let target = r.parameter_target(0);
                    let mut values = r.values(0).clone();
                    values.size = 25;
                    assert!(apply_geometry_values(r, target, values).is_some());
                    assert!(r.parameter_edit.active());
                    original
                });
                // Use the production hit-test/press route, not a manually opened snapshot.
                canvas_pointer(0, 0, 80. / dpi, 70. / dpi);
                canvas_pointer(0, 1, 100. / dpi, 80. / dpi);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(matches!(r.gesture(), Some(Gesture::Move)));
                    assert!(r.geometry_drag().is_some() && r.text_handle().is_none());
                    assert!(!r.parameter_edit.active());
                    if close_panel {
                        r.close_panel();
                    }
                });
                clock.set(clock.get() + Duration::from_millis(600));
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let r = slot.borrow();
                    assert!(matches!(r.as_ref().unwrap().gesture(), Some(Gesture::Move)));
                });
                if cancel {
                    REGISTRY.with(|slot| {
                        assert!(cancel_active_gesture(slot.borrow_mut().as_mut().unwrap()));
                    });
                } else {
                    canvas_pointer(0, 2, 100. / dpi, 80. / dpi);
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.gesture().is_none());
                    assert_eq!(r.session.objects[0].style.width, 25.);
                    if !cancel {
                        assert_ne!(r.session.objects[0].bounds, original.bounds);
                        assert!(
                            r.session.undo(),
                            "movement owns its own undo after idle and panel close"
                        );
                    }
                    assert_eq!(r.session.objects[0].bounds, original.bounds);
                    assert!(
                        r.session.undo(),
                        "earlier parameters remain an independent undo"
                    );
                    assert_eq!(r.session.objects, vec![original]);
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                });
            }
        }
    }
    shutdown();
}
#[test]
fn delayed_parameters_wait_for_document_gesture_commit_or_cancel() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Clock(Rc<Cell<Duration>>);
    impl Platform for Clock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
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
        for cancel in [false, true] {
            let baseline = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.session = Session::new();
                r.parameter_values = parameters::State::new();
                r.mode = InteractionMode::Tool(0);
                r.sync_edit_scope();
                r.canvas_state.annotation_ready = true;
                r.canvas_state.canvases.clear();
                let window = AnnotationCanvas::new().unwrap();
                window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: dpi,
                    });
                r.canvas_state.canvases.push(CanvasLayer::new(
                    window,
                    Rect {
                        left: -100,
                        top: -100,
                        right: 100,
                        bottom: 100,
                    },
                ));
                assert!(r.session.add(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::Rectangle,
                    bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                    style: Style::default(),
                    points: Vec::new(),
                    extra: Extra::None,
                }));
                let baseline = r.session.objects[0].clone();
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(0);
                r.toolbar_state.panel.open(panel);
                restore_parameter_projection(r);
                r.session.begin_drag();
                r.gesture = (gestures::GeometryDrag::moving(&r.session, baseline.id, (0., 0.)))
                    .map(Active::Geometry);
                let target = r.parameter_target(0);
                for rgb in [[11, 22, 33], [44, 55, 66]] {
                    let mut values = r.values(0).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, rgb).apply(&mut values);
                    assert!(apply_geometry_values(r, target, values).is_none());
                }
                r.apply_menu_edit(
                    target,
                    parameters::Edit::Choice(parameters::ChoiceTarget::Style, 1),
                );
                assert_eq!(
                    parameters::style(&r.toolbar_state.panel.get().unwrap().get_values(), 0).color,
                    [44, 55, 66]
                );
                assert_eq!(r.deferred_parameters.len(), 1);
                assert!(!r.parameter_edit.active());
                baseline
            });
            canvas_pointer(0, 1, 120. / dpi, 110. / dpi);
            clock.set(clock.get() + Duration::from_millis(600));
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.session.objects[0].style.color, baseline.style.color);
                r.finish_style_edit();
                assert!(r.gesture().is_some());
            });
            if cancel {
                REGISTRY.with(|slot| {
                    assert!(cancel_active_gesture(slot.borrow_mut().as_mut().unwrap()))
                });
            } else {
                canvas_pointer(0, 2, 120. / dpi, 110. / dpi);
            }
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.deferred_parameters.is_empty());
                assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                r.finish_style_edit();
                assert!(r.session.undo(), "parameter update owns a separate undo");
                assert_eq!(r.session.objects[0].style, baseline.style);
                if !cancel {
                    assert_ne!(r.session.objects[0].bounds, baseline.bounds);
                    assert!(
                        r.session.undo(),
                        "movement remains one undo after idle time"
                    );
                }
                assert_eq!(r.session.objects, vec![baseline]);
                assert!(r.session.undo());
                assert!(r.session.objects.is_empty());
                assert!(!r.session.can_undo());
            });
        }
    }
    shutdown();
}
#[test]
fn brush_wheel_updates_open_panel_parameters_without_feedback_callbacks() {
    use lexift_core::ports::screen_capture::{
        ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
    };
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    struct Capture;
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
            Ok(None)
        }
        fn clear(&self) {}
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
        )
        .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
    );
    for dpi in [1., 1.25] {
        for selected in [false, true] {
            let original = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.session = Session::new();
                r.parameter_values = parameters::State::new();
                r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
                r.canvas_state.annotation_ready = true;
                r.mode = InteractionMode::Tool(5);
                r.sync_edit_scope();
                r.selected[5] = 11;
                r.sync_edit_scope();
                r.edit_values(11).shape = 1;
                r.canvas_state.canvases.clear();
                let window = AnnotationCanvas::new().unwrap();
                window
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                r.canvas_state.canvases.push(CanvasLayer::new(
                    window,
                    Rect {
                        left: -500,
                        top: -300,
                        right: 500,
                        bottom: 500,
                    },
                ));
                if selected {
                    let mut object = Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind: Kind::MosaicBrush,
                        bounds: Bounds::from_corners((-200., -100.), (-180., -80.)),
                        style: Style::default(),
                        points: vec![(-200., -100.), (-180., -80.)],
                        extra: Extra::Mosaic(
                            lexift_core::domain::annotation::MosaicAnnotation::new(
                                lexift_core::domain::annotation::MosaicEffect::Blur,
                                70,
                                12.,
                                dpi,
                            ),
                        ),
                    };
                    object.refresh_bounds();
                    assert!(r.session.add(object));
                }
                restore_parameter_projection(r);
                r.session.objects.first().cloned()
            });
            open_panel(5, false);
            brush_size_scrolled(0, 1.);
            brush_size_scrolled(0, 1.);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.values(11).size, 14);
                assert_eq!(r.toolbar_state.panel.get().unwrap().get_values().size, 14);
                if let Some(original) = original {
                    let Extra::Mosaic(value) = &r.session.objects[0].extra else {
                        panic!()
                    };
                    assert_eq!(value.diameter, 12. * dpi);
                    assert_eq!(
                        value.effect,
                        lexift_core::domain::annotation::MosaicEffect::Blur
                    );
                    assert_eq!(value.strength, 70);
                    r.finish_style_edit();
                    assert_eq!(r.session.objects, vec![original]);
                    assert_eq!(r.session.selected, None);
                    assert!(r.session.undo());
                } else {
                    r.finish_style_edit();
                }
                assert!(!r.session.can_undo());
            });
        }
    }
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.close_panel();
        r.session = Session::new();
        r.parameter_values = parameters::State::new();
        r.mode = InteractionMode::Tool(6);
        r.sync_edit_scope();
        r.edit_values(13).shape = 1;
    });
    open_panel(6, false);
    let (panel, generation, revision) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (
            r.toolbar_state.panel.get().unwrap().clone_strong(),
            r.generation,
            r.toolbar_state.panel.revision(),
        )
    });
    panel.window().set_size(slint::LogicalSize::new(329., 58.));
    panel.show().unwrap();
    let wheel = |x, delta| {
        panel.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(x, 29.),
            delta_x: 0.,
            delta_y: delta,
        });
    };
    wheel(120., 120.); // Icon hit area.
    assert_eq!(panel.get_values().size, 13);
    choice::open(
        generation,
        revision,
        crate::AnnotationChoiceRequest {
            field: -9,
            ..Default::default()
        },
    );
    wheel(162., 120.); // Numeric chip, with the popup open.
    assert_eq!(panel.get_values().size, 14);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.values(13).size, 14);
        assert_eq!(
            r.toolbar_state
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .get_numeric_value(),
            14
        );
        assert!(!r.session.can_undo());
    });
    for _ in 0..110 {
        wheel(120., 120.);
    }
    assert_eq!(panel.get_values().size, 100);
    for _ in 0..110 {
        wheel(162., -120.);
    }
    assert_eq!(panel.get_values().size, 1);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let press = eraser::Press::begin(&mut r.session, (0., 0.), true, 1., 1., &[]);
        r.gesture = Some(Active::Erasing(press));
    });
    wheel(120., 120.);
    assert_eq!(
        panel.get_values().size,
        2,
        "wheel updates the next gesture only"
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let Some(Active::Erasing(press)) = &r.gesture else {
            panic!()
        };
        assert_eq!(press.diameter, 1., "current stroke diameter remains frozen");
        r.gesture = None;
        r.session.cancel_drag();
        r.edit_values(13).shape = 0;
        panel.set_values(r.values(13).clone());
    });
    wheel(162., 120.);
    assert_eq!(panel.get_values().size, 1, "rectangle mode disables wheel");
    REGISTRY.with(|slot| assert!(!slot.borrow().as_ref().unwrap().session.can_undo()));
    shutdown();
    panel.invoke_numeric_scrolled(-9, 120.); // A retired panel must be harmless.
}
#[test]
fn queued_panel_field_update_preserves_a_later_menu_field() {
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
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.canvas_state.annotation_ready = true;
        assert!(r.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        }));
        restore_parameter_projection(r);
    });
    open_panel(0, false);
    // Capture a toolbar edit, then let a menu update a different field
    // before the deferred toolbar callback reaches the Registry.
    let (panel, target) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (
            r.toolbar_state.panel.get().unwrap().clone_strong(),
            r.parameter_target(0),
        )
    });
    let mut values = panel.get_values();
    values.size = 25;
    panel.invoke_values_changed(values);
    REGISTRY.with(|slot| {
        slot.borrow_mut().as_mut().unwrap().apply_menu_edit(
            target,
            parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33]),
        );
    });
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert_eq!(r.session.objects[0].style.width, 25.);
        assert_eq!(r.session.objects[0].style.color, [11, 22, 33]);
        r.finish_style_edit();
        assert!(r.session.undo());
        assert_eq!(r.session.objects[0].style, Style::default());
    });
    shutdown();
}
#[test]
fn queued_object_panel_requires_the_originating_canvas_selection_and_session() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Clock(Rc<Cell<Duration>>);
    impl Platform for Clock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
    slint::platform::update_timers_and_animations();
    let advance = || {
        clock.set(clock.get() + Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
    };
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
    let bounds = Rect {
        left: -100,
        top: -100,
        right: 100,
        bottom: 100,
    };
    let object = Object {
        erased: Vec::new(),
        id: Default::default(),
        kind: Kind::Rectangle,
        bounds: Bounds::from_corners((-30., -30.), (30., 30.)),
        style: Style::default(),
        points: Vec::new(),
        extra: Extra::None,
    };
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.canvas_state
            .canvases
            .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
        assert!(r.session.add(object.clone()));
        r.generation
    });
    queue_geometry_panel(generation, 0);
    REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().session.selected = None);
    advance();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(r.toolbar_state.panel.get().is_none());
        r.session.selected = Some(0);
    });
    queue_geometry_panel(generation, 0);
    REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().mode = InteractionMode::Mouse);
    advance();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(r.toolbar_state.panel.get().is_none());
        r.mode = InteractionMode::Tool(0);
        r.sync_edit_scope();
    });
    queue_geometry_panel(generation, 0);
    REGISTRY.with(|slot| {
        slot.borrow_mut().as_mut().unwrap().canvas_state.canvases[0] =
            CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds)
    });
    advance();
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .panel
                .get()
                .is_none()
        )
    });
    queue_geometry_panel(generation, 0);
    advance();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(
            r.toolbar_state.panel.get().is_some(),
            "current request should open its panel"
        );
        r.close_panel();
    });
    queue_geometry_panel(generation, 0);
    close();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert_ne!(r.generation, generation);
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.canvas_state
            .canvases
            .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
        assert!(r.session.add(object));
    });
    advance();
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .panel
                .get()
                .is_none()
        )
    });
    shutdown();
}
#[test]
fn owner_close_rejects_reentrant_pointer_input_from_child_menu_teardown() {
    exercise_menu_owner_teardown(true, false);
}
#[test]
fn child_menu_close_rejects_cleanup_input_without_retiring_its_owner() {
    exercise_menu_owner_teardown(false, false);
}
#[test]
fn child_menu_close_rejects_reentrant_input_to_another_live_canvas() {
    exercise_menu_owner_teardown(false, true);
}
#[test]
fn parameter_panels_ignore_native_dismissal_while_tool_menus_still_close() {
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    thread_local! { static WATCHES: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) }; }
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
    // Native placement now requires a known work area; provide one in the fixture.
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -2000,
                top: -1000,
                right: 2000,
                bottom: 2000,
            })
        });
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.lifecycle.set_popup_dismissal = Rc::new(|_, enabled| {
            WATCHES.with(|events| events.borrow_mut().push(enabled));
            true
        });
    });
    for dpi in [1., 1.25] {
        for group in [0, 4] {
            open_panel(group, false);
            let (generation, revision, sink, retired_panel) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let panel = r.toolbar_state.panel.get().unwrap();
                panel
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                (
                    r.generation,
                    r.toolbar_state.panel.revision(),
                    pointer_panel(
                        panel.as_weak(),
                        r.generation,
                        r.toolbar_state.panel.revision(),
                        r.toolbar_state.panel.input(),
                    ),
                    panel.clone_strong(),
                )
            });
            WATCHES.with(|events| events.borrow_mut().clear());
            for attempt in 0..4 {
                show_panel(generation, revision, attempt);
            }
            REGISTRY.with(|slot| choice::rearm_panel(slot.borrow().as_ref().unwrap()));
            WATCHES.with(|events| {
                assert!(!events.borrow().is_empty());
                assert!(events.borrow().iter().all(|enabled| !enabled));
            });
            for cause in [
                PopupDismissCause::PointerOutside,
                PopupDismissCause::ForegroundChanged,
            ] {
                sink(PopupPointerInput::DismissRequested { cause });
            }
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(r.toolbar_state.panel.revision(), revision);
                assert!(r.toolbar_state.panel.get().is_some_and(|p| !p.get_menu()));
            });
            // Queue parameter work before retiring the source, then keep the component
            // alive to exercise the identity fence rather than weak-handle expiry.
            let mut old_values = retired_panel.get_values();
            let original_size = old_values.size;
            old_values.size = 91;
            retired_panel.invoke_values_changed(old_values);
            // The same button explicitly toggles the panel closed.
            open_panel(group, false);
            REGISTRY.with(|slot| {
                assert!(
                    slot.borrow()
                        .as_ref()
                        .unwrap()
                        .toolbar_state
                        .panel
                        .get()
                        .is_none()
                )
            });
            open_panel(group, true);
            let (generation, revision, menu_sink) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let panel = r.toolbar_state.panel.get().unwrap();
                (
                    r.generation,
                    r.toolbar_state.panel.revision(),
                    pointer_panel(
                        panel.as_weak(),
                        r.generation,
                        r.toolbar_state.panel.revision(),
                        r.toolbar_state.panel.input(),
                    ),
                )
            });
            for attempt in 0..4 {
                show_panel(generation, revision, attempt);
            }
            WATCHES.with(|events| assert_eq!(events.borrow().last(), Some(&true)));
            // An old parameter-window event cannot dismiss its replacement menu.
            sink(PopupPointerInput::DismissRequested {
                cause: PopupDismissCause::PointerOutside,
            });
            sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
            retired_panel.invoke_escape_requested();
            retired_panel.invoke_watermark_edit_requested();
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(r.toolbar_state.panel.revision(), revision);
                assert!(r.toolbar_state.panel.get().is_some_and(|p| p.get_menu()));
                assert!(r.toolbar_state.watermark.get().is_none());
                assert!(!r.session.can_undo());
                assert_eq!(
                    r.values(retired_panel.get_tool() as usize).size,
                    original_size
                );
            });
            menu_sink(PopupPointerInput::DismissRequested {
                cause: PopupDismissCause::PointerOutside,
            });
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                assert!(
                    slot.borrow()
                        .as_ref()
                        .unwrap()
                        .toolbar_state
                        .panel
                        .get()
                        .is_none()
                )
            });
            open_panel(group, true);
            let (generation, revision, foreground_sink) = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                let panel = r.toolbar_state.panel.get().unwrap();
                let position = panel.window().position();
                let sink = pointer_panel(
                    panel.as_weak(),
                    r.generation,
                    r.toolbar_state.panel.revision(),
                    r.toolbar_state.panel.input(),
                );
                r.lifecycle.toolbar_cursor_position = Rc::new(move || {
                    Some(Point {
                        x: position.x + 1,
                        y: position.y + 1,
                    })
                });
                (r.generation, r.toolbar_state.panel.revision(), sink)
            });
            for attempt in 0..4 {
                show_panel(generation, revision, attempt);
            }
            foreground_sink(PopupPointerInput::DismissRequested {
                cause: PopupDismissCause::ForegroundChanged,
            });
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(
                    r.toolbar_state.panel.get().is_none(),
                    "foreground dismissal bypasses the pointer guard for tool menus"
                );
                r.lifecycle.toolbar_cursor_position = Rc::new(|| None);
            });
        }
    }
    shutdown();
}
#[test]
fn undo_same_object_rejects_queued_parameter_and_menu_edits() {
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
    let (generation, identity) = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        assert!(r.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-100., 50.), (100., 150.)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::None,
        }));
        adopt_selected_style(r, 0);
        (r.generation, r.session.objects[0].id)
    });
    open_panel(0, false);
    // Native placement now requires a known work area; provide one in the fixture.
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -2000,
                top: -1000,
                right: 2000,
                bottom: 2000,
            })
        });
    });
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let panel = slot.as_ref().unwrap().toolbar_state.panel.get().unwrap();
        let mut values = panel.get_values();
        values.size = 11;
        panel.set_values(values.clone());
        panel.invoke_values_changed(values);
    });
    slint::platform::update_timers_and_animations();
    let panel_revision = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.finish_style_edit();
        assert_eq!(r.session.objects[0].style.width, 11.);
        r.toolbar_state.panel.revision()
    });
    choice::open(
        generation,
        panel_revision,
        crate::AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["Rectangle".into(), "Ellipse".into()])),
            width: 180.,
            height: 34.,
            ..Default::default()
        },
    );
    let retained_menu = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        let mut stale = r.toolbar_state.panel.get().unwrap().get_values();
        stale.size = 80;
        r.toolbar_state
            .panel
            .get()
            .unwrap()
            .invoke_values_changed(stale);
        r.toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .clone_strong()
    });
    retained_menu.invoke_chosen(1);
    undo();
    retained_menu.invoke_chosen(1);
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.session.objects[0].id, identity);
        assert_eq!(r.session.objects[0].kind, Kind::Rectangle);
        assert_eq!(r.session.objects[0].style.width, Style::default().width);
        assert_eq!(
            r.toolbar_state.panel.get().unwrap().get_values().size,
            Style::default().width as i32
        );
        assert!(r.toolbar_state.menus.current.is_none());
    });
    undo();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.session.objects.is_empty());
        assert!(
            !r.session.can_undo(),
            "retired callbacks added history after undo"
        );
        assert!(r.toolbar_state.panel.get().is_some());
    });
    shutdown();
}
#[test]
fn mosaic_cancellation_and_long_slider_drag_preserve_document_and_panel() {
    use lexift_core::ports::screen_capture::{
        ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
    };
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    struct Capture;
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
            Ok(None)
        }
        fn clear(&self) {}
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
        )
        .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
    );
    for dpi in [1., 1.25] {
        for shape in [0, 1] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
                r.canvas_state.pending_displays = vec![Rect {
                    left: -500,
                    top: -300,
                    right: 500,
                    bottom: 500,
                }];
                create_canvas(r, 0);
                r.canvas_state.canvases[0]
                    .window
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                r.canvas_state.annotation_ready = true;
                r.mode = InteractionMode::Tool(5);
                r.sync_edit_scope();
                r.selected[5] = 11;
                r.sync_edit_scope();
                r.edit_values(11).shape = shape;
                r.edit_values(11).strength = 0.;
            });
            open_panel(5, false);
            for cancel in 0..4 {
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 1, 200., 180.);
                match cancel {
                    0 => escape(),
                    1 => canvas_pointer(0, 3, 200., 180.),
                    2 => {
                        let sink = REGISTRY.with(|slot| {
                            pointer_canvas(
                                slot.borrow().as_ref().unwrap().canvas_state.canvases[0]
                                    .window
                                    .as_weak(),
                                slot.borrow().as_ref().unwrap().generation,
                                0,
                                slot.borrow().as_ref().unwrap().canvas_state.canvases[0].revision(),
                                slot.borrow().as_ref().unwrap().canvas_state.canvases[0].input(),
                            )
                        });
                        sink(PopupPointerInput::LeftCancelled {
                            x: 200. * dpi,
                            y: 180. * dpi,
                        });
                    }
                    _ => REGISTRY.with(|slot| {
                        slot.borrow_mut()
                            .as_mut()
                            .unwrap()
                            .set_mode(InteractionMode::Tool(0));
                    }),
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(
                        gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                            && r.gesture().is_none()
                    );
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    assert!(r.render_state.mosaic.pending.is_empty());
                    r.mode = InteractionMode::Tool(5);
                    r.sync_edit_scope();
                });
            }
            if REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .toolbar_state
                    .panel
                    .get()
                    .is_none()
            }) {
                open_panel(5, false);
            }
            canvas_pointer(0, 0, 80., 80.);
            canvas_pointer(0, 1, 200., 180.);
            canvas_pointer(0, 2, 200., 180.);
            let panel = REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .toolbar_state
                    .panel
                    .get()
                    .unwrap()
                    .clone_strong()
            });
            panel.invoke_parameter_dragging(true);
            slint::platform::update_timers_and_animations();
            for strength in [20., 40., 80.] {
                let mut values = panel.get_values();
                values.strength = strength;
                panel.set_values(values.clone());
                panel.invoke_values_changed(values);
                slint::platform::update_timers_and_animations();
                std::thread::sleep(Duration::from_millis(350));
                slint::platform::update_timers_and_animations();
                REGISTRY
                    .with(|slot| assert!(slot.borrow().as_ref().unwrap().parameter_edit.active()));
            }
            panel.invoke_parameter_dragging(false);
            slint::platform::update_timers_and_animations();
            if shape == 1 {
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.session.selected, None);
                    assert_eq!(r.values(11).strength, 80.);
                    assert!(matches!(&r.session.objects[0].extra,
                            Extra::Mosaic(value) if value.strength == 0));
                });
                canvas_pointer(0, 0, 140., 130.);
                canvas_pointer(0, 1, 250., 250.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(r.drawing_press().is_some());
                    assert!(r.geometry_drag().is_none());
                });
                canvas_pointer(0, 3, 250., 250.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(r.session.objects.len(), 1);
                    assert_eq!(r.values(11).strength, 80.);
                    assert!(r.toolbar_state.panel.get().is_some());
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    r.close();
                });
                continue;
            }
            let press_undo = || {
                panel.window().dispatch_event(WindowEvent::KeyPressed {
                    text: slint::platform::Key::Control.into(),
                });
                panel
                    .window()
                    .dispatch_event(WindowEvent::KeyPressed { text: "z".into() });
                panel
                    .window()
                    .dispatch_event(WindowEvent::KeyReleased { text: "z".into() });
                panel.window().dispatch_event(WindowEvent::KeyReleased {
                    text: slint::platform::Key::Control.into(),
                });
                slint::platform::update_timers_and_animations();
            };
            panel.set_parameter_input_active(true);
            press_undo();
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                assert!(matches!(&slot.as_ref().unwrap().session.objects[0].extra,
                        Extra::Mosaic(value) if value.strength == 80));
            });
            panel.set_parameter_input_active(false);
            press_undo();
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(
                    r.toolbar_state.panel.get().is_some()
                        && !r.parameter_edit.dragging()
                        && !r.parameter_edit.active()
                );
                let Extra::Mosaic(value) = &r.session.objects[0].extra else {
                    panic!()
                };
                assert_eq!(
                    value.strength, 0,
                    "one undo must restore the entire slider gesture"
                );
                assert_eq!(value.creation_scale, dpi);
            });
            let placed =
                REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().session.objects[0].clone());
            canvas_pointer(0, 0, 140., 130.);
            canvas_pointer(0, 1, 250., 250.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(matches!(r.gesture(), Some(Gesture::Move)));
                assert_ne!(r.session.objects[0].bounds, placed.bounds);
            });
            canvas_pointer(0, 3, 250., 250.);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(
                    r.session.objects[0], placed,
                    "capture loss must restore a moved object"
                );
                assert!(
                    r.toolbar_state.panel.get().is_some(),
                    "canvas operations must preserve the parameter panel"
                );
                assert!(r.session.undo());
                assert!(r.session.objects.is_empty() && !r.session.can_undo());
                r.close();
            });
            // A destroyed panel's queued callback cannot undo a new session.
            let replacement = Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((0., 0.), (50., 50.)),
                points: vec![],
                style: Style::default(),
                extra: Extra::None,
            };
            let replacement = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let session = &mut slot.as_mut().unwrap().session;
                session.add(replacement);
                session.objects.last().unwrap().clone()
            });
            panel.invoke_undo_requested();
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.session.objects, vec![replacement]);
                r.close();
            });
        }
    }
    close();
}
#[test]
fn mosaic_panel_updates_effect_without_converting_existing_geometry() {
    use lexift_core::ports::screen_capture::{
        ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
    };
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
    struct Capture;
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
            Ok(None)
        }
        fn clear(&self) {}
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
        )
        .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        assert!(r.set_mode(InteractionMode::Tool(5)));
        assert!(r.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((-100., 20.), (100., 100.)),
            points: Vec::new(),
            style: Style::default(),
            extra: mosaic::settings(r.values(11), 1.25)
        }));
    });
    open_panel(5, false);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let panel = slot.as_ref().unwrap().toolbar_state.panel.get().unwrap();
        let mut v = panel.get_values();
        v.mode = 1;
        v.shape = 1;
        v.strength = 80.;
        panel.set_values(v.clone());
        panel.invoke_values_changed(v);
    });
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.session.objects[0].kind, Kind::MosaicArea);
        let Extra::Mosaic(value) = &r.session.objects[0].extra else {
            panic!()
        };
        assert_eq!(
            value.effect,
            lexift_core::domain::annotation::MosaicEffect::Blur
        );
        assert_eq!(value.strength, 80);
        assert_eq!(value.creation_scale, 1.25);
        assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let style = current_style(r, 11);
        let values = r.values(11).clone();
        let mut press =
            drawing::Press::begin((-150., 50.), Kind::MosaicBrush, style, 1.25, &values);
        press.update((-50., 100.), 1.);
        r.gesture = Some(Active::Drawing(press));
        let Extra::Mosaic(value) = &gestures::display_draft(&r.gesture, &r.pending_drawing)
            .unwrap()
            .extra
        else {
            panic!()
        };
        assert_eq!(value.creation_scale, 1.25);
        r.finish_style_edit();
        r.session.begin_drag();
        r.session.objects[0].move_by(40., 20.);
        r.session.finish_drag();
        adopt_selected_style(r, 0);
        assert_eq!(r.values(11).strength, 80.);
        assert_eq!(
            r.toolbar_state.panel.get().unwrap().get_values().strength,
            80.,
            "mosaic selection must not inherit spotlight opacity"
        );
        r.gesture = None;
        deselect_object(r);
        assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
        assert_eq!(r.values(11).strength, 80.);
        assert_eq!(r.values(11).mode, 1);
        r.session.selected = Some(0);
        adopt_selected_style(r, 0);
        assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
        assert_eq!(r.values(11).strength, 80.);
    });
    close();
}
#[test]
fn caption_color_inherits_badge_until_explicitly_chosen() {
    let mut object = crate::annotation_sequence::tests::badge(1.);
    let mut values = defaults(10);
    let blue = Style {
        color: [89, 143, 223],
        ..Style::default()
    };
    update_sequence_style(&mut object, &values, &[], blue);
    let Extra::Sequence(s) = &object.extra else {
        unreachable!()
    };
    assert_eq!(s.caption_color, blue.color);
    assert!(!s.caption_custom_color);
    values.sequence_text_color = "#123456".into();
    update_sequence_style(&mut object, &values, &[], Style::default());
    let Extra::Sequence(s) = &object.extra else {
        unreachable!()
    };
    assert_eq!(s.caption_color, [18, 52, 86]);
    assert!(s.caption_custom_color);
    assert_eq!(s.endpoint, Endpoint::FilledArrow);
}
#[test]
fn pending_mosaic_cancel_preserves_unrelated_parameter_undo() {
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
        for operation in 0..4 {
            let (baseline, opacity) = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.session = Session::new();
                r.render_state.mosaic = mosaic::State::default();
                r.parameter_values = parameters::State::new();
                r.mode = InteractionMode::Tool(0);
                r.sync_edit_scope();
                r.selected[0] = 2;
                r.sync_edit_scope();
                assert!(r.session.add(Object {
                    erased: Vec::new(),
                    id: ObjectId::DRAFT,
                    kind: Kind::Rectangle,
                    bounds: Bounds::from_corners((-120., 30.), (-40., 90.)),
                    style: Style::default(),
                    points: Vec::new(),
                    extra: Extra::None,
                }));
                let baseline = r.session.objects[0].clone();
                let opacity = r.session.spotlight_opacity;
                r.selected[0] = 2;
                assert!(r.set_mode(InteractionMode::Tool(0)));
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(2);
                r.toolbar_state.panel.open(panel);
                let target = r.parameter_target(2);
                let mut values = r.values(2).clone();
                values.strength = 28.;
                assert!(apply_geometry_values(r, target, values).is_some());
                r.finish_style_edit();
                assert!(r.set_mode(InteractionMode::Tool(5)));
                r.render_state.mosaic.stage(
                    &mut r.session,
                    Object {
                        erased: Vec::new(),
                        id: ObjectId::DRAFT,
                        kind: Kind::MosaicArea,
                        extra: Extra::Mosaic(
                            lexift_core::domain::annotation::MosaicAnnotation::new(
                                lexift_core::domain::annotation::MosaicEffect::Blur,
                                50,
                                12.,
                                dpi,
                            ),
                        ),
                        ..baseline.clone()
                    },
                );
                assert_eq!(r.render_state.mosaic.pending.len(), 1);
                if operation == 0 {
                    r.session.selected = None;
                } else {
                    let mask = r.session.objects[1].clone();
                    r.session.begin_drag();
                    r.gesture = (gestures::GeometryDrag::moving(&r.session, mask.id, (-80., 60.)))
                        .map(Active::Geometry);
                    r.session.objects[1].move_by(20., 10.);
                }

                (baseline, opacity)
            });
            if operation == 1 {
                REGISTRY.with(|slot| mosaic::cancel_pending(slot.borrow_mut().as_mut().unwrap()));
            } else if operation == 3 {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.mode = InteractionMode::Tool(5);
                    r.sync_edit_scope();
                    assert!(r.set_mode(InteractionMode::Tool(0)));
                });
            } else {
                escape();
            }
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.render_state.mosaic.pending.is_empty());
                assert!(r.gesture().is_none() && r.text_handle().is_none());
                assert!(r.geometry_drag().is_none());
                assert!(r.deferred_parameters.is_empty());
                assert!(!r.parameter_edit.active());
                assert_eq!(r.session.objects, vec![baseline.clone()]);
                assert!((r.session.spotlight_opacity - 0.28).abs() < 0.0001);
                assert!(
                    r.session.undo(),
                    "unrelated parameter history survives mask rejection"
                );
                assert_eq!(r.session.spotlight_opacity, opacity);
                assert_eq!(r.session.objects, vec![baseline]);
                assert!(r.session.undo());
                assert!(r.session.objects.is_empty());
                assert!(
                    !r.session.can_undo(),
                    "rejected masks and their movement leave no history"
                );
            });
        }
    }
    shutdown();
}
#[test]
fn panel_owner_failure_retries_without_touching_reopened_instance() {
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
    thread_local! { static NATIVE_READY: Cell<bool> = const { Cell::new(false) }; }
    init(
        |_| {
            if NATIVE_READY.with(Cell::get) {
                PassiveWindowPreparation::Ready
            } else {
                PassiveWindowPreparation::Pending
            }
        },
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
    let ready = Rc::new(Cell::new(false));
    let calls = Rc::new(Cell::new(0));
    let completed = Rc::new(Cell::new(0));
    // Native placement now requires a known work area; provide one in the fixture.
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -2000,
                top: -1000,
                right: 2000,
                bottom: 2000,
            })
        });
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        let completed = completed.clone();
        r.lifecycle.complete_passive_window_show = Rc::new(move |window, _| {
            assert!(
                !window.is_visible(),
                "input and frame preparation must precede visibility"
            );
            completed.set(completed.get() + 1);
            true
        });
        let (ready, calls) = (ready.clone(), calls.clone());
        r.lifecycle.attach_tool_window = Rc::new(move |_, _| {
            calls.set(calls.get() + 1);
            ready.get()
        });
    });
    open_panel(0, true);
    let (generation, old_revision) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (r.generation, r.toolbar_state.panel.revision())
    });
    show_panel(generation, old_revision, 0);
    assert_eq!(
        calls.get(),
        0,
        "pending HWND must not prime visibility or ownership"
    );
    REGISTRY.with(|slot| {
        assert!(
            !slot
                .borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .panel
                .get()
                .unwrap()
                .window()
                .is_visible()
        )
    });
    NATIVE_READY.with(|ready| ready.set(true));
    show_panel(generation, old_revision, 0);
    assert_eq!(calls.get(), 1);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let panel = slot.as_ref().unwrap().toolbar_state.panel.get().unwrap();
        assert!(!panel.window().is_visible());
    });
    assert_eq!(completed.get(), 0);
    ready.set(true);
    show_panel(generation, old_revision, 1);
    assert_eq!(completed.get(), 0, "first layout observation stays hidden");
    show_panel(generation, old_revision, 2);
    assert_eq!(completed.get(), 1);
    assert_eq!(
        calls.get(),
        3,
        "owner must be established on every hidden preparation pass"
    );
    REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_panel());
    open_panel(0, true);
    let revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .panel
            .revision()
    });
    assert_ne!(revision, old_revision);
    show_panel(generation, old_revision, 20);
    assert_eq!(calls.get(), 3, "old retry must not operate a new component");
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
    ready.set(false);
    show_panel(generation, revision, 20);
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .panel
                .get()
                .is_none()
        )
    });
    shutdown();
}
#[test]
fn panels_flip_and_clamp_on_negative_monitors() {
    let area = Rect {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    assert_eq!(
        panel_position(Point { x: -100, y: 1000 }, 54, 720, 204, area, 8),
        Point { x: -720, y: 788 }
    );
}

#[test]
fn tool_group_menus_skip_fonts_parameters_and_keep_final_layout() {
    use slint::Model;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
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
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.lifecycle.popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -1200,
                top: -900,
                right: 1800,
                bottom: 1100,
            })
        });
    });
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for scale in [1., 1.25] {
            for dark in [false, true] {
                for (group, tools) in GROUPS.iter().enumerate() {
                    // Exercise both text and watermark as the remembered selection.
                    for &tool in *tools {
                        let (mode, binding) = REGISTRY.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let r = slot.as_mut().unwrap();
                            r.selected[group] = tool;
                            (r.mode, r.parameter_values.binding().get())
                        });
                        open_panel(group, true);
                        let (generation, revision) = REGISTRY.with(|slot| {
                            let slot = slot.borrow();
                            let r = slot.as_ref().unwrap();
                            assert!(!r.fonts.is_loaded(), "picker must not scan installed fonts");
                            assert_eq!(r.mode, mode);
                            assert_eq!(r.parameter_values.binding().get(), binding);
                            assert!(!r.parameter_edit.active());
                            let panel = r.toolbar_state.panel.get().unwrap();
                            assert_eq!(panel.get_font_families().row_count(), 0);
                            assert_eq!(panel.get_fields().row_count(), 0);
                            panel.global::<crate::Colors>().set_preference(if dark {
                                1
                            } else {
                                0
                            });
                            panel
                                .window()
                                .dispatch_event(WindowEvent::ScaleFactorChanged {
                                    scale_factor: scale,
                                });
                            (r.generation, r.toolbar_state.panel.revision())
                        });
                        show_panel(generation, revision, 0);
                        show_panel(generation, revision, 1);
                        REGISTRY.with(|slot| {
                            let slot = slot.borrow();
                            let r = slot.as_ref().unwrap();
                            let panel = r.toolbar_state.panel.get().unwrap();
                            assert!(r.toolbar_state.panel_preparation.ready);
                            assert!(panel.window().is_visible());
                            assert_eq!(
                                panel.window().size(),
                                toolbar::physical_size(220., 46. + tools.len() as f32 * 42., scale)
                            );
                            if group == 3
                                && tool == 9
                                && let Ok(directory) =
                                    std::env::var("LEXIFT_ANNOTATION_SNAPSHOT_DIR")
                            {
                                let pixels = panel.window().take_snapshot().unwrap();
                                crate::i18n::write_test_bitmap(
                                    &directory,
                                    &format!("tool-menu-{}-{dark}-{scale}", language.code()),
                                    pixels.width(),
                                    pixels.height(),
                                    pixels.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                                );
                            }
                        });
                        open_panel(group, true);
                        REGISTRY.with(|slot| {
                            assert!(
                                slot.borrow()
                                    .as_ref()
                                    .unwrap()
                                    .toolbar_state
                                    .panel
                                    .get()
                                    .is_none()
                            )
                        });
                    }
                }
            }
        }
    }
    crate::i18n::select(lexift_core::domain::ui_language::UiLanguage::EnglishUs);
    shutdown();
}
