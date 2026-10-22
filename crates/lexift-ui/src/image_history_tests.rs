//! Render the actual history browser across the supported locale/theme/DPI matrix.
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};
struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.0.borrow_mut() = Some(w.clone());
        Ok(w)
    }
}
#[test]
fn history_browser_renders_locales_themes_dpi_and_keyboard_navigation() {
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
    let w = crate::ImageHistoryWindow::new().unwrap();
    let adapter = last.borrow().as_ref().unwrap().clone();
    let preview = slint::Image::from_rgba8_premultiplied(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::clone_from_slice(
        &[42, 88, 160, 255].repeat(180 * 320),
        180,
        320,
    ));
    w.set_entries(ModelRc::new(VecModel::from(vec![crate::ImageHistoryRow {
        identity: "8".into(),
        preview: preview.clone(),
        kind: "Screenshot".into(),
        dimensions: "32768 × 32768".into(),
        updated: "10-22 14:36:28".into(),
    }])));
    w.set_selection("8".into());
    w.set_preview(preview);
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for theme in [0, 1] {
            for dpi in [1., 1.25] {
                w.global::<crate::Colors>().set_preference(theme);
                w.window()
                    .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: dpi,
                    });
                w.window().set_size(slint::LogicalSize::new(800., 600.));
                w.show().unwrap();
                slint::platform::update_timers_and_animations();
                adapter.request_redraw();
                let size = adapter.size();
                let mut pixels =
                    vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                assert!(adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, size.width as usize);
                }));
                assert!(pixels.iter().any(|p| p.b.saturating_sub(p.r) > 30));
                if let Ok(directory) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
                    crate::i18n::write_test_bitmap(
                        &directory,
                        &format!("image-history-{}-{theme}-{dpi}", language.code()),
                        size.width,
                        size.height,
                        pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                    );
                }
            }
        }
    }
    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = actions.clone();
    w.on_navigate(move |direction| observed.borrow_mut().push(direction));
    w.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::DownArrow.into(),
        });
    w.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::DownArrow.into(),
        });
    assert_eq!(&*actions.borrow(), &[1]);
    w.hide().unwrap();
}
