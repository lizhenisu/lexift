use super::*;
use slint::platform::{
    Platform, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
struct Backend(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
impl Platform for Backend {
    fn create_window_adapter(
        &self,
    ) -> std::result::Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(window.clone());
        Ok(window)
    }
}
#[test]
fn older_presented_progress_does_not_timeout_or_acknowledge_a_newer_fence() {
    use lexift_core::ports::annotation_render::RenderCompletion;
    let mut v = View::default();
    let stamp = RenderStamp {
        session: 7,
        revision: 10,
    };
    v.identity = Some(Identity {
        session: 7,
        operation: 1,
        request: 1,
    });
    v.revision = 10;
    v.pending_stamp = Some(stamp);
    v.fence = Some((v.identity.unwrap(), stamp));
    v.waiting_since = Some(Instant::now() - Duration::from_secs(6));
    let mut frame = RenderCompletion {
        stamp: RenderStamp {
            revision: 9,
            ..stamp
        },
        sampled_regions: Arc::from([]),
        presented: true,
        wait: None,
    };
    assert!(v.observe_presentation(&frame));
    assert!(v.waiting_since.unwrap().elapsed() < Duration::from_secs(1));
    assert_eq!(v.pending_stamp, Some(stamp));
    assert_eq!(v.fence.unwrap().1, stamp);
    let stalled = Instant::now() - Duration::from_secs(6);
    v.waiting_since = Some(stalled);
    assert!(v.observe_presentation(&frame));
    assert_eq!(
        v.waiting_since,
        Some(stalled),
        "repeating an old frame is not progress"
    );
    frame.stamp.session += 1;
    assert!(!v.observe_presentation(&frame));
    frame.stamp = RenderStamp {
        revision: 11,
        ..stamp
    };
    assert!(!v.observe_presentation(&frame));
}

#[test]
fn cancellation_discards_recording_input_and_restores_toolbar_expansion() {
    slint::platform::set_platform(Box::new(Backend(Rc::new(RefCell::new(Vec::new()))))).unwrap();
    super::super::super::init(
        |_| PassiveWindowPreparation::Ready,
        super::super::tests::lifecycle(),
    );
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        let document = r.document.clone();
        VIEW.with(|slot| {
            let mut v = slot.borrow_mut();
            v.document = Some(document);
            v.toolbar_collapsed = false;
        });
        r.toolbar_state.main.get().unwrap().set_collapsed(true);
        r.gesture = Some(Active::Action(
            super::super::super::gestures::Action::PlacePoint,
        ));
        r.hovered = Some(0);
    });
    restore_editor();
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.gesture.is_none());
        assert!(r.hovered.is_none());
        assert!(r.document.session.objects.is_empty());
        assert!(!r.toolbar_state.main.get().unwrap().get_collapsed());
    });
    super::super::super::shutdown();
}

#[test]
fn recording_roi_is_not_mistaken_for_a_changed_monitor_layout() {
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(Backend(windows))).unwrap();
    let monitor = Rect {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    let bounds = Rect {
        left: -1600,
        top: 200,
        right: -600,
        bottom: 800,
    };
    let mut lifecycle = super::super::tests::lifecycle();
    lifecycle.annotation_displays = Rc::new(move || vec![monitor]);
    lifecycle.screenshot_displays = Rc::new(move || vec![(monitor, 1.25)]);
    super::super::super::init(|_| PassiveWindowPreparation::Ready, lifecycle);
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    super::super::super::set_handler(Rc::new(move |e| received.borrow_mut().push(e)));
    let request = ExportRequest {
        session: 7,
        request: 1,
        snapshot: Arc::new(lexift_core::domain::annotation_export::ExportSnapshot {
            session: Arc::new(Session::new()),
            monitor: bounds,
            monitors: vec![(monitor, 1.25)],
            scale: 1.25,
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        }),
    };
    let lifecycle =
        lexift_core::domain::screen_recording::State::new(7, 1, bounds, monitor).unwrap();
    VIEW.with(|v| v.borrow_mut().identity = Some(lifecycle.identity));
    FLOW.with(|f| {
        let mut f = f.borrow_mut();
        f.state.session = 7;
        f.state.phase = ScreenshotPhase::Recording;
        f.state.recording = Some(lexift_core::domain::screenshot::recording::Session {
            lifecycle,
            request,
            monitor,
            excluded: vec![],
            progress: None,
            artifact: None,
            backend_request: None,
        });
    });
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.canvas_state
            .canvases
            .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
    });
    super::super::super::toolbar_workflow::check_display_layout();
    assert!(
        events.borrow().is_empty(),
        "a smaller recording canvas is not a monitor change"
    );
    REGISTRY.with(|r| {
        r.borrow_mut()
            .as_mut()
            .unwrap()
            .lifecycle
            .screenshot_displays = Rc::new(move || vec![(monitor, 1.)])
    });
    super::super::super::toolbar_workflow::check_display_layout();
    assert!(matches!(
        events.borrow().last(),
        Some(lexift_core::AppEvent::Screenshot(
            ScreenshotEvent::Recording(Event::LayoutChanged {
                session: 7,
                operation: 1
            })
        ))
    ));
    clear();
    super::super::super::shutdown();
}
#[test]
fn recording_controls_render_all_languages_themes_dpis_and_phases() {
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(Backend(windows.clone()))).unwrap();
    let control = RecordingControl::new().unwrap();
    let adapter = windows.borrow().last().unwrap().clone();
    control.set_dimensions("1920 × 1080 px".into());
    control.set_elapsed("10:59".into());
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for dark in [false, true] {
            control
                .global::<crate::Colors>()
                .set_preference(i32::from(dark));
            for dpi in [1., 1.25] {
                adapter
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                let (width, height) = ((380. * dpi) as u32, (40. * dpi) as u32);
                adapter.set_size(slint::PhysicalSize::new(width, height));
                for phase in 0..7 {
                    control.set_phase(phase);
                    control.set_interrupted(phase == 5);
                    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                    adapter.request_redraw();
                    adapter.draw_if_needed(|r| {
                        r.render(&mut pixels, width as usize);
                    });
                    assert!(pixels.windows(2).any(|p| p[0] != p[1]));
                    if let Ok(directory) = std::env::var("LEXIFT_RECORDING_RENDER_DIR") {
                        crate::i18n::write_test_bitmap(
                            &directory,
                            &format!("recording-{}-{dark}-{dpi}-{phase}", language.code()),
                            width,
                            height,
                            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
    }
    let toolbar = AnnotationToolbar::new().unwrap();
    toolbar.set_screenshot_mode(true);
    let original = toolbar.get_toolbar_width();
    toolbar.set_recording_mode(true);
    assert!(toolbar.get_toolbar_width() < original);
    toolbar.set_recording_mode(false);
    assert_eq!(toolbar.get_toolbar_width(), original);
}
