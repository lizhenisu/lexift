use super::*;
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.0.borrow_mut() = Some(w.clone());
        Ok(w)
    }
}
#[test]
fn countdown_projects_monotonic_deadline_without_tick_accumulation() {
    let now = Instant::now();
    let end = now + Duration::from_secs(3);
    for (ms, n) in [
        (0, 3),
        (1, 3),
        (999, 3),
        (1000, 2),
        (2001, 1),
        (3000, 0),
        (6000, 0),
    ] {
        assert_eq!(remaining_seconds(end, now + Duration::from_millis(ms)), n);
    }
}
#[test]
fn countdown_and_duration_menu_render_all_locales_themes_and_dpi() {
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
    let w = ScreenshotCountdownWindow::new().unwrap();
    let adapter = last.borrow().as_ref().unwrap().clone();
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for theme in [0, 1] {
            for dpi in [1., 1.25] {
                w.global::<crate::Colors>().set_preference(theme);
                w.set_seconds(10);
                w.window()
                    .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: dpi,
                    });
                w.window()
                    .set_size(slint::LogicalSize::new(w.get_content_width(), 48.));
                w.show().unwrap();
                slint::platform::update_timers_and_animations();
                adapter.request_redraw();
                let size = adapter.size();
                let mut pixels =
                    vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                assert!(adapter.draw_if_needed(|r| {
                    r.render(&mut pixels, size.width as usize);
                }));
                assert!(size.width >= 250 && size.height >= 48);
                if let Ok(folder) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
                    crate::i18n::write_test_bitmap(
                        &folder,
                        &format!("countdown-{}-{theme}-{dpi}", language.code()),
                        size.width,
                        size.height,
                        pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                    );
                }
            }
        }
    }
    w.hide().unwrap();
    let menu = AnnotationChoiceWindow::new().unwrap();
    let adapter = last.borrow().as_ref().unwrap().clone();
    let labels = [
        "Screenshot now",
        "Screenshot after 3 seconds",
        "Screenshot after 5 seconds",
        "Screenshot after 10 seconds",
    ];
    menu.set_options(slint::ModelRc::new(slint::VecModel::from(
        labels.map(|s| crate::i18n::tr(s).into()).to_vec(),
    )));
    menu.set_content_width(240.);
    menu.set_selected(0);
    menu.window()
        .set_size(slint::LogicalSize::new(240., menu.get_menu_height()));
    menu.show().unwrap();
    slint::platform::update_timers_and_animations();
    let chosen = Rc::new(std::cell::Cell::new(None));
    let observed = chosen.clone();
    menu.on_chosen(move |i| observed.set(Some(i)));
    for key in [
        slint::platform::Key::DownArrow,
        slint::platform::Key::Return,
    ] {
        menu.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: key.into() });
        menu.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: key.into() });
    }
    assert_eq!(chosen.get(), Some(1));
    adapter.request_redraw();
    menu.hide().unwrap();
}

#[test]
fn preparing_tick_retains_activation_handoff_until_explicit_finish() {
    FLOW.with(|s| {
        let mut s = s.borrow_mut();
        s.state.session = 77;
        s.state.phase = ScreenshotPhase::Preparing;
    });
    VIEWS.with(|s| {
        let mut s = s.borrow_mut();
        s.session = 77;
        s.sequence = 33;
        s.deadline = Some(Instant::now());
    });
    tick(77);
    VIEWS.with(|s| {
        assert!(s.borrow().deadline.is_some());
        assert_eq!(s.borrow().sequence, 33);
    });
    FLOW.with(|s| s.borrow_mut().state.phase = ScreenshotPhase::Idle);
    tick(77);
    VIEWS.with(|s| assert!(s.borrow().deadline.is_none()));
}
