//! Visual regression for the shared editor OCR busy state using bundled translations.
use lexift_ui::{AnnotationToolbar, Colors};
use slint::{
    ComponentHandle,
    platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::{cell::RefCell, rc::Rc};
struct Backend(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
impl Platform for Backend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(w.clone());
        Ok(w)
    }
}
fn bitmap(path: &std::path::Path, width: u32, height: u32, pixels: &[slint::Rgb8Pixel]) {
    let stride = (width * 3 + 3) & !3;
    let mut data = Vec::with_capacity((54 + stride * height) as usize);
    data.extend(b"BM");
    data.extend((54 + stride * height).to_le_bytes());
    data.extend([0; 4]);
    data.extend(54u32.to_le_bytes());
    data.extend(40u32.to_le_bytes());
    data.extend(width.to_le_bytes());
    data.extend(height.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(24u16.to_le_bytes());
    data.extend([0; 24]);
    for y in (0..height).rev() {
        for x in 0..width {
            let p = pixels[(y * width + x) as usize];
            data.extend([p.b, p.g, p.r]);
        }
        data.extend(std::iter::repeat_n(0, (stride - width * 3) as usize));
    }
    std::fs::write(path, data).unwrap();
}
#[test]
fn shared_editor_ocr_busy_state_keeps_layout_in_all_languages_themes_and_dpi() {
    let windows = Rc::new(RefCell::new(vec![]));
    slint::platform::set_platform(Box::new(Backend(windows.clone()))).unwrap();
    let w = AnnotationToolbar::new().unwrap();
    let adapter = windows.borrow()[0].clone();
    w.set_screenshot_mode(true);
    w.set_image_edit_mode(true);
    let logical_width = w.get_toolbar_width();
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        slint::select_bundled_translation(language.code()).unwrap();
        for theme in [0, 1] {
            for scale in [1., 1.25] {
                w.global::<Colors>().set_preference(theme);
                adapter
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                w.set_output_busy(true);
                w.set_recognition_busy(true);
                assert_eq!(w.get_toolbar_width(), logical_width);
                let width = (logical_width * scale).round() as u32;
                let height = (54. * scale).round() as u32;
                adapter.set_size(slint::PhysicalSize::new(width, height));
                w.show().unwrap();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                adapter.request_redraw();
                assert!(adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                }));
                assert!(pixels.windows(2).any(|p| p[0] != p[1]));
                if let Ok(folder) = std::env::var("LEXIFT_EDITOR_OCR_RENDER_DIR") {
                    std::fs::create_dir_all(&folder).unwrap();
                    bitmap(
                        &std::path::Path::new(&folder)
                            .join(format!("{}-{theme}-{scale}.bmp", language.code())),
                        width,
                        height,
                        &pixels,
                    );
                }
                w.set_output_busy(false);
                w.set_recognition_busy(false);
                assert_eq!(w.get_toolbar_width(), logical_width);
            }
        }
    }
    w.hide().unwrap();
}
