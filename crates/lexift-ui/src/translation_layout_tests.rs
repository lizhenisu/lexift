use crate::TranslationPopup;
use slint::{
    ComponentHandle,
    platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::rc::Rc;

struct TestPlatform(std::rc::Rc<std::cell::RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(window.clone());
        Ok(window)
    }
}

#[test]
fn long_translation_grows_and_all_lines_remain_scrollable() {
    let adapters = Rc::new(std::cell::RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(TestPlatform(adapters.clone()))).unwrap();
    let popup = TranslationPopup::new().unwrap();
    popup.set_max_popup_height(900.);
    popup.set_translated_text("首行完整显示\n\n".repeat(50).into());
    popup.window().set_size(slint::LogicalSize::new(420., 400.));
    assert!(
        popup.get_result_content_height() > popup.get_result_viewport_height(),
        "long text must create scrollable content"
    );
    let (height, _) = crate::bridge::measured_popup_metrics(&popup);
    assert_eq!(
        height, 900.,
        "long text must use the available work area rather than a fixed cap"
    );
    popup.set_max_popup_height(480.);
    assert_eq!(crate::bridge::measured_popup_metrics(&popup).0, 480.);
    popup.set_max_popup_height(900.);
    popup.set_translated_text("短译文".into());
    assert!(crate::bridge::measured_popup_metrics(&popup).0 < 556.);
    let short_height = popup.get_measured_result_height();
    popup.set_translated_text("一\n\n二\n三\n四".into());
    assert!(popup.get_measured_result_height() > short_height * 3.);
    popup.set_translated_text(
        "中文长译文和 English words，需要在当前宽度正确换行。"
            .repeat(80)
            .into(),
    );
    popup.set_popup_width(640.);
    let wide = popup.get_measured_result_height();
    popup.set_popup_width(340.);
    assert!(popup.get_measured_result_height() > wide);
    popup.set_error_text("长错误提示\n".repeat(80).into());
    assert_eq!(crate::bridge::measured_popup_metrics(&popup).0, 900.);
    popup.set_error_text("".into());
    popup.set_popup_width(420.);
    popup.set_popup_height(900.);
    popup.set_source_card_height(100.);
    let adapter = adapters.borrow()[0].clone();
    adapter.set_size(slint::PhysicalSize::new(420, 390));
    let mut initial_pixels = vec![slint::Rgb8Pixel::default(); 420 * 390];
    adapter.request_redraw();
    adapter.draw_if_needed(|renderer| {
        renderer.render(&mut initial_pixels, 420);
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
    });
    for scale in [1., 1.25] {
        adapter
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (420. * scale) as u32;
        let height = (900. * scale) as u32;
        adapter.set_size(slint::PhysicalSize::new(width, height));
        crate::bridge::invalidate_popup_surface_cache(&popup);
        let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
        for bottom in [false, true] {
            popup.set_result_scroll_y(if bottom {
                -(popup.get_result_content_height() - popup.get_result_viewport_height()).max(0.)
            } else {
                0.
            });
            adapter.request_redraw();
            assert!(adapter.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            }));
            let background = pixels[((height - 25) * width + 12) as usize];
            assert!(
                background.r >= 235 && background.g >= 235 && background.b >= 235,
                "resized surface must paint the background through its bottom edge"
            );
            if let Ok(directory) = std::env::var("LEXIFT_TRANSLATION_SNAPSHOT_DIR") {
                crate::i18n::write_test_bitmap(
                    &directory,
                    &format!("popup-{scale}-bottom-{bottom}"),
                    width,
                    height,
                    pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                );
            }
        }
    }
    let main = crate::AppWindow::new().unwrap();
    main.window().set_size(slint::LogicalSize::new(560., 520.));
    main.set_translated_text("主窗口长译文\n".repeat(200).into());
    assert!(main.get_translation_content_height() > main.get_translation_viewport_height());
}
