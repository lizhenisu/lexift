use super::*;
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
pub(super) struct TestPlatform;
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}
pub(super) fn position_platform() -> Box<dyn Platform> {
    Box::new(PositionPlatform(Default::default()))
}
// Records both the final position and the position at the moment a window is shown.
struct PositionPlatform(Rc<RefCell<Vec<std::rc::Weak<PositionWindow>>>>);
struct PositionWindow {
    window: slint::Window,
    renderer: slint::platform::software_renderer::SoftwareRenderer,
    size: Cell<slint::PhysicalSize>,
    position: Cell<slint::PhysicalPosition>,
    shows: RefCell<Vec<slint::PhysicalPosition>>,
}
impl Platform for PositionPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let adapter = Rc::new_cyclic(|weak: &std::rc::Weak<PositionWindow>| PositionWindow {
            window: slint::Window::new(weak.clone()),
            renderer: slint::platform::software_renderer::SoftwareRenderer::new(),
            size: Cell::default(),
            position: Cell::default(),
            shows: RefCell::default(),
        });
        self.0.borrow_mut().push(Rc::downgrade(&adapter));
        Ok(adapter)
    }
}
impl WindowAdapter for PositionWindow {
    fn window(&self) -> &slint::Window {
        &self.window
    }
    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }
    fn size(&self) -> slint::PhysicalSize {
        self.size.get()
    }
    fn set_size(&self, size: slint::WindowSize) {
        self.size.set(size.to_physical(self.window.scale_factor()));
        self.window
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: size.to_logical(self.window.scale_factor()),
            });
    }
    fn position(&self) -> Option<slint::PhysicalPosition> {
        Some(self.position.get())
    }
    fn set_position(&self, position: slint::WindowPosition) {
        self.position
            .set(position.to_physical(self.window.scale_factor()));
    }
    fn set_visible(&self, visible: bool) -> Result<(), slint::PlatformError> {
        if visible {
            self.shows.borrow_mut().push(self.position.get());
        }
        Ok(())
    }
}
fn pump(label: &str, ready: impl Fn() -> bool) {
    let start = Instant::now();
    while !ready() && start.elapsed() < Duration::from_secs(3) {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(ready(), "window workflow did not settle: {}", label);
}
fn rendered_label(r: &Registry) -> crate::screen_interaction::size_label::Feedback {
    let label = label_feedback(r).expect("feedback in the same frame as the crop");
    let index = r
        .canvas_state
        .canvases
        .iter()
        .position(|c| c.bounds == label.host)
        .unwrap();
    let frame = frame(r, index).unwrap();
    let x = (label.bounds.left - label.host.left + 2) as u32;
    let y = (label.bounds.top - label.host.top + 14) as u32;
    assert_eq!(
        frame.pixel(x, y).unwrap().red(),
        0x20,
        "label must be inside the composed frame"
    );
    label
}
pub(super) fn object(bounds: Bounds) -> Object {
    Object {
        id: ObjectId::DRAFT,
        kind: Kind::Rectangle,
        bounds,
        style: Style {
            fill: true,
            ..Default::default()
        },
        points: vec![],
        extra: Extra::None,
        erased: vec![],
    }
}
pub(super) fn lifecycle() -> WindowLifecycleCallbacks {
    let mut l = WindowLifecycleCallbacks::new(
        |_, _| true,
        |_| true,
        |_| true,
        |_, _, _| true,
        |_, _| true,
        |_, _| true,
        || true,
    );
    l.annotation_displays = Rc::new(|| {
        vec![Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        }]
    });
    l.popup_work_area = Rc::new(|_| {
        Some(Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        })
    });
    l.annotation_frame_presenter = Rc::new(|_, _, _, _| true);
    l.annotation_click_through = Rc::new(|_, _| true);
    l.ocr_dismissal = Rc::new(|_, _| true);
    l.complete_region_frame = Rc::new(|_| true);
    l.configure_hint_window = Rc::new(|_| true);
    l.annotation_export_supported = true;
    l
}

#[test]
fn mouse_mode_rejects_object_editing_and_stale_parameters_for_every_kind() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    super::super::init(|_| PassiveWindowPreparation::Ready, lifecycle());
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        for kind in [
            Kind::Rectangle,
            Kind::Ellipse,
            Kind::HighlightRectangle,
            Kind::Arrow,
            Kind::Text,
            Kind::Sequence,
            Kind::MosaicArea,
            Kind::Magnifier,
        ] {
            let mut mark = object(Bounds::from_corners((20., 20.), (120., 100.)));
            mark.kind = kind;
            if kind == Kind::Arrow {
                mark.points = vec![(20., 20.), (120., 100.)];
            }
            if kind == Kind::Text {
                mark.extra = Extra::Text(text_from_values(r, "fixture".into()));
            }
            if kind == Kind::Sequence {
                mark = sequence_from_values(r, (20., 20.), 1.);
            }
            if kind == Kind::Magnifier {
                mark = magnifier_from_source(
                    mark.bounds,
                    mark.style,
                    r.document.parameter_values.defaults(7),
                )
                .unwrap();
            }
            let tool = kind.tool();
            assert!(r.set_mode(InteractionMode::Tool(tools::group(tool))));
            assert!(r.document.session.add(mark));
            adopt_selected_style(r, 0);
            let old = r.parameter_target(tool as i32);
            let mut values = r.values(tool).clone();
            values.size = 23;
            assert!(r.set_mode(InteractionMode::Mouse));
            let before = r.document.session.objects.clone();
            let defaults = r.document.parameter_values.defaults(tool).clone();
            assert_eq!(r.document.session.selected, None);
            assert!(r.document.session.hit((30., 30.), 1.).is_none());
            assert!(!r.parameter_entry_active(tool as i32));
            assert!(!adopt_selected_style(r, 0));
            assert!(apply_parameter_values(r, old, values).is_none());
            assert_eq!(r.document.parameter_values.defaults(tool), &defaults);
            assert_eq!(r.document.session.objects, before);
            assert!(r.document.session.undo());
            assert!(
                r.document.session.objects.is_empty(),
                "mouse operations must not add undo steps"
            );
        }
    });
    super::super::shutdown();
}
#[test]
fn screenshot_toolbars_are_exclusive_and_restore_document_undo_after_failed_output_and_pin() {
    let adapters = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(PositionPlatform(adapters.clone()))).unwrap();
    let l = lifecycle();
    super::super::init(|_| PassiveWindowPreparation::Ready, l.clone());
    selection_host::init(|_| PassiveWindowPreparation::Ready, l.clone());
    crate::pinned_screenshot::init(|_| PassiveWindowPreparation::Ready, l);
    let core = Rc::new(RefCell::new(lexift_core::AppState::new(
        lexift_core::domain::settings::Settings::default(),
    )));
    let commands = Rc::new(RefCell::new(vec![]));
    let state = core.clone();
    let recorded = commands.clone();
    super::super::set_handler(Rc::new(move |event| {
        let (result, screenshot) = {
            let mut s = state.borrow_mut();
            let result = s.reduce(event);
            (result, s.screenshot.clone())
        };
        recorded.borrow_mut().extend(result);
        sync(&screenshot);
    }));
    toggle(Instant::now());
    pump("original", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    let original = object(Bounds::from_corners((10., 10.), (90., 80.)));
    let old = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        r.document.session.add(original);
        r.document.parameter_values.defaults_mut(0).size = 17;
        r.toolbar_state.main.get().unwrap().set_collapsed(true);
        r.toolbar_state.main.get().unwrap().clone_strong()
    });
    request();
    let id = core.borrow().screenshot.session;
    assert_eq!(core.borrow().screenshot.phase, ScreenshotPhase::Preparing);
    suspend(id);
    assert!(!super::super::is_open());
    assert!(FLOW.with(|s| {
        s.borrow()
            .recovery
            .as_ref()
            .unwrap()
            .document
            .session
            .can_undo()
    }));
    emit(ScreenshotEvent::Prepared {
        session: id,
        result: Ok(()),
    });
    let frames = Arc::new(vec![CapturedRegion {
        bounds: Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        },
        pixels: [220, 220, 220, 255].repeat(300 * 200),
    }]);
    let weak = Arc::downgrade(&frames);
    show(
        id,
        frames.clone(),
        Arc::new(crate::FrozenCapture::new(frames.clone())),
    );
    assert!(!super::super::is_open());
    selection_host::pointer(
        id,
        frames[0].bounds,
        1.,
        PopupPointerInput::LeftPressed { x: 40., y: 40. },
    );
    selection_host::pointer(
        id,
        frames[0].bounds,
        1.,
        PopupPointerInput::Moved { x: 200., y: 160. },
    );
    selection_host::pointer(
        id,
        frames[0].bounds,
        1.,
        PopupPointerInput::LeftReleased { x: 200., y: 160. },
    );
    pump("screenshot", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    assert_eq!(core.borrow().screenshot.phase, ScreenshotPhase::Editing);
    assert!(!selection_host::active());
    REGISTRY.with(|s| {
        assert_eq!(
            rendered_label(s.borrow().as_ref().unwrap()).text,
            "160 × 120 px"
        );
    });
    let edge = Rect {
        left: 2,
        top: 2,
        right: 102,
        bottom: 42,
    };
    let original = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().unwrap();
        std::mem::replace(
            &mut r.screenshot.as_mut().unwrap().region,
            RegionEditor::new(edge),
        )
    });
    let (generation, revision, main) = REGISTRY.with(|s| {
        let slot = s.borrow();
        let r = slot.as_ref().unwrap();
        (
            r.generation,
            r.toolbar_state.main.revision(),
            r.toolbar_state.main.get().unwrap().clone_strong(),
        )
    });
    let adapter = adapters
        .borrow()
        .iter()
        .filter_map(std::rc::Weak::upgrade)
        .find(|a| std::ptr::eq(a.window(), main.window()))
        .unwrap();
    REGISTRY.with(|s| {
        let slot = s.borrow();
        let r = slot.as_ref().unwrap();
        let before = label_feedback(r).unwrap();
        let pixels = frame(r, 0).unwrap();
        let position = main.window().position();
        let size = main.window().size();
        main.window()
            .set_position(slint::PhysicalPosition::new(-300, -500));
        main.window().set_size(slint::PhysicalSize::new(40, 54));
        let after = label_feedback(r).unwrap();
        assert_eq!(
            before.bounds, after.bounds,
            "toolbar position and size must not move the label"
        );
        assert_eq!(before.text, after.text);
        assert_eq!(
            pixels.data(),
            frame(r, 0).unwrap().data(),
            "toolbar changes must not alter feedback pixels"
        );
        main.window().set_position(position);
        main.window().set_size(size);
    });
    let assert_feedback = || {
        let label = REGISTRY.with(|s| rendered_label(s.borrow().as_ref().unwrap()));
        let p = main.window().position();
        let b = label.bounds;
        let c = label.crop;
        assert!(
            b.right <= c.left || b.left >= c.right || b.bottom <= c.top || b.top >= c.bottom,
            "a free side must keep the caption outside the crop: label={b:?}, crop={c:?}"
        );

        assert_eq!(
            adapter.shows.borrow().last(),
            Some(&p),
            "must position before showing"
        );
    };
    show_main(generation, revision, 0);
    assert_feedback();
    // All handles, body move, no movement, Escape, capture loss, repeated restoration.
    for edge in [
        edge,
        Rect {
            left: 200,
            top: 2,
            right: 298,
            bottom: 198,
        },
    ] {
        let (l, t, r, b) = (
            edge.left as f32,
            edge.top as f32,
            edge.right as f32,
            edge.bottom as f32,
        );
        let (mx, my) = ((l + r) / 2., (t + b) / 2.);
        for (x, y) in [
            (l, t),
            (mx, t),
            (r, t),
            (l, my),
            (r, my),
            (l, b),
            (mx, b),
            (r, b),
            (mx, my),
        ] {
            for end in [2, 3, -1, 0] {
                REGISTRY.with(|s| {
                    let mut slot = s.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.screenshot.as_mut().unwrap().region = RegionEditor::new(edge);
                    // Direct fixture geometry changes must also present their label hit map.
                    super::super::render_initial_canvas(r, 0);
                });
                canvas_pointer(0, 0, x, y);
                pump("edge adjustment hidden", || !main.window().is_visible());
                show_main(generation, revision, 0);
                assert!(
                    !main.window().is_visible(),
                    "late show cannot interrupt adjustment"
                );
                if end != 0 {
                    canvas_pointer(0, 1, x + 4., y + 4.);
                }
                if end == -1 {
                    super::super::escape();
                } else {
                    let delta = if end == 2 { 4. } else { 0. };
                    canvas_pointer(0, if end == 0 { 2 } else { end }, x + delta, y + delta);
                }
                pump("edge adjustment restored", || main.window().is_visible());
                assert_feedback();
            }
        }
    }
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.screenshot.as_mut().unwrap().region = original;
    });
    let original_bounds = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .unwrap()
            .screenshot
            .as_ref()
            .unwrap()
            .region
            .bounds()
            .unwrap()
    });
    emit(ScreenshotEvent::Selected {
        session: id,
        bounds: original_bounds,
    });
    show_main(generation, revision, 0);
    drop(adapter);
    drop(main);
    let toolbar = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .unwrap()
            .toolbar_state
            .main
            .get()
            .unwrap()
            .clone_strong()
    });
    canvas_pointer(0, 1, 130., 100.);
    REGISTRY.with(|s| {
        assert_eq!(
            s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                .window
                .get_cursor(),
            crate::AnnotationCursor::Moving
        )
    });
    let original_crop = core.borrow().screenshot.bounds;
    for cancel in [2, 3, -1] {
        canvas_pointer(0, 0, 130., 100.);
        pump("crop toolbar hidden", || !toolbar.window().is_visible());
        // A late initial preparation must not reveal the toolbar during a drag.
        let (generation, revision) = REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            (r.generation, r.toolbar_state.main.revision())
        });
        show_main(generation, revision, 0);
        assert!(!toolbar.window().is_visible());
        canvas_pointer(0, 1, 140., 110.);
        assert_eq!(
            REGISTRY.with(|s| s
                .borrow()
                .as_ref()
                .unwrap()
                .screenshot
                .as_ref()
                .unwrap()
                .region
                .bounds()),
            Some(Rect {
                left: 50,
                top: 50,
                right: 210,
                bottom: 170
            })
        );
        REGISTRY.with(|s| {
            assert_eq!(
                rendered_label(s.borrow().as_ref().unwrap()).text,
                "160 × 120 px"
            );
        });
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                    .window
                    .get_cursor(),
                crate::AnnotationCursor::Moving
            )
        });
        if cancel == -1 {
            super::super::escape();
        } else {
            canvas_pointer(0, cancel, 130., 100.);
        }
        pump("crop toolbar restored", || toolbar.window().is_visible());
        assert_eq!(core.borrow().screenshot.bounds, original_crop);
        REGISTRY.with(|s| {
            assert_eq!(
                rendered_label(s.borrow().as_ref().unwrap()).text,
                "160 × 120 px"
            );
        });
        assert!(!toolbar.get_collapsed());
    }
    for (x, y, expected) in [
        (40., 40., crate::AnnotationCursor::DiagonalDown),
        (120., 40., crate::AnnotationCursor::Vertical),
        (200., 40., crate::AnnotationCursor::DiagonalUp),
        (40., 100., crate::AnnotationCursor::Horizontal),
        (200., 100., crate::AnnotationCursor::Horizontal),
        (40., 160., crate::AnnotationCursor::DiagonalUp),
        (120., 160., crate::AnnotationCursor::Vertical),
        (200., 160., crate::AnnotationCursor::DiagonalDown),
    ] {
        canvas_pointer(0, 1, x, y);
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                    .window
                    .get_cursor(),
                expected
            )
        });
        canvas_pointer(0, 0, x, y);
        pump("resize hidden", || !toolbar.window().is_visible());
        canvas_pointer(0, 1, 130., 100.);
        let resized = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .screenshot
                .as_ref()
                .unwrap()
                .region
                .bounds()
        });
        let b = resized.unwrap();
        REGISTRY.with(|s| {
            assert_eq!(
                rendered_label(s.borrow().as_ref().unwrap()).text,
                format!("{} × {} px", b.right - b.left, b.bottom - b.top)
            );
        });
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                    .window
                    .get_cursor(),
                expected
            )
        });
        canvas_pointer(0, 3, 130., 100.);
        pump("resize restored", || toolbar.window().is_visible());
    }
    canvas_pointer(0, 4, 130., 100.);
    REGISTRY.with(|s| {
        assert_eq!(
            s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                .window
                .get_cursor(),
            crate::AnnotationCursor::Idle
        )
    });
    drop(toolbar);
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.toolbar_state.main.get().unwrap().get_screenshot_mode());
        assert!(r.document.session.objects.is_empty());
        assert!(r.set_mode(InteractionMode::Tool(0)));
    });
    canvas_pointer(0, 0, 60., 60.);
    canvas_pointer(0, 1, 120., 120.);
    canvas_pointer(0, 2, 120., 120.);
    REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref().unwrap();
        assert_eq!(r.document.session.objects.len(), 1);
        assert!(r.document.session.can_undo());
    });
    let snapshot = REGISTRY.with(|s| export::snapshot(s.borrow().as_ref().unwrap()).unwrap());
    assert_eq!(snapshot.session.objects.len(), 1);
    REGISTRY.with(|s| {
        assert!(
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Mouse)
        )
    });
    let marks = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .unwrap()
            .document
            .session
            .objects
            .clone()
    });
    canvas_pointer(0, 1, 60., 90.);
    REGISTRY.with(|s| {
        assert_eq!(
            s.borrow().as_ref().unwrap().canvas_state.canvases[0]
                .window
                .get_cursor(),
            crate::AnnotationCursor::Moving
        )
    });
    canvas_pointer(0, 0, 60., 90.);
    pump("crop over annotation hides toolbar", || {
        REGISTRY.with(|s| {
            !s.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .main
                .get()
                .unwrap()
                .window()
                .is_visible()
        })
    });
    canvas_pointer(0, 1, 70., 100.);
    REGISTRY.with(|s| {
        let slot = s.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.screenshot.as_ref().unwrap().region.dragging());
        assert_eq!(r.document.session.selected, None);
        assert_eq!(r.document.session.objects, marks);
        assert_eq!(
            r.screenshot.as_ref().unwrap().region.bounds().unwrap().left,
            50
        );
    });
    canvas_pointer(0, 2, 60., 90.);
    pump("crop toolbar restored", || {
        REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .toolbar_state
                .main
                .get()
                .unwrap()
                .window()
                .is_visible()
        })
    });
    brush_size_scrolled(0, 120.);
    REGISTRY.with(|s| {
        let slot = s.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.document.mode, InteractionMode::Mouse);
        assert_eq!(r.document.session.objects, marks);
        assert!(r.toolbar_state.panel.get().is_none());
        assert_eq!(r.document.session.selected, None);
    });
    REGISTRY.with(|s| {
        assert!(
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Tool(0))
        )
    });
    output(ScreenshotOutput::Copy);
    let operation = core.borrow().screenshot.operation;
    assert_eq!(core.borrow().screenshot.phase, ScreenshotPhase::Output);
    emit(ScreenshotEvent::Completed {
        session: id,
        operation,
        result: Err(lexift_core::Error::new("fixture")),
    });
    assert_eq!(core.borrow().screenshot.phase, ScreenshotPhase::Editing);
    assert!(super::super::is_open());
    output(ScreenshotOutput::Pin);
    let operation = core.borrow().screenshot.operation;
    crate::pinned_screenshot::show(
        id,
        operation,
        Arc::new(CapturedRegion {
            bounds: Rect {
                left: 40,
                top: 40,
                right: 200,
                bottom: 160,
            },
            pixels: [200, 200, 200, 255].repeat(160 * 120),
        }),
    );
    pump("pin", || {
        core.borrow().screenshot.phase == ScreenshotPhase::Idle
    });
    assert!(crate::pinned_screenshot::active());
    restore(id);
    pump("restore", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    let count = commands.borrow().len();
    old.invoke_screenshot_requested();
    slint::platform::update_timers_and_animations();
    assert_eq!(
        commands.borrow().len(),
        count,
        "retired toolbar must not start a new screenshot"
    );
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(!r.toolbar_state.main.get().unwrap().get_screenshot_mode());
        assert!(r.document.mode.is_mouse());
        assert!(r.toolbar_state.main.get().unwrap().get_collapsed());
        assert_eq!(r.document.session.objects.len(), 1);
        assert_eq!(r.values(0).size, 17);
        assert!(r.document.session.undo());
        assert!(r.document.session.objects.is_empty());
        assert!(r.screenshot.is_none());
    });
    drop(frames);
    assert!(
        weak.upgrade().is_none(),
        "pinned crop must not retain full-screen frames"
    );
    request();
    let failed = core.borrow().screenshot.session;
    suspend(failed);
    emit(ScreenshotEvent::Prepared {
        session: failed,
        result: Err(lexift_core::Error::new("capture fixture")),
    });
    restore(failed);
    pump("failed preparation restore", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    assert!(core.borrow().screenshot.error.is_some());
    request();
    let cancelled = core.borrow().screenshot.session;
    suspend(cancelled);
    emit(ScreenshotEvent::Prepared {
        session: cancelled,
        result: Ok(()),
    });
    let cancelled_frames = Arc::new(vec![CapturedRegion {
        bounds: Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        },
        pixels: [220, 220, 220, 255].repeat(300 * 200),
    }]);
    let cancelled_weak = Arc::downgrade(&cancelled_frames);
    show(
        cancelled,
        cancelled_frames.clone(),
        Arc::new(crate::FrozenCapture::new(cancelled_frames.clone())),
    );
    cancel();
    restore(cancelled);
    drop(cancelled_frames);
    assert!(cancelled_weak.upgrade().is_none());
    selection_host::pointer(
        cancelled,
        Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        },
        1.,
        PopupPointerInput::LeftReleased { x: 150., y: 150. },
    );
    pump("cancelled preparation restore", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    assert_eq!(core.borrow().screenshot.phase, ScreenshotPhase::Idle);
    assert!(!selection_host::active());
    assert!(REGISTRY.with(|s| s.borrow().as_ref().unwrap().screenshot.is_none()));
    crate::pinned_screenshot::close_all();
    super::super::shutdown();
}
#[test]
fn frozen_preview_masks_outside_annotations_and_export_excludes_controls() {
    use lexift_core::domain::annotation_export::ExportSnapshot;
    for scale in [1., 1.25] {
        let monitor = Rect {
            left: -100,
            top: -80,
            right: 200,
            bottom: 160,
        };
        let crop = Rect {
            left: -40,
            top: -20,
            right: 100,
            bottom: 100,
        };
        let mut session = Session::new();
        session.add(object(Bounds::from_corners((-80., -60.), (140., 120.))));
        let base = CapturedRegion {
            bounds: monitor,
            pixels: [200, 200, 200, 255].repeat(300 * 240),
        };
        let capture = Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![base],
        });
        let snapshot = ExportSnapshot {
            session: Arc::new(session),
            monitor,
            scale,
            monitors: vec![(monitor, scale)],
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        };
        let input = screenshot_preview::Input {
            snapshot,
            capture: capture.clone(),
            crop,
            label: None,
            help: None,
            hovered: None,
            crop_handles: true,
            crop_border: true,
            erase: None,
        };
        let preview = screenshot_preview::paint(&input, &AtomicBool::new(false)).unwrap();
        assert_eq!(preview.pixel(10, 10).unwrap().red(), 90);
        let output = crate::annotation_export_render::compose(
            &input.snapshot,
            capture,
            &AtomicBool::new(false),
        )
        .unwrap();
        let index = (60 * 300 + 60) * 4;
        assert_ne!(
            &preview.data()[index..index + 4],
            &output.pixels[index..index + 4]
        );
        for y in 90..150 {
            let start = (y * 300 + 90) * 4;
            assert_eq!(
                &preview.data()[start..start + 60 * 4],
                &output.pixels[start..start + 60 * 4],
                "cached document must match output pixels inside the crop"
            );
        }
        let mut pin_input = input;
        pin_input.crop = monitor;
        pin_input.crop_handles = false;
        pin_input.crop_border = false;
        let layers = screenshot_preview::paint_frames(
            &pin_input,
            &AtomicBool::new(false),
            &mut screenshot_preview::Cache::default(),
        )
        .unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(
            layers[0].data(),
            output.pixels,
            "pin body must never contain interaction decorations"
        );
        assert!(
            layers[1].data().as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "selected object controls belong to a separate transparent layer"
        );
        if let Ok(directory) = std::env::var("LEXIFT_SCREENSHOT_SNAPSHOT_DIR") {
            crate::i18n::write_test_bitmap(
                &directory,
                &format!("screenshot-preview-{scale}"),
                300,
                240,
                preview
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| [p[0], p[1], p[2], p[3]]),
            );
        }
    }
}

#[test]
fn screenshot_toolbars_render_all_locales_themes_and_dpi() {
    use slint::platform::WindowEvent;
    struct RenderPlatform(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for RenderPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            self.0.borrow_mut().push(adapter.clone());
            Ok(adapter)
        }
    }
    let adapters = Rc::new(RefCell::new(vec![]));
    slint::platform::set_platform(Box::new(RenderPlatform(adapters.clone()))).unwrap();
    let toolbar = AnnotationToolbar::new().unwrap();
    let adapter = adapters.borrow()[0].clone();
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for dark in [false, true] {
            toolbar
                .global::<crate::Colors>()
                .set_preference(i32::from(dark));
            for scale in [1., 1.25] {
                for (screenshot, pin) in [(false, false), (true, false), (true, true)] {
                    toolbar.set_screenshot_mode(screenshot);
                    toolbar.set_pin_edit_mode(pin);
                    adapter
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        });
                    let width = (toolbar.get_toolbar_width() * scale).round() as u32;
                    let height = (54. * scale).round() as u32;
                    adapter.set_size(slint::PhysicalSize::new(width, height));
                    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                    adapter.request_redraw();
                    adapter.draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, width as usize);
                    });
                    assert!(
                        pixels.windows(2).any(|p| p[0] != p[1]),
                        "toolbar must render content"
                    );
                    if let Ok(directory) = std::env::var("LEXIFT_SCREENSHOT_SNAPSHOT_DIR") {
                        crate::i18n::write_test_bitmap(
                            &directory,
                            &format!(
                                "toolbar-{}-{dark}-{scale}-{screenshot}-{pin}",
                                language.code()
                            ),
                            width,
                            height,
                            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn pinned_editor_keeps_documents_history_and_hidden_output_separate() {
    use lexift_core::domain::{
        annotation_export::ExportRequest,
        pinned_image::{Command as PinCommand, Event as PinEvent, Output},
    };
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let mut l = lifecycle();
    let sinks = Rc::new(RefCell::new(Vec::<crate::PopupPointerSink>::new()));
    let recorded_sinks = sinks.clone();
    l.complete_toolbar_show = Rc::new(move |_, sink| {
        recorded_sinks.borrow_mut().push(sink);
        true
    });
    super::super::init(|_| PassiveWindowPreparation::Ready, l.clone());
    crate::pinned_screenshot::init(|_| PassiveWindowPreparation::Ready, l);
    let core = Rc::new(RefCell::new(lexift_core::AppState::default()));
    let commands = Rc::new(RefCell::new(Vec::new()));
    let state = core.clone();
    let recorded = commands.clone();
    super::super::set_handler(Rc::new(move |event| {
        let result = state.borrow_mut().reduce(event);
        let snapshot = state.borrow().clone();
        sync(&snapshot.screenshot);
        crate::pinned_screenshot::sync_outputs(&snapshot.pinned_images);
        for c in &result {
            match c {
                lexift_core::AppCommand::PinnedImage(PinCommand::BeginEdit { id, edit }) => {
                    let (id, edit) = (*id, *edit);
                    slint::Timer::single_shot(Duration::ZERO, move || pin_editor::begin(id, edit));
                }
                lexift_core::AppCommand::PinnedImage(PinCommand::EndEdit { id, edit }) => {
                    let (id, edit) = (*id, *edit);
                    slint::Timer::single_shot(Duration::ZERO, move || pin_editor::end(id, edit));
                }
                _ => {}
            }
        }
        recorded.borrow_mut().extend(result);
    }));
    toggle(Instant::now());
    pump("desktop", || {
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        r.document
            .session
            .add(object(Bounds::from_corners((10., 10.), (90., 70.))));
        r.document.parameter_values.defaults_mut(0).size = 17;
    });
    let snapshot = REGISTRY.with(|s| export::snapshot(s.borrow().as_ref().unwrap()).unwrap());
    let base = Arc::new(CapturedRegion {
        bounds: snapshot.monitor,
        pixels: [220, 220, 220, 255].repeat(300 * 200),
    });
    let source = Arc::new(ScreenCaptureFrame {
        revision: 1,
        serial: 1,
        regions: vec![],
    });
    let request = ExportRequest {
        session: 1,
        request: 1,
        snapshot: Arc::new(snapshot),
    };
    {
        let mut s = core.borrow_mut();
        s.screenshot.session = 1;
        s.screenshot.operation = 1;
        s.screenshot.phase = ScreenshotPhase::Output;
    }
    sync(&core.borrow().screenshot);
    crate::pinned_screenshot::show_document(1, 1, base.clone(), Some((base, source, request)));
    let (id, pin) = crate::pinned_screenshot::diagnostic_windows()
        .pop()
        .unwrap();
    let pin_sink = sinks.borrow().last().unwrap().clone();
    pin.invoke_annotate_requested();
    pump("pin edit", || {
        pin_editor::current().is_some_and(|p| p.0 == id)
            && REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    assert!(pin.get_editing());
    assert!(!pin.get_interactive());
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.toolbar_state.main.get().unwrap().get_pin_edit_mode());
        assert_eq!(r.canvas_state.canvases.len(), 1);
        assert!(r.document.mode.is_mouse());
        assert!(r.document.session.selected.is_none());
        assert!(
            r.document
                .session
                .objects
                .iter()
                .all(|o| !r.document.session.can_edit(o))
        );
        r.set_mode(InteractionMode::Tool(0));
    });
    let scale =
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.canvases[0].input_scale());
    for (event, x, y) in [(0, 130., 90.), (1, 200., 150.), (2, 200., 150.)] {
        pin.invoke_edit_pointer(event, x / scale, y / scale);
    }
    assert_eq!(
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().document.session.objects.len()),
        2
    );
    let before = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .unwrap()
            .document
            .session
            .objects
            .clone()
    });
    let px = |x: f32| (x / scale + pin.get_frame_inset()) * pin.window().scale_factor();
    pin_sink(PopupPointerInput::LeftPressed {
        x: px(220.),
        y: px(160.),
    });
    pin_sink(PopupPointerInput::Moved {
        x: px(260.),
        y: px(190.),
    });
    pin_sink(PopupPointerInput::LeftCancelled {
        x: px(260.),
        y: px(190.),
    });
    assert_eq!(
        REGISTRY.with(|s| s
            .borrow()
            .as_ref()
            .unwrap()
            .document
            .session
            .objects
            .clone()),
        before,
        "capture loss cancels the draft without changing completed marks"
    );
    REGISTRY.with(|s| {
        assert!(
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Mouse)
        )
    });
    assert!(
        !pin.get_interactive(),
        "mouse mode must pass through the editing pin"
    );
    for (event, x, y) in [(0, 130., 120.), (1, 180., 160.), (2, 180., 160.)] {
        pin.invoke_edit_pointer(event, x / scale, y / scale);
    }
    pin.invoke_edit_scroll(120.);
    REGISTRY.with(|s| {
        let slot = s.borrow();
        let r = slot.as_ref().unwrap();
        assert_eq!(r.document.mode, InteractionMode::Mouse);
        assert_eq!(r.document.session.objects, before);
        assert_eq!(r.document.session.selected, None);
        assert!(r.toolbar_state.panel.get().is_none());
    });
    REGISTRY.with(|s| {
        assert!(
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .set_mode(InteractionMode::Tool(0))
        )
    });
    assert!(
        pin.get_interactive(),
        "drawing restores only the target pin input"
    );
    pin_editor::output(Output::Copy);
    assert!(commands.borrow().iter().any(|c|matches!(c,lexift_core::AppCommand::PinnedImage(PinCommand::Output{document:Some(d),hidden:false,..}) if d.snapshot.session.objects.len()==2)));
    assert!(pin.get_editing(), "copy must keep editing open");
    let operation = core.borrow().pinned_images[&id].operation;
    emit_pin(PinEvent::Completed {
        id,
        operation,
        result: Ok(lexift_core::ports::annotation_save::SaveOutcome::Cancelled),
    });
    pin_editor::finish_now();
    pump("desktop restored", || {
        !pin_editor::active()
            && REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref().unwrap();
        assert_eq!(r.document.session.objects.len(), 1);
        assert_eq!(r.values(0).size, 17);
        assert!(r.document.mode.is_mouse());
    });
    pin.invoke_annotate_requested();
    pump("reopen pin", || {
        pin_editor::active()
            && REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.annotation_ready)
    });
    undo();
    assert_eq!(
        REGISTRY.with(|s| s.borrow().as_ref().unwrap().document.session.objects.len()),
        1
    );
    pin.invoke_visibility_requested(true);
    pump("hide ends editor", || !pin_editor::active());
    assert!(pin.get_annotations_hidden());
    pin.invoke_output_requested(0);
    assert!(commands.borrow().iter().any(|c| matches!(
        c,
        lexift_core::AppCommand::PinnedImage(PinCommand::Output { hidden: true, .. })
    )));
    crate::pinned_screenshot::request_close(id);
    pump("closed", || {
        crate::pinned_screenshot::diagnostic_windows().is_empty()
    });
    assert!(!core.borrow().pinned_images.contains_key(&id));
    crate::pinned_screenshot::close_all();
    super::super::shutdown();
}

#[test]
fn screenshot_completed_frames_ignore_newer_input_and_reject_late_results() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let presented = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let results = presented.clone();
    let mut l = lifecycle();
    l.annotation_frame_presenter = Rc::new(move |_, pixels, _, _| {
        results.borrow_mut().push(pixels.to_vec());
        true
    });
    super::super::init(|_| PassiveWindowPreparation::Ready, l.clone());
    crate::screen_interaction::size_label::prepare(1., &l).unwrap();
    let monitor = Rect {
        left: 0,
        top: 0,
        right: 300,
        bottom: 200,
    };
    let capture = Arc::new(ScreenCaptureFrame {
        revision: 9,
        serial: 1,
        regions: vec![CapturedRegion {
            bounds: monitor,
            pixels: [120, 160, 180, 255].repeat(300 * 200),
        }],
    });
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.generation = 42;
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        let window = AnnotationCanvas::new().unwrap();
        window.window().set_size(slint::PhysicalSize::new(300, 200));
        r.canvas_state
            .canvases
            .push(CanvasLayer::new(window, monitor));
        r.screenshot = Some(Editing {
            pin: None,
            id: 51,
            frames: Arc::new(capture.regions.clone()),
            capture,
            region: RegionEditor::new(Rect {
                left: 20,
                top: 60,
                right: 140,
                bottom: 180,
            }),
            preview: None,
            revision: 2,
            presented: 0,
            feedback_placement: None,
            toolbar_adjustment: 0,
            keyboard: Default::default(),
            initial_focus: None,
            help: Default::default(),
            size_input: Default::default(),
        });
    });
    let old = REGISTRY.with(|s| frame(s.borrow().as_ref().unwrap(), 0).unwrap());
    REGISTRY.with(|s| {
        s.borrow_mut()
            .as_mut()
            .unwrap()
            .screenshot
            .as_mut()
            .unwrap()
            .region = RegionEditor::new(Rect {
            left: 150,
            top: 50,
            right: 280,
            bottom: 180,
        })
    });
    let new = REGISTRY.with(|s| frame(s.borrow().as_ref().unwrap(), 0).unwrap());
    // Input has advanced again while the second completed frame waits for UI delivery.
    REGISTRY.with(|s| {
        s.borrow_mut()
            .as_mut()
            .unwrap()
            .screenshot
            .as_mut()
            .unwrap()
            .region = RegionEditor::new(Rect {
            left: 30,
            top: 40,
            right: 180,
            bottom: 170,
        })
    });
    let metadata = || {
        REGISTRY.with(|s| {
            vec![(
                s.borrow().as_ref().unwrap().canvas_state.canvases[0].revision(),
                None,
            )]
        })
    };
    super::super::frozen_editor::present(42, 51, 2, Some(vec![new.clone()]), metadata());
    assert_eq!(presented.borrow().as_slice(), &[new.data().to_vec()]);
    super::super::frozen_editor::present(42, 51, 1, Some(vec![old.clone()]), metadata());
    super::super::frozen_editor::present(42, 50, 3, Some(vec![old.clone()]), metadata());
    assert_eq!(
        presented.borrow().len(),
        1,
        "old completion or another session must not overwrite a full frame"
    );
    REGISTRY.with(|s| s.borrow_mut().as_mut().unwrap().close());
    super::super::frozen_editor::present(42, 51, 3, Some(vec![old]), vec![]);
    assert_eq!(
        presented.borrow().len(),
        1,
        "closed canvas cannot receive a queued frame"
    );
    super::super::shutdown();
}
