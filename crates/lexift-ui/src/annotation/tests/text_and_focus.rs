use super::*;
#[test]
fn caret_blinks_resets_and_stays_visible_during_composition_or_selection() {
    let mut caret = super::CaretBlink { visible: true };
    assert!(caret.tick(false));
    assert!(!caret.visible);
    assert!(caret.tick(true));
    assert!(caret.visible);
    assert!(!caret.tick(true));
    assert!(caret.tick(false));
    caret.reset();
    assert!(caret.visible);
}
#[test]
fn new_sequence_adopts_displayed_settings_and_cancel_restores_selected_projection() {
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
        r.mode = InteractionMode::Tool(4);
        r.sync_edit_scope();
        let mut existing = crate::annotation_sequence::tests::badge(1.);
        existing.style.color = [1, 2, 3];
        if let Extra::Sequence(sequence) = &mut existing.extra {
            sequence.size = 90.;
        }
        crate::annotation_sequence::layout(&mut existing);
        assert!(r.session.add(existing));
        let old_id = r.session.objects[0].id;
        let old_next = r.session.next_sequence;
        project_object_parameters(r, 0);
        assert_eq!(r.values(10).text_size, 90);
        let placed = sequence_from_values(r, (100., 100.), 1.25);
        let Extra::Sequence(sequence) = &placed.extra else {
            panic!()
        };
        assert_eq!(
            sequence.size,
            r.parameter_values.defaults(10).text_size as f32
        );
        assert_ne!(placed.style.color, [1, 2, 3]);
        begin_sequence_placement(r, (100., 100.), 1.25);
        assert!(cancel_sequence_placement(r));
        assert_eq!(
            r.session.selected.map(|index| r.session.objects[index].id),
            Some(old_id)
        );
        assert_eq!(r.session.next_sequence, old_next);
        assert_eq!(r.values(10).text_size, 90);
        assert_eq!(r.parameter_values.defaults(10).text_size, 90);
        assert_eq!(r.mode, InteractionMode::Tool(4));
    });
    REGISTRY.with(|slot| {
        slot.borrow_mut().take();
    });
}
#[test]
fn sequence_press_drag_release_and_cancellation_do_not_create_hover_frames() {
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
            r.mode = InteractionMode::Tool(4);
            r.sync_edit_scope();
        });
        for x in 40..100 {
            canvas_pointer(0, 1, x as f32, 80.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(
                    gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                        && r.session.objects.is_empty()
                );
                assert!(!r.render_state.frame_tasks.render_pending());
                assert_eq!(
                    r.canvas_state.canvases[0].window.get_cursor(),
                    crate::AnnotationCursor::Drawing
                );
            });
        }
        canvas_pointer(0, 0, 80., 80.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.objects.len(), 1);
            assert_eq!(
                r.session.objects[0].bounds.center(),
                (-500. + 80. * dpi, -300. + 80. * dpi)
            );
            assert!(r.sequence_placement().is_some() && r.text_handle().is_none());
            assert_eq!(r.session.next_sequence, 2);
            assert!(r.session.selected.is_none() && !r.session.can_undo());
        });
        canvas_pointer(0, 1, 280., 200.);
        canvas_pointer(0, 2, 300., 210.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(
                r.session.objects[0].bounds.center(),
                (-500. + 300. * dpi, -300. + 210. * dpi)
            );
            assert!(r.sequence_placement().is_none() && r.text_handle().is_none());
            assert_eq!(r.session.selected, Some(0));
            assert!(r.session.can_undo());
        });
        undo();
        // A new press retires an interrupted placement instead of joining its snapshot.
        canvas_pointer(0, 0, 80., 80.);
        canvas_pointer(0, 1, 200., 180.);
        canvas_pointer(0, 0, 100., 120.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.objects.len(), 1);
            assert_eq!(r.session.next_sequence, 2);
            assert_eq!(
                r.session.objects[0].bounds.center(),
                (-500. + 100. * dpi, -300. + 120. * dpi)
            );
        });
        canvas_pointer(0, 3, 100., 120.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
            assert!(r.sequence_placement().is_none());
            assert_eq!(r.session.next_sequence, 1);
        });
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
                assert!(r.session.objects.is_empty() && !r.session.can_undo());
                assert_eq!(r.session.next_sequence, 1);
                assert!(r.gesture().is_none() && r.text_handle().is_none());
                assert!(r.geometry_drag().is_none());
                assert!(r.sequence_placement().is_none());
                r.mode = InteractionMode::Tool(4);
                r.sync_edit_scope();
            });
        }
        canvas_pointer(0, 0, 80., 80.);
        canvas_pointer(0, 2, 80., 80.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.objects.len(), 1);
            assert_eq!(r.session.objects[0].kind, Kind::Sequence);
            assert!(r.session.object_index(r.session.objects[0].id).is_some());
        });
        open_text_editor((-100., 0.), Some(0), dpi);
        let (generation, old_id) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let edit = r.editor.edit().unwrap();
            assert!(edit.caret.visible && edit.caret_running());
            assert!(edit.original.content.is_empty());
            (r.generation, edit.id)
        });
        tick_text_caret(generation, old_id);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(!r.editor.edit().unwrap().caret.visible);
            reset_text_caret(r);
            assert!(r.editor.edit().unwrap().caret.visible);
            r.editor.edit_mut().unwrap().selection_anchor = Some(0);
        });
        tick_text_caret(generation, old_id);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.editor.edit().unwrap().caret.visible);
            finish_text_edit_registry(r, false);
        });
        open_text_editor((-100., 0.), Some(0), dpi);
        tick_text_caret(generation, old_id);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_ne!(r.editor.edit().unwrap().id, old_id);
            assert!(r.editor.edit().unwrap().caret.visible);
            finish_text_edit_registry(r, false);
        });
        tick_text_caret(generation, old_id);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.session.objects.len(), 1);
            assert!(r.session.undo());
            assert!(!r.session.can_undo());
            begin_sequence_placement(r, (-100., 0.), dpi);
            assert!(r.sequence_placement().is_some());
            r.close();
            assert!(r.sequence_placement().is_none() && r.gesture().is_none());
        });
    }
    REGISTRY.with(|slot| {
        slot.borrow_mut().take();
    });
}
#[test]
fn sequence_placement_parameters_keep_default_target_after_release_or_cancel() {
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
        for cancel in [false, true] {
            let (original, target, changed) = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.session = Session::new();
                r.parameter_values = parameters::State::new();
                r.mode = InteractionMode::Tool(4);
                r.sync_edit_scope();
                r.canvas_state.annotation_ready = true;
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
                let mut existing = crate::annotation_sequence::tests::badge(dpi);
                crate::annotation_sequence::layout(&mut existing);
                assert!(r.session.add(existing));
                let original = r.session.objects[0].clone();
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(10);
                r.toolbar_state.panel.open(panel);
                restore_parameter_projection(r);
                begin_sequence_placement(r, (-200., -100.), dpi);
                let target = r.parameter_target(10);
                assert!(target.object.is_none());
                let mut changed = r.values(10).clone();
                parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33])
                    .apply(&mut changed);
                changed.start = 20;
                assert!(apply_geometry_values(r, target, changed.clone()).is_none());
                assert_eq!(r.deferred_parameters.len(), 1);
                assert!(!r.parameter_edit.active());
                (original, target, changed)
            });
            if cancel {
                REGISTRY.with(|slot| {
                    assert!(cancel_sequence_placement(
                        slot.borrow_mut().as_mut().unwrap()
                    ));
                });
            } else {
                canvas_pointer(0, 2, 320. / dpi, 220. / dpi);
            }
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.deferred_parameters.is_empty());
                assert_eq!(
                    r.parameter_values.defaults(10).color_index,
                    changed.color_index
                );
                assert_eq!(
                    r.parameter_values.defaults(10).custom_color,
                    changed.custom_color
                );
                assert_eq!(r.session.next_sequence, 20);
                assert_eq!(r.session.objects[0], original);
                if cancel {
                    assert_eq!(r.session.selected, Some(0));
                } else {
                    assert_eq!(r.session.selected, Some(1));
                    assert_ne!(r.session.objects[1].style.color, [11, 22, 33]);
                }
                r.finish_style_edit();
                assert!(
                    r.session.undo(),
                    "number change owns a separate parameter undo"
                );
                assert_eq!(r.session.selected, if cancel { Some(0) } else { None });
                assert_eq!(r.session.next_sequence, if cancel { 2 } else { 3 });
                if !cancel {
                    assert!(r.session.undo(), "creation plus placement owns one undo");
                }
                assert_eq!(r.session.objects, vec![original]);
                assert!(r.session.undo());
                assert!(r.session.objects.is_empty() && !r.session.can_undo());
                // An old target must not mutate a reopened parameter panel.
                r.close_panel();
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(10);
                r.toolbar_state.panel.open(panel);
                let before = r.parameter_values.defaults(10).clone();
                assert!(apply_geometry_values(r, target, changed).is_none());
                assert_eq!(r.parameter_values.defaults(10), &before);
            });
        }
    }
    shutdown();
}
#[test]
fn retired_sequence_panel_discards_waiting_requests_before_reopen() {
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
        r.mode = InteractionMode::Tool(4);
        r.sync_edit_scope();
        let mut original = crate::annotation_sequence::tests::badge(1.25);
        crate::annotation_sequence::layout(&mut original);
        assert!(r.session.add(original));
        let original = r.session.objects[0].clone();
        let panel = AnnotationPanel::new().unwrap();
        panel.set_tool(10);
        r.toolbar_state.panel.open(panel);
        restore_parameter_projection(r);
        begin_sequence_placement(r, (-200., -100.), 1.25);
        let retired_target = r.parameter_target(10);
        let mut pending = r.values(10).clone();
        parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33]).apply(&mut pending);
        pending.start = 20;
        assert!(apply_geometry_values(r, retired_target, pending.clone()).is_none());
        assert_eq!(r.deferred_parameters.len(), 1);
        r.close_panel();
        assert!(r.deferred_parameters.is_empty());
        let reopened = AnnotationPanel::new().unwrap();
        reopened.set_tool(10);
        r.toolbar_state.panel.open(reopened);
        assert!(apply_geometry_values(r, retired_target, pending).is_none());
        assert!(cancel_sequence_placement(r));
        assert_eq!(r.session.objects, vec![original.clone()]);
        assert_eq!(r.session.selected, Some(0));
        assert_eq!(r.session.next_sequence, 2);
        assert_ne!(
            parameters::style(r.parameter_values.defaults(10), 10).color,
            [11, 22, 33]
        );
        assert!(!r.parameter_edit.active());
        assert!(r.session.undo());
        assert!(r.session.objects.is_empty() && !r.session.can_undo());
    });
    shutdown();
}
#[test]
fn applied_watermark_parameters_and_stale_events_do_not_change_defaults() {
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
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.selected[3] = 9;
        r.sync_edit_scope();
        r.generation
    });
    open_panel(3, false);
    let defaults = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .parameter_values
            .defaults(9)
            .clone()
    });
    open_watermark_input(generation);
    let revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    apply_watermark(generation, revision, "甲".into());
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        let panel = r.toolbar_state.panel.get().unwrap();
        let mut values = panel.get_values();
        values.text_size = 44;
        values.color_index = 8;
        values.custom_color = "#123456".into();
        values.strength = 70.;
        values.mode = 7;
        panel.set_values(values.clone());
        panel.invoke_values_changed(values);
    });
    slint::platform::update_timers_and_animations();
    open_watermark_input(generation);
    let cancelled_revision = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        r.toolbar_state
            .watermark
            .get()
            .unwrap()
            .set_value("未应用".into());
        r.toolbar_state.watermark.revision()
    });
    close_watermark_input(generation, cancelled_revision);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.finish_style_edit();
        let watermark = r.session.watermark.as_ref().unwrap();
        assert_eq!(watermark.template, "甲");
        assert_eq!(watermark.font_size, 44.);
        assert_eq!(watermark.color, [18, 52, 86]);
        assert_eq!(watermark.opacity, 70);
        assert_eq!(r.parameter_values.defaults(9), &defaults);
        let mut stale = r.toolbar_state.panel.get().unwrap().get_values();
        stale.text_size = 68;
        r.toolbar_state
            .panel
            .get()
            .unwrap()
            .invoke_values_changed(stale);
    });
    open_watermark_input(generation);
    let revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    apply_watermark(generation, revision, String::new());
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.session.watermark.is_none());
        assert_eq!(r.values(9), &defaults);
        assert_eq!(r.parameter_values.defaults(9), &defaults);
    });
    undo();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.session.watermark.as_ref().unwrap().font_size, 44.);
        assert_eq!(r.values(9).text_size, 44);
        assert_eq!(r.values(9).custom_color, "#123456");
        assert_eq!(r.parameter_values.defaults(9), &defaults);
    });
    shutdown();
}
#[test]
fn new_text_draft_parameters_are_cancelled_or_committed_and_retained_for_subsequent_drawing() {
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
        r.selected[3] = 8;
        r.sync_edit_scope();
    });
    for commit in [false, true] {
        open_panel(3, false);
        open_text_editor((-100., 80.), None, 1.25);
        slint::platform::update_timers_and_animations();
        let _defaults = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let defaults = r.parameter_values.defaults(8).clone();
            let panel = r.toolbar_state.panel.get().unwrap();
            let mut values = panel.get_values();
            values.text_size = 58;
            values.color_index = 8;
            values.custom_color = "#102030".into();
            values.text_background = true;
            values.text_bg_padding = 13;
            panel.set_values(values.clone());
            panel.invoke_values_changed(values);
            r.editor.window().unwrap().set_value("中文草稿".into());
            defaults
        });
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.parameter_values.defaults(8).text_size, 58);
            assert!(r.parameter_values.defaults(8).text.is_empty());
            assert_eq!(r.values(8).text_size, 58);
            assert_eq!(r.editor.edit().unwrap().original.font_size, 58.);
            assert_eq!(current_style(r, 8).color, [16, 32, 48]);
            finish_text_edit_registry(r, commit);
            r.finish_style_edit();
            assert_eq!(r.parameter_values.defaults(8).text_size, 58);
            assert!(r.parameter_values.defaults(8).text.is_empty());
            assert!(r.editor.draft_values().is_none());
            if commit {
                let object = r.session.objects.last().unwrap();
                assert_eq!(object.style.color, [16, 32, 48]);
                let Extra::Text(text) = &object.extra else {
                    unreachable!()
                };
                assert_eq!(text.content, "中文草稿");
                assert_eq!(text.font_size, 58.);
                assert_eq!(text.background.as_ref().unwrap().padding, 13.);
                assert!(r.session.undo());
                assert!(
                    !r.session.can_undo(),
                    "draft parameters added an extra undo"
                );
            } else {
                assert!(!r.session.can_undo());
                assert_eq!(
                    r.toolbar_state.panel.get().unwrap().get_values().text_size,
                    58
                );
            }
            assert!(r.session.objects.is_empty());
            // Closing retains accepted settings without retaining draft contents.
            r.close_panel();
            assert_eq!(r.parameter_values.defaults(8).text_size, 58);
            assert!(r.parameter_values.defaults(8).text.is_empty());
        });
    }
    shutdown();
}
#[test]
fn sequence_panel_parameters_reach_the_session_before_the_panel_closes() {
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
        assert!(r.set_mode(InteractionMode::Tool(4)));
        let mut object = crate::annotation_sequence::tests::badge(1.25);
        crate::annotation_sequence::layout(&mut object);
        assert!(r.session.add(object));
    });
    open_panel(4, false);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let panel = slot.as_ref().unwrap().toolbar_state.panel.get().unwrap();
        let mut v = panel.get_values();
        v.shape = 0;
        v.format = 4;
        v.text_size = 28;
        v.start = 8;
        v.sequence_ratio = 4;
        panel.set_values(v.clone());
        panel.invoke_values_changed(v);
    });
    slint::platform::update_timers_and_animations();
    open_text_editor((-100., 80.), Some(0), 1.25);
    slint::platform::update_timers_and_animations();
    let (menu_generation, menu_panel, original_font) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (
            r.generation,
            r.toolbar_state.panel.revision(),
            r.values(10).font,
        )
    });
    choice::open(
        menu_generation,
        menu_panel,
        crate::AnnotationChoiceRequest {
            field: 12,
            options: ModelRc::new(VecModel::from(vec!["First".into(), "Second".into()])),
            width: 180.,
            height: 34.,
            ..Default::default()
        },
    );
    let retired_menu = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .clone_strong()
    });
    let stale_font = if original_font == 0 { 1 } else { 0 };
    retired_menu.invoke_chosen(stale_font);
    let retired_editor = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let id = r.editor.edit().unwrap().id;
        let mut stale = r.toolbar_state.panel.get().unwrap().get_values();
        stale.text_size = 70;
        stale.format = 1;
        // Queue through the real callback while Registry is borrowed: its
        // fallback must capture this editor, not the eventual replacement.
        r.toolbar_state
            .panel
            .get()
            .unwrap()
            .invoke_values_changed(stale);
        finish_text_edit_registry(r, false);
        id
    });
    open_text_editor((-100., 80.), Some(0), 1.25);
    // Keep the retired component alive and deliver another old callback:
    // neither weak-reference expiry nor only the first event may protect us.
    retired_menu.invoke_chosen(stale_font);
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_ne!(r.editor.edit().unwrap().id, retired_editor);
        assert_eq!(r.values(10).text_size, 28);
        assert_eq!(r.values(10).format, 4);
        assert_eq!(r.values(10).font, original_font);
        assert!(r.toolbar_state.menus.current.is_none());
        assert!(
            r.toolbar_state.panel.get().is_some(),
            "retiring a menu closed the parameter panel"
        );
        assert_eq!(r.editor.edit().unwrap().original.font_size, 52.5);
    });
    let (generation, edit_id, panel_revision, choice_revision) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        let editor = r.editor.window().unwrap();
        editor.set_value("ABC".into());
        editor.invoke_place_selection(1, 2);
        r.toolbar_state
            .panel
            .get()
            .unwrap()
            .set_parameter_input_active(true);
        (
            r.generation,
            r.editor.edit().unwrap().id,
            r.toolbar_state.panel.revision(),
            r.toolbar_state.menus.revision,
        )
    });
    assert!(!restore_matching_text_focus(
        generation,
        edit_id,
        panel_revision,
        choice_revision
    ));
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref()
            .unwrap()
            .toolbar_state
            .panel
            .get()
            .unwrap()
            .set_parameter_input_active(false);
    });
    assert!(!restore_matching_text_focus(
        generation,
        edit_id,
        panel_revision,
        choice_revision.wrapping_sub(1)
    ));
    assert!(!restore_matching_text_focus(
        generation,
        edit_id.wrapping_sub(1),
        panel_revision,
        choice_revision
    ));
    assert!(!restore_matching_text_focus(
        generation,
        edit_id,
        panel_revision.wrapping_sub(1),
        choice_revision
    ));
    open_watermark_input(generation);
    assert!(REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .get()
            .is_some()
    }));
    assert!(
        !restore_matching_text_focus(generation, edit_id, panel_revision, choice_revision),
        "an older text restoration cannot preempt the watermark input"
    );
    REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_watermark_input());
    assert!(restore_matching_text_focus(
        generation,
        edit_id,
        panel_revision,
        choice_revision
    ));
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert_eq!(r.editor.edit().unwrap().id, edit_id);
        assert!(r.editor.edit().unwrap().caret.visible);
        let editor = r.editor.window().unwrap();
        assert_eq!(editor.get_value(), "ABC");
        assert_eq!(
            (editor.get_anchor_offset(), editor.get_cursor_offset()),
            (1, 2)
        );
        editor
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "X".into() });
        assert_eq!(editor.get_value(), "AXC");
        r.apply_menu_edit(
            r.parameter_target(10),
            parameters::Edit::Color(parameters::ColorTarget::SequenceText, [16, 32, 48]),
        );
        finish_text_edit_registry(r, false);
        assert_eq!(r.session.next_sequence, 8);
        let Extra::Sequence(s) = &r.session.objects[0].extra else {
            unreachable!()
        };
        assert_eq!(s.shape, SequenceShape::Circle);
        assert_eq!(s.format, SequenceFormat::Chinese);
        assert_eq!(
            s.caption_color,
            [16, 32, 48],
            "completing editing lost the accepted menu patch"
        );
        assert_eq!(
            lexift_core::domain::annotation::sequence_label(s.value, s.format),
            "一"
        );
        assert_eq!(s.size, 28.);
        assert_eq!(s.caption.font_size, 52.5);
        let preview = sequence_from_values(r, (500., 200.), 1.25);
        let Extra::Sequence(s) = &preview.extra else {
            unreachable!()
        };
        assert_eq!(
            lexift_core::domain::annotation::sequence_label(s.value, s.format),
            "\u{516b}"
        );
        // Accepted settings persist; the counter remains session-owned.
        assert_eq!(s.format, SequenceFormat::Chinese);
        assert_eq!(s.size, 28.);
        assert_eq!(r.parameter_values.defaults(10).format, 4);
        let retired = r.session.objects[0].id;
        assert_eq!(r.parameter_values.binding().get(), Some(retired));
        let generation = r.generation;
        drop(slot);
        delete_selected();
        let stale = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.session.objects.is_empty());
            assert_eq!(r.parameter_values.bound_object(), None);
            assert_eq!(
                r.toolbar_state.panel.get().unwrap().get_values(),
                *r.values(10)
            );
            let mut stale = r.toolbar_state.panel.get().unwrap().get_values();
            stale.format = 4;
            stale.text_size = 70;
            stale
        });
        update_geometry_values(
            generation,
            parameter_edit::Target {
                panel: panel_revision,
                tool: 10,
                object: Some(retired),
                editor: None,
                watermark: 0,
                projection: 0,
            },
            stale,
        );
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert_eq!(r.values(10).format, 4);
        assert_eq!(r.values(10).text_size, 28);
        let mut new_object = sequence_from_values(r, (500., 200.), 1.25);
        let Extra::Sequence(sequence) = &mut new_object.extra else {
            unreachable!()
        };
        sequence.format = SequenceFormat::Roman;
        sequence.size = 41.;
        crate::annotation_sequence::layout(&mut new_object);
        assert!(r.session.add(new_object));
        refresh_parameter_target(r);
        assert_eq!(r.toolbar_state.panel.get().unwrap().get_values().format, 1);
        assert_eq!(
            r.toolbar_state.panel.get().unwrap().get_values().text_size,
            41
        );
        assert_eq!(r.parameter_values.defaults(10).format, 4);
        assert_eq!(r.parameter_values.defaults(10).text_size, 28);
        r.close();
        assert_eq!(r.session.next_sequence, 1);
        assert_eq!(r.values(10).sequence_label, "1");
        assert!(
            r.editor.window().is_none()
                && r.toolbar_state.menus.current.is_none()
                && r.toolbar_state.panel.get().is_none()
        );
        slot.take();
    });
}
#[test]
fn sequence_caption_and_connector_invalidate_another_monitors_frame() {
    let mut session = Session::new();
    let mut object = crate::annotation_sequence::tests::badge(1.);
    crate::annotation_sequence::layout(&mut object);
    session.add(object);
    session.selected = None;
    let monitor = Rect {
        left: 0,
        top: 0,
        right: 500,
        bottom: 300,
    };
    let before = render_signature(&session, monitor, 1., None, None);
    if let Extra::Sequence(s) = &mut session.objects[0].extra {
        s.caption.content = "跨屏更新".into();
    }
    crate::annotation_sequence::layout(&mut session.objects[0]);
    assert_ne!(before, render_signature(&session, monitor, 1., None, None));
    let before = render_signature(&session, monitor, 1., None, None);
    session.objects[0].toggle_text_arrow(1.);
    session.objects[0].edit_handle(5, (300., 150.), 1.);
    assert_ne!(before, render_signature(&session, monitor, 1., None, None));
}
#[test]
fn foreground_dismissal_closes_the_menu_without_activating_the_text_window() {
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Clock(
        Rc<Cell<Duration>>,
        Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
    );
    impl Platform for Clock {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            self.1.borrow_mut().push(window.clone());
            Ok(window)
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    thread_local! { static ACTIVATIONS: Cell<usize> = const { Cell::new(0) }; }
    thread_local! { static PREPARING: Cell<bool> = const { Cell::new(false) }; }

    let clock = Rc::new(Cell::new(Duration::ZERO));
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(Clock(clock.clone(), windows.clone()))).unwrap();
    slint::platform::update_timers_and_animations();
    let advance = || {
        clock.set(clock.get() + Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    };
    let sinks = Rc::new(RefCell::new(Vec::new()));
    let received = sinks.clone();
    init(
        |_| PassiveWindowPreparation::Ready,
        WindowLifecycleCallbacks::new(
            move |_, sink| {
                received.borrow_mut().push(sink);
                true
            },
            |_| {
                ACTIVATIONS.with(|count| count.set(count.get() + 1));
                true
            },
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
        r.selected[3] = 8;
        r.sync_edit_scope();
    });
    open_panel(3, false);
    open_text_editor((-120., -50.), None, 1.25);
    advance();
    let (generation, panel, editor, id) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        let editor = r.editor.window().unwrap().clone_strong();
        editor.set_value("ABC".into());
        editor.invoke_place_selection(1, 2);
        (
            r.generation,
            r.toolbar_state.panel.revision(),
            editor,
            r.editor.edit().unwrap().id,
        )
    });
    advance();
    ACTIVATIONS.with(|count| count.set(0));
    let request = crate::AnnotationChoiceRequest {
        field: 1,
        options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
        ..Default::default()
    };
    choice::open(generation, panel, request.clone());
    advance();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        let position = r
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .window()
            .position();
        // Alt+Tab can change foreground while the physical cursor is still on the popup.
        r.lifecycle.toolbar_cursor_position = Rc::new(move || {
            Some(Point {
                x: position.x + 8,
                y: position.y + 8,
            })
        });
    });
    let sink = sinks.borrow().last().unwrap().clone();
    sink(PopupPointerInput::DismissRequested {
        cause: PopupDismissCause::ForegroundChanged,
    });
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        0,
        "native external dismissal must not raise the editor"
    );
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.toolbar_state.menus.current.is_none() && r.toolbar_state.panel.get().is_some());
        assert_eq!(r.editor.edit().unwrap().id, id);
    });
    assert_eq!(
        (editor.get_anchor_offset(), editor.get_cursor_offset()),
        (1, 2)
    );
    editor
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: "X".into() });
    assert_eq!(
        editor.get_value(),
        "AXC",
        "local focus retains the insertion selection"
    );
    choice::open(generation, panel, request);
    advance();
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .invoke_chosen(0)
    });
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        1,
        "explicit menu completion still restores native input"
    );
    let parameter_panel = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .panel
            .get()
            .unwrap()
            .clone_strong()
    });
    parameter_panel
        .window()
        .set_size(slint::LogicalSize::new(900., 58.));
    let save_panel_render = |name: &str| {
        let Some(directory) = std::env::var_os("LEXIFT_ANNOTATION_RENDER_DIR") else {
            return;
        };
        let size = parameter_panel.window().size();
        let (width, height) = (size.width as usize, size.height as usize);
        let adapters = windows.borrow();
        let adapter = adapters
            .iter()
            .find(|adapter| std::ptr::eq(adapter.window(), parameter_panel.window()))
            .unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); width * height];
        parameter_panel.window().request_redraw();
        assert!(adapter.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width);
        }));
        let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
        ppm.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.ppm")), ppm).unwrap();
    };
    let (generation, panel_revision) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        (r.generation, r.toolbar_state.panel.revision())
    });
    choice::open(
        generation,
        panel_revision,
        crate::AnnotationChoiceRequest {
            field: -102,
            ..Default::default()
        },
    );
    advance();
    let numeric = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .clone_strong()
    });
    for event in [
        WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(226., 43.),
            button: slint::platform::PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(226., 43.),
            button: slint::platform::PointerEventButton::Left,
        },
    ] {
        numeric.window().dispatch_event(event);
    }
    advance();
    assert!(
        numeric.get_parameter_input_active(),
        "numeric popup input owns the keyboard"
    );
    let before = ACTIVATIONS.with(Cell::get);
    numeric.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Return.into(),
    });
    numeric.window().dispatch_event(WindowEvent::KeyReleased {
        text: slint::platform::Key::Return.into(),
    });
    advance();
    advance();
    assert!(!numeric.get_parameter_input_active());
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        before + 1,
        "Enter restores input exactly once"
    );
    save_panel_render("numeric-input-blur");

    let foreground = Rc::new(Cell::new(Some(lexift_core::ports::magnifier::WindowToken(
        101,
    ))));
    let observed = foreground.clone();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.lifecycle.annotation_foreground = Some(Rc::new(move || observed.get()));
        r.close_panel();
    });
    open_panel(3, false);
    let observed_panel = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .panel
            .get()
            .unwrap()
            .clone_strong()
    });
    observed_panel.invoke_parameter_input_finished(true);
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(202)));
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        2,
        "a foreground change before queued dispatch must revoke native activation"
    );
    observed_panel.invoke_parameter_input_finished(true);
    advance();
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(303)));
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        2,
        "a foreground change before the focus timer must revoke native activation"
    );
    observed_panel.invoke_parameter_input_finished(true);
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        3,
        "an unchanged foreground still permits the user's explicit completion"
    );
    let current_panel = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .panel
            .revision()
    });
    let menu_request = crate::AnnotationChoiceRequest {
        field: 1,
        options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
        ..Default::default()
    };
    choice::open(generation, current_panel, menu_request.clone());
    advance();
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .invoke_chosen(0)
    });
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(404)));
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        3,
        "menu callbacks retain the observation made before queued selection"
    );
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .annotation_window_token =
            Rc::new(|_| Some(lexift_core::ports::magnifier::WindowToken(505)))
    });
    choice::open(generation, current_panel, menu_request);
    advance();
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .invoke_chosen(0)
    });
    advance();
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(505)));
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        4,
        "retiring a popup may return to an exact annotation owner"
    );
    let toggle_request = crate::AnnotationChoiceRequest {
        field: 1,
        options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
        ..Default::default()
    };
    choice::open(generation, current_panel, toggle_request.clone());
    advance();
    observed_panel.invoke_choice_requested(toggle_request.clone());
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(606)));
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        4,
        "queued same-button close must retain the triggering foreground"
    );
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .menus
                .current
                .is_none()
        )
    });
    choice::open(generation, current_panel, toggle_request);
    advance();
    observed_panel.invoke_escape_requested();
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(707)));
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        4,
        "queued Escape closes one menu without reauthorizing activation"
    );
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(
            r.toolbar_state.menus.current.is_none()
                && r.editor.window().is_some()
                && r.toolbar_state.panel.get().is_some()
        );
        assert_eq!(r.editor.edit().unwrap().id, id);
    });

    let queries = Rc::new(Cell::new(0));
    let recorded = queries.clone();
    let observed = foreground.clone();
    let panel_input = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.lifecycle.annotation_foreground = Some(Rc::new(move || {
            recorded.set(recorded.get() + 1);
            observed.get()
        }));
        r.toolbar_state.panel.input()
    });
    let panel_sink = pointer_panel(
        observed_panel.as_weak(),
        generation,
        current_panel,
        panel_input,
    );
    for _ in 0..100 {
        panel_sink(PopupPointerInput::Moved { x: 8., y: 8. });
    }
    assert_eq!(
        queries.get(),
        0,
        "toolbar hover must not query foreground identity"
    );

    finish_text_edit(generation, false);
    REGISTRY.with(|slot| {
        slot.borrow_mut().as_mut().unwrap().prepare = |_| {
            if PREPARING.with(Cell::get) {
                PassiveWindowPreparation::Pending
            } else {
                PassiveWindowPreparation::Ready
            }
        }
    });
    PREPARING.with(|pending| pending.set(true));
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(808)));
    open_text_editor((-120., -50.), None, 1.25);
    advance();
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(909)));
    PREPARING.with(|pending| pending.set(false));
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        4,
        "text preparation retries retain the original opening request"
    );
    finish_text_edit(generation, false);

    PREPARING.with(|pending| pending.set(true));
    observed_panel.invoke_watermark_edit_requested();
    foreground.set(Some(lexift_core::ports::magnifier::WindowToken(1010)));
    advance();
    advance();
    PREPARING.with(|pending| pending.set(false));
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        4,
        "watermark opening retains its source across input dispatch and native preparation"
    );
    let watermark = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    close_watermark_input(generation, watermark);
    observed_panel.invoke_watermark_edit_requested();
    advance();
    advance();
    assert_eq!(
        ACTIVATIONS.with(Cell::get),
        5,
        "an unchanged explicit opening still activates the watermark input"
    );

    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.as_mut().unwrap().close();
        slot.take();
    });
}
#[test]
fn watermark_menu_returns_to_its_owner_and_retirement_cancels_restoration() {
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
        clock.set(clock.get() + Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    };
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
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.generation
    });
    open_watermark_input(generation);
    let returns = Rc::new(Cell::new(0));
    let (revision, retained) = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        let editor = r.toolbar_state.watermark.get().unwrap().clone_strong();
        let count = returns.clone();
        editor.on_focus_input(move || count.set(count.get() + 1));
        (r.toolbar_state.watermark.revision(), editor)
    });
    choice::open_watermark(generation, revision, 60., 217., 140., 34.);
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .invoke_chosen(0);
    });
    advance();
    advance();
    assert_eq!(
        returns.get(),
        0,
        "closing the popup only schedules a return"
    );
    assert!(retained.get_value().contains("$yyyy-MM-dd HH:mm:ss$"));
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        focus::Request {
            session: generation,
            target: focus::Target::Watermark { editor: revision },
            menu: r.toolbar_state.menus.revision,
            activation: focus::Activation::Local,
        }
    });
    // A watermark return belongs to its input window, not a replaced parameter toolbar.
    let panel = AnnotationPanel::new().unwrap();
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .toolbar_state
            .panel
            .open(panel.clone_strong())
    });
    advance();
    assert_eq!(returns.get(), 1);
    assert!(!restore_matching_input_focus(focus::Request {
        session: generation.wrapping_add(1),
        ..request
    }));
    assert!(!restore_matching_input_focus(focus::Request {
        target: focus::Target::Text {
            editor: revision,
            panel: 0
        },
        ..request
    }));
    choice::open_watermark(generation, revision, 60., 217., 140., 34.);
    assert!(
        !restore_matching_input_focus(request),
        "a new popup owns input"
    );
    choice::open_watermark(generation, revision, 60., 217., 140., 34.);
    assert_eq!(returns.get(), 1);
    panel.set_parameter_input_active(true);
    advance();
    assert_eq!(returns.get(), 1, "the parameter input keeps the keyboard");
    panel.set_parameter_input_active(false);
    REGISTRY.with(|slot| restore_editor_focus(slot.borrow().as_ref().unwrap()));
    advance();
    assert_eq!(
        returns.get(),
        2,
        "finishing the control returns to the active watermark input"
    );
    choice::open_watermark(generation, revision, 60., 217., 140., 34.);
    choice::open_watermark(generation, revision, 60., 217., 140., 34.);
    // Destroy/reopen before the next frame, retaining the old component intentionally.
    open_watermark_input(generation);
    let new_returns = Rc::new(Cell::new(0));
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_ne!(r.toolbar_state.watermark.revision(), revision);
        let count = new_returns.clone();
        r.toolbar_state
            .watermark
            .get()
            .unwrap()
            .on_focus_input(move || count.set(count.get() + 1));
    });
    advance();
    assert_eq!(returns.get(), 2);
    assert_eq!(new_returns.get(), 0);
    assert!(!restore_matching_input_focus(request));
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.as_mut().unwrap().close();
        slot.take();
    });
    advance();
    assert_eq!(returns.get(), 2);
    assert_eq!(new_returns.get(), 0);
}
#[test]
fn watermark_open_on_a_work_area_smaller_than_the_input_does_not_panic() {
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
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.lifecycle.popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -100,
                top: -50,
                right: 200,
                bottom: 200,
            })
        });
        r.generation
    });
    open_watermark_input(generation);
    let old_revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
    REGISTRY.with(|slot| {
        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .watermark
                .get()
                .is_some()
        )
    });
    open_watermark_input(generation);
    let queries = Rc::new(Cell::new(0));
    let counted = queries.clone();
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .popup_work_area = Rc::new(move |_| {
            counted.set(counted.get() + 1);
            Some(Rect {
                left: -100,
                top: -50,
                right: 200,
                bottom: 200,
            })
        });
    });
    settle_watermark_position(generation, old_revision, 2);
    assert_eq!(
        queries.get(),
        0,
        "old DPI settlement must not touch its replacement"
    );
    let current_revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    settle_watermark_position(generation, current_revision, 1);
    assert_eq!(queries.get(), 1);
    shutdown();
}
#[test]
fn watermark_pointer_bridge_reaches_controls_and_stale_instances_are_ignored() {
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
    for dpi in [1., 1.25] {
        let editor = AnnotationWatermarkInput::new().unwrap();
        editor
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
        editor
            .window()
            .set_size(slint::LogicalSize::new(440., 270.));
        editor.show().unwrap();
        let applied = Rc::new(RefCell::new(None));
        let applied_sink = applied.clone();
        editor.on_apply(move |value| *applied_sink.borrow_mut() = Some(value.to_string()));
        let cancelled = Rc::new(Cell::new(false));
        let cancel_sink = cancelled.clone();
        editor.on_cancel(move || cancel_sink.set(true));
        let sink = watermark::pointer_sink(
            editor.as_weak(),
            || true,
            window::InputWork::new(),
            choice::watermark_pointer,
        );
        let click = |x: f32, y: f32| {
            sink(PopupPointerInput::Moved {
                x: x * dpi,
                y: y * dpi,
            });
            sink(PopupPointerInput::LeftPressed {
                x: x * dpi,
                y: y * dpi,
            });
            sink(PopupPointerInput::LeftReleased {
                x: x * dpi,
                y: y * dpi,
            });
        };
        click(30., 55.);
        editor.window().dispatch_event(WindowEvent::KeyPressed {
            text: "中文".into(),
        });
        editor.window().dispatch_event(WindowEvent::KeyReleased {
            text: "中文".into(),
        });
        assert_eq!(editor.get_value(), "中文");
        let requested = Rc::new(Cell::new(false));
        let requested_sink = requested.clone();
        editor.on_presets_requested(move |x, y, width, height| {
            assert_eq!((x, y, width, height), (60., 217., 140., 34.));
            requested_sink.set(true);
        });
        click(130., 234.);
        assert!(requested.get());
        assert_eq!(editor.get_selected_preset(), 1);
        editor.invoke_choose_preset(1);
        assert!(editor.get_value().contains("$yyyy/MM/dd HH:mm:ss$"));
        click(300., 234.);
        assert_eq!(
            applied.borrow().as_deref(),
            Some(editor.get_value().as_str())
        );
        click(385., 234.);
        assert!(cancelled.get());
        drop(editor);
        sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
    }
    let sinks = Rc::new(RefCell::new(Vec::<crate::bridge::PopupPointerSink>::new()));
    let received = sinks.clone();
    init(
        |_| PassiveWindowPreparation::Ready,
        WindowLifecycleCallbacks::new(
            move |_, sink| {
                received.borrow_mut().push(sink);
                true
            },
            |_| true,
            |_| false,
            |_, _, _| false,
            |_, _| true,
            |_, _| true,
            || false,
        ),
    );
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.generation
    });
    open_watermark_input(generation);
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
    let old_revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
    let old_sink = sinks.borrow()[0].clone();
    // Retain the old component deliberately: identity protection must not rely on weak failure.
    let retained_old = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .get()
            .unwrap()
            .clone_strong()
    });
    choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.toolbar_state.watermark.get().unwrap().get_presets_open());
        let menu = &r.toolbar_state.menus.current.as_ref().unwrap().window;
        assert_eq!(menu.get_selected(), 1);
        assert_eq!(menu.get_menu_height(), 404.);
    });
    // Clicking the trigger toggles the independent menu, not the input window.
    choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        assert!(slot.as_ref().unwrap().toolbar_state.menus.current.is_none());
        assert!(
            slot.as_ref()
                .unwrap()
                .toolbar_state
                .watermark
                .get()
                .is_some()
        );
    });
    choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
    let retired_menu = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .clone_strong()
    });
    // Queue a choice before replacement, then deliver more callbacks while
    // the retired window remains alive. Ownership must reject all of them.
    retired_menu.invoke_chosen(0);
    open_watermark_input(generation);
    choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
    choice::dismiss_watermark(generation, old_revision);
    show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
    let current_revision = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .watermark
            .revision()
    });
    choice::open_watermark(generation, current_revision, 60., 217., 140., 34.);
    REGISTRY.with(|slot| {
        let _replacement = slot.borrow_mut();
        old_sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
    });
    retired_menu.invoke_chosen(0);
    retired_menu.invoke_cancel();
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.toolbar_state.menus.current.is_some());
        let editor = r.toolbar_state.watermark.get().unwrap();
        assert!(editor.get_presets_open());
        assert_eq!(editor.get_selected_preset(), 1);
        assert_eq!(editor.get_value(), "");
    });
    drop(retired_menu);
    let current_menu = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .menus
            .current
            .as_ref()
            .unwrap()
            .window
            .clone_strong()
    });
    current_menu.invoke_chosen(0);
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(
            r.toolbar_state
                .watermark
                .get()
                .unwrap()
                .get_selected_preset(),
            0
        );
        assert!(
            r.toolbar_state
                .watermark
                .get()
                .unwrap()
                .get_value()
                .contains("$yyyy-MM-dd HH:mm:ss$")
        );
    });
    std::thread::sleep(Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.toolbar_state.menus.current.is_none());
        let editor = r.toolbar_state.watermark.get().unwrap();
        assert!(!editor.get_presets_open());
        let before = editor.get_value().to_string();
        editor.window().dispatch_event(WindowEvent::KeyPressed {
            text: "继续".into(),
        });
        editor.window().dispatch_event(WindowEvent::KeyReleased {
            text: "继续".into(),
        });
        assert!(editor.get_value().contains("继续"));
        assert!(editor.get_value().contains(&before));
    });
    drop(current_menu);
    drop(retained_old);
    apply_watermark(generation, old_revision, "stale".into());
    close_watermark_input(generation, old_revision);
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(r.toolbar_state.watermark.get().is_some());
        assert!(r.toolbar_state.menus.current.is_none());
        assert!(r.session.watermark.is_none());
        assert!(!r.session.can_undo());
        r.close();
        assert!(r.toolbar_state.watermark.get().is_none());
        slot.take();
    });
}
#[test]
fn text_handle_activation_preserves_parameters_and_separates_undo() {
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
        for cancel in [false, true] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.session = Session::new();
                r.parameter_values = parameters::State::new();
                r.mode = InteractionMode::Tool(3);
                r.sync_edit_scope();
                r.selected[3] = 8;
                r.sync_edit_scope();
                let text = text_from_values(r, "文字".into());
                assert!(r.session.add(Object {
                    erased: Vec::new(),
                    id: ObjectId::DRAFT,
                    kind: Kind::Text,
                    bounds: crate::annotation_text::text_bounds((-120., 60.), &text, dpi),
                    style: current_style(r, 8),
                    points: Vec::new(),
                    extra: Extra::Text(text),
                }));
                let baseline = r.session.objects[0].clone();
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(8);
                r.toolbar_state.panel.open(panel);
                restore_parameter_projection(r);
                let start = baseline.edit_handles(dpi)[3];
                begin_text_handle(r, baseline.clone(), 3, start, dpi, false);
                let mut values = r.values(8).clone();
                parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                    .apply(&mut values);
                let target = r.parameter_target(8);
                assert!(apply_geometry_values(r, target, values).is_some());
                let colored = r.session.objects[0].clone();
                update_text_handle(r, (start.0 + dpi, start.1));
                assert!(r.parameter_edit.active());
                assert!(!r.gesture().unwrap().owns_document_transaction());
                update_text_handle(r, (start.0 + 20. * dpi, start.1 + 20. * dpi));
                assert!(!r.parameter_edit.active());
                assert!(r.gesture().unwrap().owns_document_transaction());
                assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                let end = (start.0 + 40. * dpi, start.1 + 40. * dpi);
                update_text_handle(r, end);
                let mut values = r.values(8).clone();
                parameters::Edit::Color(parameters::ColorTarget::Body, [77, 88, 99])
                    .apply(&mut values);
                assert!(apply_geometry_values(r, target, values).is_none());
                if cancel {
                    assert!(cancel_active_gesture(r));
                } else {
                    finish_text_handle(r, end);
                }
                assert_eq!(r.session.objects[0].style.color, [77, 88, 99]);
                r.finish_style_edit();
                assert!(r.session.undo());
                assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                if !cancel {
                    assert_ne!(r.session.objects[0].bounds, colored.bounds);
                    assert!(r.session.undo());
                }
                assert_eq!(r.session.objects, vec![colored]);
                assert!(r.session.undo());
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
fn draft_text_handle_preserves_parameters_across_activation_and_cancel() {
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
        for cancel in [false, true] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.close_panel();
                r.editor.clear();
                r.session = Session::new();
                r.parameter_values = parameters::State::new();
                r.mode = InteractionMode::Tool(3);
                r.sync_edit_scope();
                r.selected[3] = 8;
                r.sync_edit_scope();
                let original = text_from_values(r, "说明文字".into());
                let at = (-120., 60.);
                let bounds = crate::annotation_text::text_bounds(at, &original, dpi);
                let window = AnnotationTextInput::new().unwrap();
                window.set_value(original.content.clone().into());
                window.set_font_name(original.font_family.clone().into());
                window.set_text_size(original.font_size);
                let edit = TextEdit::new(
                    editor::next_instance(),
                    None,
                    at,
                    dpi,
                    original.clone(),
                    bounds,
                    original.content.clone(),
                );
                r.editor
                    .open(window, edit, Some(r.parameter_values.defaults(8).clone()));
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(8);
                r.toolbar_state.panel.open(panel);
                // An inactive control press owns no geometry to roll back.
                let before = editing_text_object(r).unwrap();
                let press = before.edit_handles(dpi)[3];
                begin_text_handle(r, before, 3, press, dpi, true);
                let target = r.parameter_target(8);
                let mut values = r.values(8).clone();
                values.text_size += 11;
                assert!(apply_geometry_values(r, target, values).is_some());
                let accepted = editing_text_object(r).unwrap();
                assert!(!r.text_handle().unwrap().info().active);
                assert!(cancel_active_gesture(r));
                assert_eq!(
                    editing_text_object(r).unwrap(),
                    accepted,
                    "cancelling an inactive handle must retain accepted typography"
                );
                assert!(r.text_handle().is_none() && r.gesture().is_none());
                let baseline = editing_text_object(r).unwrap();
                let start = baseline.edit_handles(dpi)[3];
                begin_text_handle(r, baseline, 3, start, dpi, true);
                let target = r.parameter_target(8);
                let mut values = r.values(8).clone();
                parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                    .apply(&mut values);
                assert!(apply_geometry_values(r, target, values).is_some());
                let colored = editing_text_object(r).unwrap();
                update_text_handle(r, (start.0 + 20. * dpi, start.1 + 20. * dpi));
                assert!(!r.parameter_edit.active());
                assert_eq!(editing_text_object(r).unwrap().style.color, [44, 55, 66]);
                assert!(r.gesture().unwrap().blocks_parameter_edits());
                assert!(!r.gesture().unwrap().owns_document_transaction());
                let mut values = r.values(8).clone();
                parameters::Edit::Color(parameters::ColorTarget::Body, [77, 88, 99])
                    .apply(&mut values);
                assert!(apply_geometry_values(r, target, values).is_none());
                let end = (start.0 + 40. * dpi, start.1 + 40. * dpi);
                update_text_handle(r, end);
                assert_eq!(editing_text_object(r).unwrap().style.color, [44, 55, 66]);
                if cancel {
                    assert!(cancel_active_gesture(r));
                } else {
                    finish_text_handle(r, end);
                }
                let current = editing_text_object(r).unwrap();
                assert_eq!(current.style.color, [77, 88, 99]);
                if cancel {
                    assert_eq!(current.bounds, colored.bounds);
                } else {
                    assert_ne!(current.bounds, colored.bounds);
                }
                assert_eq!(r.editor.window().unwrap().get_value().as_str(), "说明文字");
                assert!(r.deferred_parameters.is_empty());
                r.finish_style_edit();
                assert!(r.session.objects.is_empty());
                assert!(
                    !r.session.can_undo(),
                    "draft geometry and parameters never create document history"
                );
            });
        }
    }
    shutdown();
}
#[test]
fn text_resize_ignores_clicks_and_keeps_the_grab_offset() {
    for dpi in [1., 1.25] {
        let start = (-120., 60.);
        let grab = (6. * dpi, -4. * dpi);
        assert!(super::gestures::text_drag_target(start, grab, start, dpi, false).is_none());
        assert!(
            super::gestures::text_drag_target(
                start,
                grab,
                (start.0 + 3. * dpi, start.1),
                dpi,
                false
            )
            .is_none()
        );
        let target = super::gestures::text_drag_target(
            start,
            grab,
            (start.0 + 20., start.1 + 30.),
            dpi,
            false,
        )
        .unwrap();
        assert_eq!(target, (start.0 + 20. - grab.0, start.1 + 30. - grab.1));
        // Returning to the press position after crossing the threshold restores geometry.
        assert_eq!(
            super::gestures::text_drag_target(start, grab, start, dpi, true),
            Some((start.0 - grab.0, start.1 - grab.1))
        );
    }
}
#[test]
fn color_edits_preserve_fractional_and_scaled_text_dimensions() {
    let original = TextAnnotation {
        content: "文字".into(),
        font_family: "Microsoft YaHei".into(),
        font_size: 777.75,
        bold: false,
        italic: false,
        outline: Some(TextOutline {
            color: [0, 0, 0],
            width: 180.5,
        }),
        background: Some(TextBackground {
            color: [255, 255, 255],
            opacity: 200,
            rounding: 400.5,
            padding: 240.5,
        }),
        rotation: 0.,
        layout_scale: 50.,
        linked_arrow: None,
        linked_arrow_control: None,
    };
    let before = defaults(8);
    let mut after = before.clone();
    after.color_index = 1;
    let mut updated = original.clone();
    updated.font_size = 778.;
    updated.outline.as_mut().unwrap().width = 20.;
    updated.background.as_mut().unwrap().padding = 0.;
    updated.background.as_mut().unwrap().rounding = 0.;
    preserve_text_dimensions(&mut updated, &original, &before, &after);
    assert_eq!(updated, original);
}
#[test]
fn editor_viewport_clips_large_logical_bounds_to_negative_desktop() {
    let screens = [
        Rect {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        },
        Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        },
    ];
    let huge = Bounds::from_corners((-3000., -500.), (50_000., 20_000.));
    let viewport = text_editor_viewport(huge, &screens);
    assert_eq!(
        viewport,
        Bounds::from_corners((-1920., -100.), (2560., 1440.))
    );
    let small = Bounds::from_corners((-100., 20.), (50., 60.));
    assert_eq!(text_editor_viewport(small, &screens), small);
}
#[test]
fn text_editor_failure_invalidates_preview_and_retires_input() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    thread_local! {
        static PREPARATION: Cell<u8> = const { Cell::new(0) };
    }
    struct TestPlatform(Rc<Cell<Duration>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform(clock.clone()))).unwrap();
    slint::platform::update_timers_and_animations();
    init(
        |_| match PREPARATION.with(Cell::get) {
            0 => PassiveWindowPreparation::Failed,
            1 => PassiveWindowPreparation::Pending,
            _ => PassiveWindowPreparation::Ready,
        },
        WindowLifecycleCallbacks::new(
            |_, _| false,
            |_| panic!("failed editor must not activate"),
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
        r.selected[3] = 8;
        assert!(r.set_mode(InteractionMode::Tool(3)));
    });
    for failure in 0..3 {
        PREPARATION.with(|preparation| preparation.set(failure));
        open_text_editor((-120., -50.), None, 1.25);
        let (generation, id, held) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let held = r.editor.window().unwrap().clone_strong();
            held.set_value("待显示草稿".into());
            assert!(editing_text_object(r).is_some());
            // Model a preview that was already presented before the delayed native result.
            r.render_state.frame_tasks.cancel_render();
            (r.generation, r.editor.edit().unwrap().id, held)
        });
        show_text_editor(generation, id, 20, focus::Origin::Unobserved);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.editor.window().is_none());
            assert!(editing_text_object(r).is_none());
            assert!(
                r.render_state.frame_tasks.render_pending(),
                "retiring an editor must repaint a previously displayed preview"
            );
            assert!(r.session.objects.is_empty());
            assert!(!r.session.undo());
            r.render_state.frame_tasks.cancel_render();
        });
        held.invoke_content_changed("迟到输入".into());
        held.invoke_done("迟到完成".into());
        show_text_editor(generation, id, 21, focus::Origin::Unobserved);
        clock.set(clock.get() + Duration::from_millis(600));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let r = slot.borrow();
            let r = r.as_ref().unwrap();
            assert!(r.editor.window().is_none());
            assert!(!r.render_state.frame_tasks.render_pending());
            assert!(r.session.objects.is_empty());
        });
    }
    shutdown();
}
#[test]
fn text_editor_owner_is_attached_after_ready_and_old_retries_are_inert() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    thread_local! {
        static READY: Cell<bool> = const { Cell::new(false) };
        static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let accept_owner = Rc::new(Cell::new(true));
    let accepted = accept_owner.clone();
    init(
        |_| {
            EVENTS.with(|events| events.borrow_mut().push("prepare"));
            if READY.with(Cell::get) {
                PassiveWindowPreparation::Ready
            } else {
                PassiveWindowPreparation::Pending
            }
        },
        WindowLifecycleCallbacks::new(
            |_, _| {
                EVENTS.with(|events| events.borrow_mut().push("bridge"));
                true
            },
            |_| {
                EVENTS.with(|events| events.borrow_mut().push("activate"));
                true
            },
            |_| false,
            |_, _, _| false,
            |_, _| true,
            move |_, _| {
                EVENTS.with(|events| events.borrow_mut().push("owner"));
                READY.with(Cell::get) && accepted.get()
            },
            || false,
        ),
    );
    REGISTRY.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .unwrap()
            .canvas_state
            .canvases
            .push(CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: -200,
                    top: -100,
                    right: 400,
                    bottom: 400,
                },
            ));
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.selected[3] = 8;
        assert!(r.set_mode(InteractionMode::Tool(3)));
    });
    for scale in [1., 1.25] {
        for at in [(-120., -50.), (-220., -50.)] {
            READY.with(|ready| ready.set(false));
            EVENTS.with(|events| events.borrow_mut().clear());
            open_text_editor(at, None, scale);
            let (generation, old_id) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                (r.generation, r.editor.edit().unwrap().id)
            });
            assert!(
                EVENTS.with(|events| events.borrow().is_empty()),
                "creating a component must not attach an unprepared native window"
            );
            show_text_editor(generation, old_id, 0, focus::Origin::Unobserved);
            EVENTS.with(|events| assert_eq!(*events.borrow(), ["prepare"]));
            READY.with(|ready| ready.set(true));
            EVENTS.with(|events| events.borrow_mut().clear());
            show_text_editor(generation, old_id, 1, focus::Origin::Unobserved);
            EVENTS.with(|events| {
                assert_eq!(
                    *events.borrow(),
                    ["prepare", "owner", "bridge", "owner", "activate"]
                )
            });
            finish_text_edit(generation, false);
            open_text_editor(at, None, scale);
            let new_id =
                REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().editor.edit().unwrap().id);
            assert_ne!(old_id, new_id);
            EVENTS.with(|events| events.borrow_mut().clear());
            show_text_editor(generation, old_id, 2, focus::Origin::Unobserved);
            show_text_editor(
                generation.wrapping_add(1),
                new_id,
                0,
                focus::Origin::Unobserved,
            );
            assert!(EVENTS.with(|events| events.borrow().is_empty()));
            accept_owner.set(false);
            show_text_editor(generation, new_id, 0, focus::Origin::Unobserved);
            EVENTS.with(|events| assert_eq!(*events.borrow(), ["prepare", "owner"]));
            REGISTRY
                .with(|slot| assert!(slot.borrow().as_ref().unwrap().editor.window().is_some()));
            show_text_editor(generation, new_id, 20, focus::Origin::Unobserved);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.editor.window().is_none());
                assert!(r.session.objects.is_empty());
                assert!(!r.session.undo());
            });
            accept_owner.set(true);
        }
    }
    shutdown();
}
#[test]
fn text_editor_target_survives_index_changes_and_does_not_resurrect_deleted_objects() {
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform(Rc<std::cell::Cell<Duration>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    let clock = Rc::new(std::cell::Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform(clock.clone()))).unwrap();
    slint::platform::update_timers_and_animations();
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
        for sequence in [false, true] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.selected[3] = 8;
                assert!(r.set_mode(InteractionMode::Tool(if sequence { 4 } else { 3 })));
                r.ensure_fonts();
                for i in 0..3 {
                    let object = if sequence {
                        let mut object = crate::annotation_sequence::tests::badge(dpi);
                        crate::annotation_sequence::layout(&mut object);
                        object
                    } else {
                        let text = text_from_values(r, format!("Text {i}"));
                        Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            kind: Kind::Text,
                            bounds: crate::annotation_text::text_bounds((-100., 20.), &text, dpi),
                            style: current_style(r, 8),
                            points: Vec::new(),
                            extra: Extra::Text(text),
                        }
                    };
                    assert!(r.session.add(object));
                }
            });
            open_text_editor((-100., 20.), Some(1), dpi);
            let (target, last) = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                let target = r.session.objects[1].id;
                let last = r.session.objects[2].clone();
                r.editor.window().unwrap().set_value("修改中".into());
                r.session.selected = Some(0);
                r.session.delete_selected();
                assert_eq!(editing_text_object(r).unwrap().id, target);
                let input = r.editor.window().unwrap();
                input.invoke_content_changed("修改中".into());
                input.invoke_caret_changed();
                input.invoke_done("修改中".into());
                (target, last)
            });
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(r.editor.edit().is_none());
                assert_eq!(r.session.objects.len(), 2);
                assert_eq!(r.session.objects[0].id, target);
                let content = match &r.session.objects[0].extra {
                    Extra::Text(t) => &t.content,
                    Extra::Sequence(s) => &s.caption.content,
                    _ => unreachable!(),
                };
                assert_eq!(content, "修改中");
                assert_eq!(r.session.objects[1], last);
            });
            open_text_editor((-100., 20.), Some(0), dpi);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                let before = r.session.objects.clone();
                r.editor.window().unwrap().set_value("obsolete".into());
                r.session.selected = Some(0);
                r.session.delete_selected();
                assert!(editing_text_object(r).is_none());
                finish_text_edit_registry(r, true);
                assert_eq!(r.session.objects, before[1..]);
                r.session.undo();
                assert_eq!(
                    r.session.objects, before,
                    "ignored commit must not add an undo record"
                );
                r.close();
            });
        }
    }
    shutdown();
}
