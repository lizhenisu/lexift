use super::*;
#[test]
fn native_pointer_reentry_is_deferred_and_retirement_cancels_replay() {
    use super::*;
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
    let panel = AnnotationPanel::new().unwrap();
    let main = AnnotationToolbar::new().unwrap();
    let canvas = AnnotationCanvas::new().unwrap();
    for target in 0..3 {
        let input = window::InputWork::new();
        let sink = match target {
            0 => pointer_panel(panel.as_weak(), 0, 0, input.clone()),
            1 => pointer_main(main.as_weak(), 0, 0, input.clone()),
            _ => pointer_canvas(canvas.as_weak(), 0, 0, 0, input.clone()),
        };
        REGISTRY.with(|slot| {
            let _updating_session = slot.borrow_mut();
            sink(PopupPointerInput::LeftPressed { x: 25., y: 25. });
            sink(PopupPointerInput::LeftReleased { x: 25., y: 25. });
            assert!(input.pending());
        });
        slint::platform::update_timers_and_animations();
        assert!(
            !input.pending(),
            "stale window replay must be rejected safely"
        );
        REGISTRY.with(|slot| {
            let _updating_session = slot.borrow_mut();
            sink(PopupPointerInput::LeftPressed { x: 25., y: 25. });
            assert!(input.pending());
            input.retire();
        });
        slint::platform::update_timers_and_animations();
        assert!(input.retired() && !input.pending());
    }
    let work = window::InputWork::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let sink = registry_pointer(
        work.clone(),
        Rc::new(move |event| {
            if let PopupPointerInput::Moved { x, .. } = event {
                received.borrow_mut().push(x);
            }
        }),
    );
    sink(PopupPointerInput::Moved { x: 1., y: 0. });
    assert_eq!(
        *events.borrow(),
        [1.],
        "normal pointer delivery stays immediate"
    );
    REGISTRY.with(|slot| {
        let _updating = slot.borrow_mut();
        sink(PopupPointerInput::Moved { x: 2., y: 0. });
        sink(PopupPointerInput::Moved { x: 3., y: 0. });
        assert_eq!(*events.borrow(), [1.]);
    });
    sink(PopupPointerInput::Moved { x: 3.5, y: 0. });
    assert_eq!(
        *events.borrow(),
        [1.],
        "later input cannot overtake queued input"
    );
    slint::platform::update_timers_and_animations();
    assert_eq!(
        *events.borrow(),
        [1., 2., 3., 3.5],
        "reentrant events keep FIFO order"
    );
    REGISTRY.with(|slot| {
        let _updating = slot.borrow_mut();
        sink(PopupPointerInput::Moved { x: 4., y: 0. });
    });
    work.retire();
    slint::platform::update_timers_and_animations();
    assert_eq!(
        *events.borrow(),
        [1., 2., 3., 3.5],
        "retirement cancels replay"
    );
}
#[test]
fn interrupted_drawing_cannot_commit_after_tool_switch_escape_or_capture_loss() {
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
    for dpi in [1., 1.25] {
        for pencil in [false, true] {
            for interruption in 0..5 {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
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
                    r.document.mode = InteractionMode::Tool(if pencil { 1 } else { 0 });
                    r.sync_edit_scope();
                    if pencil {
                        r.document.selected[1] = 3;
                        r.sync_edit_scope();
                    }
                });
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 1, 180., 180.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(matches!(r.gesture(), Some(Gesture::Draw { .. })));
                    assert!(gestures::display_draft(&r.gesture, &r.pending_drawing).is_some());
                    assert!(
                        r.document.session.objects.is_empty() && !r.document.session.can_undo()
                    );
                });
                match interruption {
                    0 => REGISTRY.with(|slot| {
                        assert!(
                            slot.borrow_mut()
                                .as_mut()
                                .unwrap()
                                .set_mode(InteractionMode::Tool(2))
                        );
                    }),
                    1 => escape(),
                    2 => canvas_pointer(0, 3, 180., 180.),
                    3 => REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(r.select_tool_variant(
                            if pencil { 1 } else { 0 },
                            if pencil { 4 } else { 1 }
                        ));
                    }),
                    _ => REGISTRY.with(|slot| {
                        slot.borrow_mut().as_mut().unwrap().close();
                    }),
                }
                // A release from the old capture may arrive after the toolbar action.
                canvas_pointer(0, 2, 240., 240.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.gesture().is_none() && r.text_handle().is_none());
                    assert!(r.geometry_drag().is_none());
                    assert!(
                        gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                            && r.pending_drawing.is_none()
                    );
                    assert!(
                        r.document.session.objects.is_empty() && !r.document.session.can_undo()
                    );
                    r.close();
                });
            }
        }
    }
    // Keyboard completion or removal of the last node retires a pending point press.
    for dpi in [1., 1.25] {
        for complete in [false, true] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.document.mode = InteractionMode::Tool(2);
                r.sync_edit_scope();
                r.document.selected[2] = 6;
                r.sync_edit_scope();
                r.canvas_state.annotation_ready = true;
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
            });
            canvas_pointer(0, 0, 40., 40.);
            canvas_pointer(0, 2, 40., 40.);
            canvas_pointer(0, 1, 100., 100.);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(
                    gestures::display_draft(&r.gesture, &r.pending_drawing)
                        .unwrap()
                        .points
                        .len(),
                    2
                );
                assert!(r.hovered.is_none());
                r.render_state.frame_tasks.cancel_render();
            });
            canvas_pointer(0, 4, 100., 100.);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(
                    gestures::display_draft(&r.gesture, &r.pending_drawing)
                        .unwrap()
                        .points
                        .len(),
                    1
                );
                assert!(
                    r.render_state.frame_tasks.render_pending(),
                    "exit must refresh confirmed nodes"
                );
                assert!(r.document.session.objects.is_empty() && !r.document.session.can_undo());
                r.render_state.frame_tasks.cancel_render();
            });
            canvas_pointer(0, 4, 100., 100.);
            REGISTRY.with(|slot| {
                assert!(
                    !slot
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .render_state
                        .frame_tasks
                        .render_pending()
                );
            });
            if complete {
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 2, 80., 80.);
            }
            canvas_pointer(0, 0, 140., 120.);
            if complete {
                finish_polyline();
            } else {
                backspace_polyline();
            }
            canvas_pointer(0, 2, 140., 120.);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(
                    r.gesture().is_none()
                        && r.polyline().is_none()
                        && gestures::display_draft(&r.gesture, &r.pending_drawing).is_none(),
                    "late release must not start another polyline"
                );
                assert!(r.drawing_press().is_none());
                if complete {
                    assert_eq!(r.document.session.objects.len(), 1);
                    assert_eq!(r.document.session.objects[0].points.len(), 2);
                    assert!(r.document.session.undo());
                }
                assert!(r.document.session.objects.is_empty() && !r.document.session.can_undo());
                r.close();
            });
        }
    }
    shutdown();
}
#[test]
fn sticker_only_draws_without_capture_workers_or_repeated_static_submissions() {
    use lexift_core::ports::screen_capture::{
        ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
    };
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    struct Capture(Arc<AtomicUsize>, Arc<AtomicBool>);
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            self.1.load(Ordering::Relaxed)
        }
        fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<Arc<ScreenCaptureFrame>>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
        fn clear(&self) {}
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let captures = Arc::new(AtomicUsize::new(0));
    let available = Arc::new(AtomicBool::new(false));
    let presents = Rc::new(Cell::new(0));
    let calls = presents.clone();
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
        .with_annotation_screen_capture(Some(Arc::new(Capture(
            captures.clone(),
            available.clone(),
        ))))
        .with_annotation_canvas(
            Vec::new,
            |_, _| true,
            move |_, _, _, _| {
                calls.set(calls.get() + 1);
                true
            },
        ),
    );
    for dpi in [1., 1.25] {
        available.store(dpi == 1.25, Ordering::Relaxed);
        for shape in [0, 1] {
            for texture in [0, 1] {
                let generation = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close();
                    r.canvas_state.pending_displays = vec![Rect {
                        left: -200,
                        top: -200,
                        right: 400,
                        bottom: 400,
                    }];
                    r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
                    create_canvas(r, 0);
                    r.canvas_state.canvases[0]
                        .window
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    r.canvas_state.annotation_ready = true;
                    r.document.selected[5] = 12;
                    r.set_mode(InteractionMode::Tool(5));
                    r.edit_values(12).shape = shape;
                    r.edit_values(12).mode = texture;
                    r.generation
                });
                open_panel(5, false);
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 1, 180., 170.);
                canvas_pointer(0, 2, 180., 170.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.document.session.objects.len(), 1);
                    assert_eq!(
                        r.document.session.objects[0].kind,
                        if shape == 0 {
                            Kind::StickerArea
                        } else {
                            Kind::StickerBrush
                        }
                    );
                    assert!(
                        matches!(&r.document.session.objects[0].extra, Extra::Sticker(s) if s.opacity == 95)
                    );
                    assert_eq!(r.document.session.selected, (shape == 0).then_some(0));
                    if shape == 1 {
                        assert!(r.document.session.objects[0].edit_handles(dpi).is_empty());
                        assert_eq!(r.parameter_object(12), None);
                    }
                    assert_eq!(r.values(12).strength, 95.);
                    assert_eq!(
                        r.toolbar_state.panel.get().unwrap().get_values().strength,
                        95.
                    );
                    assert!(r.render_state.mosaic.is_idle());
                    assert!(!r.render_state.native_presenter.active);
                    assert_eq!(r.toolbar_state.panel.get().unwrap().get_tool(), 12);
                });
                render_canvases(generation);
                let count = presents.get();
                for _ in 0..3 {
                    render_canvases(generation);
                }
                assert_eq!(
                    presents.get(),
                    count,
                    "static sticker must reuse presented pixels"
                );
                assert_eq!(captures.load(Ordering::Relaxed), 0);
                if shape == 1 {
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        let before = r.document.session.objects[0].clone();
                        let mut stale = r.parameter_target(12);
                        stale.object = Some(before.id);
                        let mut values = r.values(12).clone();
                        values.strength = 80.;
                        assert!(apply_parameter_values(r, stale, values.clone()).is_none());
                        assert_eq!(r.values(12).strength, 95.);
                        let target = r.parameter_target(12);
                        assert!(apply_parameter_values(r, target, values).is_some());
                        r.finish_style_edit();
                        assert_eq!(r.document.session.objects[0], before);
                        assert_eq!(r.values(12).strength, 80.);
                    });
                    // The old trajectory is not an edit target: drawing over it creates a new mark.
                    canvas_pointer(0, 0, 100., 100.);
                    canvas_pointer(0, 1, 160., 150.);
                    canvas_pointer(0, 2, 160., 150.);
                    REGISTRY.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let r = slot.as_mut().unwrap();
                            assert_eq!(r.document.session.objects.len(), 2);
                            assert_eq!(r.document.session.selected, None);
                            assert!(matches!(&r.document.session.objects[0].extra, Extra::Sticker(s) if s.opacity == 95));
                            assert!(matches!(&r.document.session.objects[1].extra, Extra::Sticker(s) if s.opacity == 80));
                            assert!(r.document.session.undo());
                            assert_eq!(r.document.session.objects.len(), 1);
                            assert_eq!(r.document.session.selected, None);
                        });
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.document.session.undo());
                    assert!(r.document.session.objects.is_empty());
                });
            }
        }
    }
    shutdown();
}
#[test]
fn creation_captures_displayed_settings_before_deselecting_all_tools() {
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
        for (tool, group, kind) in [
            (0, 0, Kind::Rectangle),
            (1, 0, Kind::Ellipse),
            (2, 0, Kind::SpotlightRectangle),
            (4, 1, Kind::HighlightLine),
            (5, 2, Kind::Arrow),
            (6, 2, Kind::Polyline),
            (7, 2, Kind::Magnifier),
            (8, 3, Kind::Text),
            (10, 4, Kind::Sequence),
            (11, 5, Kind::MosaicArea),
        ] {
            let expected = REGISTRY.with(|slot| {
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
                r.document.selected[group] = tool;
                r.set_mode(InteractionMode::Tool(group));
                let object = Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind,
                    bounds: Bounds::from_corners((-450., -250.), (-400., -200.)),
                    points: vec![],
                    style: Style::default(),
                    extra: Extra::None,
                };
                r.document.session.add(object);
                project_object_parameters(r, 0);
                // Refill is independent; it is only adopted when creation begins.
                let mut values = r.values(tool as usize).clone();
                values.color_index = 8;
                values.custom_color = "#123456".into();
                values.size = 19;
                values.shape = 1;
                values.style = 1;
                values.line_start = 2;
                values.line_end = 3;
                values.zoom = 3;
                values.connector_style = 2;
                values.text_size = 35;
                values.format = 2;
                values.mode = 1;
                values.strength = 73.;
                *r.document.parameter_values.edit(
                    tool as usize,
                    r.document.session.objects.first().map(|o| o.id),
                ) = values.clone();
                values.text = Default::default();
                values.sequence_label = Default::default();
                if tool == 10 {
                    values.start = r.document.session.next_sequence;
                }
                values
            });
            canvas_pointer(0, 0, 700. / dpi, 450. / dpi);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                let mut actual = r.document.parameter_values.defaults(tool as usize).clone();
                if tool == 10 {
                    actual.start = expected.start;
                    actual.sequence_label = Default::default();
                }
                assert_eq!(actual, expected, "tool {tool} DPI {dpi}");
                if tool != 10 {
                    assert_eq!(r.document.session.selected, None);
                }
                if tool != 10 {
                    assert_eq!(r.values(tool as usize), &expected);
                }
                assert_eq!(r.document.session.objects[0].style, Style::default());
                cancel_active_gesture(r);
                let mut actual = r.document.parameter_values.defaults(tool as usize).clone();
                if tool == 10 {
                    actual.start = expected.start;
                    actual.sequence_label = Default::default();
                }
                assert_eq!(actual, expected);
            });
        }
    }
    shutdown();
}
#[test]
fn software_capture_excludes_annotation_ui_and_reissues_for_replaced_canvas() {
    use lexift_core::ports::{magnifier::WindowToken, screen_capture::*};
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::{Arc, Mutex};
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    struct Capture(Arc<Mutex<Vec<ScreenCaptureRequest>>>);
    impl ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(&self, request: ScreenCaptureRequest) -> lexift_core::Result<()> {
            self.0.lock().unwrap().push(request);
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<Arc<ScreenCaptureFrame>>> {
            Ok(None)
        }
        fn clear(&self) {}
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
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
        .with_annotation_screen_capture(Some(Arc::new(Capture(requests.clone()))))
        .with_annotation_window_token(|window| {
            Some(WindowToken(window as *const slint::Window as isize))
        }),
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let bounds = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        r.canvas_state
            .canvases
            .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.toolbar_state.panel.open(AnnotationPanel::new().unwrap());
        r.toolbar_state
            .watermark
            .open(AnnotationWatermarkInput::new().unwrap());
        r.document.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((-80., -80.), (80., 80.)),
            points: Vec::new(),
            style: Style::default(),
            extra: mosaic::settings(r.values(11), 1.),
        });
        mosaic::sync(r);
        mosaic::sync(r);
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "unchanged capture policy must not reissue"
        );
        let old = r.canvas_state.canvases[0].window.clone_strong();
        let old_token = (r.lifecycle.annotation_window_token)(old.window()).unwrap();
        let expected = capture_exclusions(r);
        assert_eq!(expected.len(), 4);
        assert_eq!(requests.lock().unwrap()[0].excluded_windows, expected);
        assert!(
            requests.lock().unwrap()[0].paused_regions.is_empty(),
            "tool windows must be excluded without freezing mosaic edits"
        );
        r.toolbar_state
            .panel
            .get()
            .unwrap()
            .window()
            .set_position(slint::PhysicalPosition::new(-50, -50));
        mosaic::sync(r);
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "moving an excluded toolbar must not pause capture"
        );
        r.canvas_state.canvases[0] = CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds);
        mosaic::sync(r);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].regions, requests[1].regions);
        assert_eq!(requests[0].paused_regions, requests[1].paused_regions);
        assert!(!requests[1].excluded_windows.contains(&old_token));
        assert_eq!(requests[1].excluded_windows, capture_exclusions(r));
        assert!(requests[1].revision > requests[0].revision);
    });
    shutdown();
}
#[test]
fn ready_canvas_prepares_empty_native_scene_before_drawing_and_retires_it() {
    use lexift_core::ports::annotation_render::{
        AnnotationRenderPort, AnnotationScene, RenderBackend, RenderCapabilities, RenderCompletion,
    };
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::{Arc, Mutex};
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    #[derive(Default)]
    struct Port(Mutex<Option<Arc<AnnotationScene>>>);
    impl AnnotationRenderPort for Port {
        fn capabilities(&self) -> RenderCapabilities {
            RenderCapabilities {
                backend: RenderBackend::Gpu,
                max_refresh_hz: 60,
            }
        }
        fn submit(&self, scene: Arc<AnnotationScene>) -> lexift_core::Result<()> {
            *self.0.lock().unwrap() = Some(scene);
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<RenderCompletion>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .as_ref()
                .map(|scene| RenderCompletion {
                    stamp: scene.stamp,
                    sampled_regions: Arc::from([]),
                    presented: true,
                    wait: None,
                }))
        }
        fn clear(&self) {
            *self.0.lock().unwrap() = None;
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let port = Arc::new(Port::default());
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
        .with_annotation_canvas(Vec::new, |_, _| true, |_, _, _, _| true)
        .with_annotation_window_token(|_| Some(lexift_core::ports::magnifier::WindowToken(1)))
        .with_annotation_renderer(Some(port.clone())),
    );
    let (generation, revision) = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        let mut canvas = CanvasLayer::new(
            AnnotationCanvas::new().unwrap(),
            Rect {
                left: -200,
                top: -100,
                right: 200,
                bottom: 100,
            },
        );
        canvas.bootstrap = CanvasBootstrap::Ready;
        r.canvas_state.canvases.push(canvas);
        (r.generation, r.toolbar_state.main.revision())
    });
    show_main(generation, revision, 0);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.canvas_state.annotation_ready && r.render_state.frame_tasks.render_pending());
        assert!(r.document.session.objects.is_empty() && !r.document.session.can_undo());
        assert!(r.render_state.mosaic.is_idle());
    });
    render_canvases(generation);
    assert!(port.0.lock().unwrap().as_ref().unwrap().layers.is_empty());
    poll_native_renderer(generation);
    render_canvases(generation);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(
            r.render_state.native_presenter.active && r.canvas_state.canvases[0].gpu_input_cleared
        );
        assert!(r.set_mode(InteractionMode::Tool(0)));
    });
    canvas_pointer(0, 0, 50., 50.);
    canvas_pointer(0, 1, 100., 100.);
    render_canvases(generation);
    assert!(!port.0.lock().unwrap().as_ref().unwrap().layers.is_empty());
    canvas_pointer(0, 3, 100., 100.);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(r.document.session.objects.is_empty() && !r.document.session.can_undo());
        r.close();
        assert!(!r.render_state.frame_tasks.render_pending());
    });
    assert!(port.0.lock().unwrap().is_none());
    render_canvases(generation); // An old warmup cannot rebuild a closed session.
    assert!(port.0.lock().unwrap().is_none());
    shutdown();
}
#[test]
fn native_completion_hands_off_canvas_and_failure_restores_software_pixels() {
    use lexift_core::ports::annotation_render::{
        AnnotationRenderPort, AnnotationScene, RenderBackend, RenderCapabilities, RenderCompletion,
    };
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    #[derive(Default)]
    struct Port {
        scenes: Mutex<Vec<Arc<AnnotationScene>>>,
        completion: Mutex<Option<RenderCompletion>>,
        failed: AtomicBool,
        cleared: AtomicUsize,
    }
    impl AnnotationRenderPort for Port {
        fn capabilities(&self) -> RenderCapabilities {
            RenderCapabilities {
                backend: RenderBackend::Gpu,
                max_refresh_hz: 60,
            }
        }
        fn submit(&self, scene: Arc<AnnotationScene>) -> lexift_core::Result<()> {
            self.scenes.lock().unwrap().push(scene);
            Ok(())
        }
        fn latest(&self) -> lexift_core::Result<Option<RenderCompletion>> {
            if self.failed.load(Ordering::Relaxed) {
                return Err(lexift_core::Error::new("device removed"));
            }
            Ok(self.completion.lock().unwrap().clone())
        }
        fn clear(&self) {
            self.cleared.fetch_add(1, Ordering::Relaxed);
            *self.completion.lock().unwrap() = None;
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let port = Arc::new(Port::default());
    let frames = Rc::new(RefCell::new(Vec::new()));
    let received = frames.clone();
    let reject_second = Rc::new(std::cell::Cell::new(false));
    let reject = reject_second.clone();
    let second_window = Rc::new(RefCell::new(None::<slint::Weak<AnnotationCanvas>>));
    let rejected_window = second_window.clone();
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
        .with_annotation_canvas(
            Vec::new,
            |_, _| true,
            move |window, pixels, _, _| {
                if reject.get()
                    && rejected_window
                        .borrow()
                        .as_ref()
                        .and_then(slint::Weak::upgrade)
                        .is_some_and(|target| std::ptr::eq(window, target.window()))
                {
                    return false;
                }
                received.borrow_mut().push(pixels.to_vec());
                true
            },
        )
        .with_annotation_window_token(|_| Some(lexift_core::ports::magnifier::WindowToken(1)))
        .with_annotation_renderer(Some(port.clone())),
    );
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.canvas_state.canvases.push(CanvasLayer::new(
            AnnotationCanvas::new().unwrap(),
            Rect {
                left: -32,
                top: -32,
                right: 32,
                bottom: 32,
            },
        ));
        let second = AnnotationCanvas::new().unwrap();
        *second_window.borrow_mut() = Some(second.as_weak());
        second
            .window()
            .set_position(slint::PhysicalPosition::new(32, -32));
        r.canvas_state.canvases.push(CanvasLayer::new(
            second,
            Rect {
                left: 32,
                top: -32,
                right: 96,
                bottom: 32,
            },
        ));
        r.document.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-20., -20.), (80., 20.)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::None,
        });
        r.generation
    });
    render_canvases(generation);
    assert_eq!(frames.borrow().len(), 2);
    assert!(
        frames.borrow()[0]
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 0)
    );
    let stamp = port.scenes.lock().unwrap().last().unwrap().stamp;
    *port.completion.lock().unwrap() = Some(RenderCompletion {
        stamp,
        sampled_regions: Arc::from([]),
        presented: true,
        wait: None,
    });
    poll_native_renderer(generation);
    render_canvases(generation);
    assert_eq!(frames.borrow().len(), 4);
    assert!(
        frames.borrow()[2..]
            .iter()
            .all(|frame| frame.iter().all(|byte| *byte == 0))
    );
    render_canvases(generation);
    assert_eq!(
        frames.borrow().len(),
        4,
        "transparent handoff is not repeated"
    );
    reject_second.set(true);
    port.failed.store(true, Ordering::Relaxed);
    poll_native_renderer(generation);
    assert_eq!(port.cleared.load(Ordering::Relaxed), 0);
    render_canvases(generation);
    assert_eq!(frames.borrow().len(), 5);
    assert_eq!(
        port.cleared.load(Ordering::Relaxed),
        0,
        "first monitor success cannot remove the second monitor's native pixels"
    );
    reject_second.set(false);
    render_canvases(generation);
    assert_eq!(port.cleared.load(Ordering::Relaxed), 1);
    assert_eq!(frames.borrow().len(), 6);
    assert_eq!(frames.borrow()[0], frames.borrow()[4]);
    assert_eq!(frames.borrow()[1], frames.borrow()[5]);
    // Re-enable the adapter to exercise the complete effects route through
    // real UI compilation. The source mock records any CPU readback request.
    struct Capture(AtomicUsize);
    impl lexift_core::ports::screen_capture::ScreenCapturePort for Capture {
        fn is_available(&self) -> bool {
            true
        }
        fn request(
            &self,
            _: lexift_core::ports::screen_capture::ScreenCaptureRequest,
        ) -> lexift_core::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn latest(
            &self,
        ) -> lexift_core::Result<Option<Arc<lexift_core::ports::screen_capture::ScreenCaptureFrame>>>
        {
            Ok(None)
        }
        fn clear(&self) {}
    }
    let capture = Arc::new(Capture(AtomicUsize::new(0)));
    port.failed.store(false, Ordering::Relaxed);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.render_state
            .native_presenter
            .close(r.lifecycle.annotation_renderer.as_ref());
        r.lifecycle.annotation_screen_capture = Some(capture.clone());
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.toolbar_state.panel.open(AnnotationPanel::new().unwrap());
        r.render_state.mosaic.stage(
            &mut r.document.session,
            Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-16., -16.), (16., 16.)),
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::Mosaic(lexift_core::domain::annotation::MosaicAnnotation::new(
                    lexift_core::domain::annotation::MosaicEffect::Blur,
                    50,
                    12.,
                    1.,
                )),
            },
        );
        r.document.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Magnifier,
            bounds: Bounds::from_corners((-16., -16.), (16., 16.)),
            points: Vec::new(),
            style: Style::default(),
            extra: Extra::Magnifier {
                output: Bounds::from_corners((-16., -16.), (16., 16.)),
                zoom: 2.,
                ellipse: true,
                connector: MagnifierConnector::None,
                erase_annotations: false,
                antialias: true,
                shadow: false,
            },
        });
    });
    render_canvases(generation);
    let scene = port.scenes.lock().unwrap().last().unwrap().clone();
    assert!(
        scene.paused_regions.is_empty(),
        "native mosaic edits must not freeze when toolbar sampling bounds overlap"
    );
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(&*scene.excluded_windows, capture_exclusions(r));
    });
    assert!(scene.layers.iter().any(|layer| matches!(
        layer,
        lexift_core::ports::annotation_render::RenderLayer::Mosaic(_)
    )));
    assert!(scene.layers.iter().any(|layer| matches!(
        layer,
        lexift_core::ports::annotation_render::RenderLayer::Magnifier(_)
    )));
    assert_eq!(capture.0.load(Ordering::Relaxed), 0);
    *port.completion.lock().unwrap() = Some(RenderCompletion {
        stamp: scene.stamp,
        sampled_regions: Arc::from([]),
        presented: true,
        wait: None,
    });
    poll_native_renderer(generation);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(
            !r.render_state.native_presenter.active && !r.render_state.mosaic.pending.is_empty()
        );
    });
    let coverage: Vec<_> = scene
        .layers
        .iter()
        .flat_map(|layer| match layer {
            lexift_core::ports::annotation_render::RenderLayer::Mosaic(layer) => layer
                .masks
                .iter()
                .map(|mask| mask.bounds)
                .collect::<Vec<_>>(),
            lexift_core::ports::annotation_render::RenderLayer::Magnifier(layer) => {
                layer.masks.iter().map(|mask| mask.bounds).collect()
            }
            _ => Vec::new(),
        })
        .collect();
    port.completion
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .sampled_regions = coverage.into();
    poll_native_renderer(generation);
    render_canvases(generation);
    assert_eq!(
        capture.0.load(Ordering::Relaxed),
        0,
        "native effects must not request CPU screen frames"
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(
            r.render_state.native_presenter.active && r.canvas_state.canvases[0].gpu_input_cleared
        );
        assert!(r.render_state.mosaic.pending.is_empty());
        r.document.session.erase(
            lexift_core::domain::annotation_erase::EraseRegion::Rectangle(Bounds::from_corners(
                (10., 10.),
                (20., 20.),
            )),
            &[lexift_core::domain::annotation_erase::EraseTarget::Object(
                r.document.session.objects[0].id,
            )],
        );
    });
    let clears = port.cleared.load(Ordering::Relaxed);
    undo();
    assert_eq!(port.cleared.load(Ordering::Relaxed), clears + 1);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(!eraser::has_coverage(&r.document.session));
        assert!(!r.render_state.native_presenter.active);
        assert!(
            r.canvas_state
                .canvases
                .iter()
                .all(|layer| !layer.gpu_input_cleared)
        );
        r.close();
        slot.take();
    });
}
#[test]
fn mosaic_refresh_submits_only_changed_canvas_pixels_and_retries_failure() {
    use crate::annotation_render::mosaic_worker::{Job, Worker};
    use lexift_core::{
        domain::annotation::{MosaicAnnotation, MosaicEffect},
        ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
    };
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::Arc;
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let submissions = Rc::new(RefCell::new([0usize; 2]));
    let failed_once = Rc::new(Cell::new(false));
    let calls = submissions.clone();
    let failure = failed_once.clone();
    let window_ids = RefCell::new(Vec::new());
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
        .with_annotation_canvas(
            Vec::new,
            |_, _| true,
            move |window, _, _, _| {
                // The software window adapter has no native desktop position.
                let id = window as *const slint::Window as usize;
                let mut ids = window_ids.borrow_mut();
                let index = ids
                    .iter()
                    .position(|known| *known == id)
                    .unwrap_or_else(|| {
                        ids.push(id);
                        ids.len() - 1
                    });
                calls.borrow_mut()[index] += 1;
                !failure.replace(false)
            },
        ),
    );
    let monitors = [
        Rect {
            left: -64,
            top: 0,
            right: 0,
            bottom: 64,
        },
        Rect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        },
    ];
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.canvas_state.pending_displays = monitors.to_vec();
        r.document.session.displays = monitors.to_vec();
        for (index, monitor) in monitors.iter().enumerate() {
            create_canvas(r, index);
            r.canvas_state.canvases[index]
                .window
                .window()
                .set_position(slint::PhysicalPosition::new(monitor.left, 0));
            render_initial_canvas(r, index);
        }
        r.document.session.add(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicArea,
            bounds: Bounds::from_corners((8., 8.), (24., 24.)),
            points: vec![],
            style: Style::default(),
            extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 50, 12., 1.)),
        });
        r.document.session.selected = None;
        r.generation
    });
    assert_eq!(*submissions.borrow(), [1, 1]);
    let mut worker = Worker::default();
    let mut publish = |serial, color: [u8; 4]| {
        let session = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let source = &slot.as_ref().unwrap().document.session;
            let mut snapshot = Session::new();
            snapshot.objects = source.objects.clone();
            snapshot.displays = source.displays.clone();
            snapshot
        });
        worker.submit(Job {
            key: serial,
            context: 1,
            session: Arc::new(session),
            provisional_index: None,
            monitors: monitors.iter().map(|m| (*m, 1.)).collect(),
            capture: Arc::new(ScreenCaptureFrame {
                revision: 1,
                serial,
                regions: vec![CapturedRegion {
                    bounds: Rect {
                        left: -80,
                        top: -80,
                        right: 144,
                        bottom: 144,
                    },
                    pixels: color.repeat(224 * 224),
                }],
            }),
        });
        let started = Instant::now();
        let frame = loop {
            if let Some(frame) = worker.latest(1).filter(|f| f.serial == serial) {
                break frame;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "effect worker timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .render_state
                .mosaic
                .effects = Some(frame)
        });
        render_canvases(generation);
    };
    publish(1, [20, 100, 180, 255]);
    let raster_signatures = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .canvas_state
            .canvases
            .iter()
            .map(|c| c.signature)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        *submissions.borrow(),
        [1, 2],
        "blank monitor must retain its initial frame"
    );
    publish(2, [20, 100, 180, 255]);
    REGISTRY.with(|slot| {
        assert_eq!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .canvas_state
                .canvases
                .iter()
                .map(|c| c.signature)
                .collect::<Vec<_>>(),
            raster_signatures,
            "same pixels with a new serial must retain both raster signatures"
        )
    });
    assert_eq!(
        *submissions.borrow(),
        [1, 2],
        "new effect serial alone must not submit identical pixels"
    );
    failed_once.set(true);
    publish(3, [190, 30, 60, 255]);
    assert_eq!(*submissions.borrow(), [1, 3]);
    render_canvases(generation);
    assert_eq!(
        *submissions.borrow(),
        [1, 4],
        "failed presentation must retry the same render signature"
    );
    render_canvases(generation);
    assert_eq!(*submissions.borrow(), [1, 4]);
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .document
            .session
            .selected = Some(0)
    });
    render_canvases(generation);
    assert_eq!(
        *submissions.borrow(),
        [1, 5],
        "selection handles must still update"
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.document.session.objects.clear();
        r.document.session.selected = None;
        r.render_state.mosaic.effects = None;
    });
    render_canvases(generation);
    assert_eq!(
        *submissions.borrow(),
        [1, 6],
        "removing an effect must clear its old pixels"
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.as_mut().unwrap().close();
        slot.take();
    });
}
#[test]
fn magnifier_frame_draft_never_creates_a_native_view() {
    let object = Object {
        erased: Vec::new(),
        id: Default::default(),
        bounds: Bounds::from_corners((120., 80.), (220., 160.)),
        kind: Kind::Magnifier,
        style: Style::default(),
        points: vec![],
        extra: Extra::Magnifier {
            output: Bounds::from_corners((260., 80.), (410., 200.)),
            zoom: 1.5,
            ellipse: false,
            connector: MagnifierConnector::Plain,
            erase_annotations: true,
            antialias: true,
            shadow: false,
        },
    };
    let mut session = Session::new();
    assert!(session.add(object.clone()));
    let output = Bounds::from_corners((260., 80.), (410., 200.));
    let draft = Object {
        erased: Vec::new(),
        id: Default::default(),
        bounds: output,
        kind: Kind::Rectangle,
        style: Style::default(),
        points: vec![],
        extra: Extra::None,
    };
    let preview = magnifier_views(&session, Some(&draft));
    assert_eq!(
        preview
            .iter()
            .map(|view| (view.id, view.preview))
            .collect::<Vec<_>>(),
        vec![(0, false)]
    );
    assert!(session.add(object));
    let committed = magnifier_views(&session, None);
    assert_eq!(committed[1].id, 1);
    assert!(!committed[1].preview);
    assert!(session.undo());
    assert_eq!(magnifier_views(&session, None).len(), 1);
}
#[test]
fn magnifier_commit_centers_output_and_rejects_tiny_sample() {
    let values = AnnotationValues {
        zoom: 150,
        ..defaults(7)
    };
    let source = Bounds::from_corners((-180., -120.), (-30., -30.));
    let object = magnifier_from_source(source, Style::default(), &values).unwrap();
    assert!(
        values.include_annotations,
        "new sessions include annotations by default"
    );
    assert!(matches!(
        object.extra,
        Extra::Magnifier {
            erase_annotations: false,
            ..
        }
    ));
    let mut state = parameters::State::new();
    let mut session = Session::new();
    session.add(object.clone());
    let id = session.objects[0].id;
    for include in [false, true] {
        let mut edited = values.clone();
        edited.include_annotations = include;
        state.replace(7, Some(id), edited.clone());
        state.deselect();
        let next = magnifier_from_source(source, Style::default(), state.values(7, None)).unwrap();
        assert!(
            matches!(next.extra, Extra::Magnifier { erase_annotations, .. } if erase_annotations == !include)
        );
        // Re-editing an existing object must project its own value, not the default.
        state.project(&next, 0, 1, 0.5);
        assert_eq!(state.values(7, Some(next.id)).include_annotations, include);
        let before = AnnotationValues {
            include_annotations: !include,
            ..values.clone()
        };
        let mut applied = before.clone();
        parameters::Patch::between(before, edited)
            .unwrap()
            .apply(&mut applied);
        assert_eq!(applied.include_annotations, include);
        assert_eq!(
            session.objects[0].extra, object.extra,
            "default changes must not mutate existing objects"
        );
    }
    assert!(!parameters::defaults(2).erase);
    assert!(parameters::defaults(9).erase);
    assert_eq!(object.bounds, source);
    assert_eq!(object.bounds.handles()[7], (-30., -30.));
    assert!(
        matches!(object.extra, Extra::Magnifier { output, .. } if output == Bounds::from_corners((-217.5, -142.5), (7.5, -7.5)))
    );
    assert!(
        magnifier_from_source(
            Bounds::from_corners((0., 0.), (1., 1.)),
            Style::default(),
            &values,
        )
        .is_none()
    );
}
#[test]
fn deferred_magnifier_sync_ignores_cancelled_and_replaced_gestures() {
    let mut state = MagnifierSync::Cold;
    let mut revision = 0;
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, true),
        MagnifierFrameAction::Defer(1)
    );
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, true),
        MagnifierFrameAction::Wait
    );
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, false),
        MagnifierFrameAction::Clear
    );
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, true),
        MagnifierFrameAction::Defer(3)
    );
    assert!(!deferred_sync_is_current(7, state, 7, 1));
    state = MagnifierSync::Live;
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, true),
        MagnifierFrameAction::Sync
    );
    state = MagnifierSync::Failed;
    assert_eq!(
        magnifier_frame_action(&mut state, &mut revision, true),
        MagnifierFrameAction::Wait
    );
    assert!(deferred_sync_is_current(7, MagnifierSync::Pending(3), 7, 3));
    assert!(!deferred_sync_is_current(7, MagnifierSync::Cold, 7, 3));
    assert!(!deferred_sync_is_current(
        7,
        MagnifierSync::Pending(4),
        7,
        3
    ));
    assert!(!deferred_sync_is_current(
        8,
        MagnifierSync::Pending(3),
        7,
        3
    ));
}
