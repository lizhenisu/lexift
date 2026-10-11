//! Geometry and pixels of the shared Settings hint, using the real component.
use slint::{
    ComponentHandle,
    platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::{cell::RefCell, rc::Rc};
slint::slint! {
    import {SettingsHints, SettingsHintOverlay, SettingsHelpIcon} from "ui/settings-hint.slint";
    export component HintProbe inherits Window {
        out property <length> hint-x: overlay.x;
        out property <length> hint-y: overlay.y;
        out property <length> hint-width: overlay.width;
        out property <length> hint-height: overlay.height;
        out property <string> hint-text: SettingsHints.text;
        public function show-hint(text: string, x: length, y: length) { SettingsHints.show(text,x,y,32px); }
        public function clear-hint() { SettingsHints.clear(); }
        SettingsHelpIcon { x: 40px; y: 60px; text: "Extra recognition takes more time."; accessible-name: "Recognition help"; }
        FocusScope { x: 200px; y: 60px; width: 24px; height: 24px; }
        overlay := SettingsHintOverlay { window-width: root.width; window-height: root.height; }
    }
}

#[test]
fn help_icon_shows_shared_hint_on_hover_and_keyboard_focus_and_clears() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let slot = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(slot.clone()))).unwrap();
    let window = HintProbe::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(400., 240.));
    let adapter = slot.borrow().as_ref().unwrap().clone();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 400 * 240];
    adapter.draw_if_needed(|r| {
        r.render(&mut pixels, 400);
    });
    let position = slint::LogicalPosition::new(52., 72.);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    assert_eq!(window.get_hint_text(), "Extra recognition takes more time.");
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(300., 150.),
    });
    assert!(window.get_hint_text().is_empty());
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(300., 150.),
    });
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
    assert!(
        !window.get_hint_text().is_empty(),
        "keyboard focus exposes help"
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Tab.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: slint::platform::Key::Tab.into(),
    });
    assert!(
        window.get_hint_text().is_empty(),
        "leaving help clears the hint"
    );
    window.invoke_show_hint("obsolete".into(), 40., 60.);
    window.invoke_clear_hint();
    assert!(window.get_hint_text().is_empty());
}
struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.0.borrow_mut() = Some(adapter.clone());
        Ok(adapter)
    }
}
#[test]
fn shared_hint_has_compact_intrinsic_width_wraps_and_flips_inside_window() {
    let slot = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(slot.clone()))).unwrap();
    let window = HintProbe::new().unwrap();
    window.show().unwrap();
    let adapter = slot.borrow().as_ref().unwrap().clone();
    for scale in [1., 1.25] {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        for (width, height) in [(900., 700.), (280., 240.)] {
            window
                .window()
                .set_size(slint::LogicalSize::new(width, height));
            for text in [
                "Filter models".to_string(),
                "model-provider/a-very-long-model-name-with-complete-version-id".repeat(3),
            ] {
                for (x, y) in [(0., 0.), (width - 32., height - 32.)] {
                    window.invoke_show_hint(text.clone().into(), x, y);
                    let size = adapter.size();
                    let mut pixels =
                        vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                    adapter.request_redraw();
                    adapter.draw_if_needed(|r| {
                        r.render(&mut pixels, size.width as usize);
                    });
                    let w = window.get_hint_width();
                    let h = window.get_hint_height();
                    assert!(w <= 480. && w <= width - 16.);
                    assert!(
                        window.get_hint_x() >= 8. && window.get_hint_x() + w <= width - 8.1 + 0.2
                    );
                    assert!(
                        window.get_hint_y() >= 8. && window.get_hint_y() + h <= height - 8. + 0.1
                    );
                    if text == "Filter models" {
                        assert!(w < 160.);
                        assert_eq!(h, 28.);
                    } else {
                        assert!(h > 28.);
                    }
                    if y > 0. {
                        assert!(window.get_hint_y() + h < y);
                    }
                }
            }
        }
    }
    window.invoke_clear_hint();
}
