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
fn pump(label: &str, ready: impl Fn() -> bool) {
    let start = Instant::now();
    while !ready() && start.elapsed() < Duration::from_secs(3) {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(ready(), "window workflow did not settle: {}", label);
}
fn object(bounds: Bounds) -> Object {
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
fn lifecycle() -> WindowLifecycleCallbacks {
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
    l.annotation_export_supported = true;
    l
}
#[test]
fn screenshot_toolbars_are_exclusive_and_restore_document_undo_after_failed_output_and_pin() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
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
            &mut crate::annotation_scene::SceneCompiler::default(),
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
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.toolbar_state.main.get().unwrap().get_pin_edit_mode());
        assert_eq!(r.canvas_state.canvases.len(), 1);
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
