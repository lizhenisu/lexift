//! Actual Slint overlay pixels at both supported test scales.
use crate::OcrSelectionWindow;
use slint::{
    ComponentHandle,
    platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
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
fn frozen_pixels_remain_clear_inside_selection_and_dim_outside_at_both_dpis() {
    let slot = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(slot.clone()))).unwrap();
    let view = OcrSelectionWindow::new().unwrap();
    view.set_frozen_image(slint::Image::from_rgba8(
        slint::SharedPixelBuffer::clone_from_slice(&vec![255u8; 400 * 300 * 4], 400, 300),
    ));
    view.set_selecting(true);
    view.set_selection_left(100.);
    view.set_selection_top(100.);
    view.set_selection_width(200.);
    view.set_selection_height(120.);
    view.show().unwrap();
    let window = slot.borrow().as_ref().unwrap().clone();
    for scale in [1., 1.25] {
        view.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        view.window().set_size(slint::LogicalSize::new(400., 300.));
        let size = window.size();
        let mut pixels = vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(&mut pixels, size.width as usize);
        });
        let at = |x: f32, y: f32| {
            pixels[(y * scale) as usize * size.width as usize + (x * scale) as usize]
        };
        assert_eq!(at(150., 150.), slint::Rgb8Pixel::new(255, 255, 255));
        assert!(at(40., 150.).r < 150);
        assert!(at(320., 150.).r < 150);
        assert!(at(150., 250.).r < 150);
    }
}
