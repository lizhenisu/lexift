use super::*;
use lexift_core::domain::{
    runtime_config::HotkeyRegistrations, settings::ThemePreference, ui_language::UiLanguage,
};
use slint::{
    ComponentHandle,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::{cell::Cell, rc::Rc};
struct TestPlatform;
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}
fn ready() -> AppState {
    let mut s = AppState::default();
    s.registered_hotkeys = HotkeyRegistrations {
        translation: Some(s.runtime_config.hotkey),
        annotation: Some(s.runtime_config.annotation_hotkey),
        ocr: Some(s.runtime_config.ocr_hotkey),
        screenshot: Some(s.runtime_config.screenshot_hotkey),
    };
    s
}
#[test]
fn guide_uses_registered_shortcuts_not_drafts_and_keeps_old_keys_after_failure() {
    crate::i18n::select(UiLanguage::EnglishUs);
    let mut state = ready();
    let original = view(&state);
    state.desired_settings.hotkey = "Ctrl+Shift+F8".parse().unwrap();
    state.desired_settings.ocr_hotkey = "Ctrl+Shift+F9".parse().unwrap();
    state.desired_settings.screenshot_hotkey = "Ctrl+F10".parse().unwrap();
    assert_eq!(view(&state), original);
    state.settings_error_field = Some(SettingsField::Hotkey);
    let failed = view(&state);
    assert_eq!(failed.translation.label, "Alt + X");
    assert!(!failed.translation.unavailable);
    assert!(failed.translation.status.contains("previous shortcut"));
    state.registered_hotkeys.translation = Some(state.desired_settings.hotkey);
    state.settings_error_field = None;
    assert_eq!(view(&state).translation.label, "Ctrl + Shift + F8");
    state.settings_error_field = Some(SettingsField::ScreenshotHotkey);
    assert_eq!(view(&state).screenshot.label, "Alt + S");
    assert!(view(&state).screenshot.status.contains("previous shortcut"));
    state.registered_hotkeys.screenshot = None;
    assert!(view(&state).screenshot.unavailable);
    state.registered_hotkeys.screenshot = Some(state.desired_settings.screenshot_hotkey);
    state.settings_error_field = None;
    assert_eq!(view(&state).screenshot.label, "Ctrl + F10");
    state.registered_hotkeys.ocr = None;
    assert!(view(&state).ocr.unavailable);
    state.settings.ocr_enabled = false;
    assert!(view(&state).ocr.status.contains("disabled"));
    state.phase = lexift_core::TranslationPhase::Translating;
    state.translated_text = "not home content".into();
    state.error_message = "not home feedback".into();
    let a = view(&state);
    state.phase = lexift_core::TranslationPhase::Error;
    state.translated_text.push('x');
    assert_eq!(view(&state), a);
}
#[test]
fn guide_only_settings_is_actionable_and_translation_updates_preserve_reading_position() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    crate::i18n::select(UiLanguage::EnglishUs);
    let w = crate::AppWindow::new().unwrap();
    let calls = Rc::new(Cell::new(0));
    let copy = calls.clone();
    w.on_settings_window_requested(move || copy.set(copy.get() + 1));
    let mut state = ready();
    apply(&w, &state);
    w.window().set_size(slint::LogicalSize::new(480., 400.));
    w.show().unwrap();
    assert!(!w.get_guide_two_columns());
    assert!(w.get_guide_content_height() > w.get_guide_viewport_height());
    w.set_guide_scroll_y(-50.);
    state.translated_text = "new translated result".into();
    apply(&w, &state);
    assert_eq!(w.get_guide_scroll_y(), -50.);
    let pointer = |x, y, pressed| {
        w.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            }
        } else {
            WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            }
        })
    };
    pointer(120., 200., true);
    pointer(120., 200., false);
    assert_eq!(calls.get(), 0);
    pointer(420., 42., true);
    pointer(420., 42., false);
    assert_eq!(calls.get(), 1);
    w.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "\n".into() });
    assert_eq!(calls.get(), 2);
}
#[test]
fn guide_renders_all_locales_themes_dpis_and_narrow_windows() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    for language in UiLanguage::ALL {
        for dark in [false, true] {
            for scale in [1., 1.25] {
                for width in [320., 480., 640., 720.] {
                    for stress in [false, true] {
                        crate::i18n::select(language);
                        crate::theme::set_current(if dark {
                            ThemePreference::Dark
                        } else {
                            ThemePreference::Light
                        });
                        let w = crate::AppWindow::new().unwrap();
                        crate::theme::apply(&w);
                        let mut state = ready();
                        if stress {
                            state.registered_hotkeys.translation =
                                Some("Ctrl+Alt+Shift+Win+F12".parse().unwrap());
                            state.settings_error_field = Some(SettingsField::Hotkey);
                            state.registered_hotkeys.annotation = None;
                            state.settings.ocr_enabled = false;
                        }
                        apply(&w, &state);
                        w.window().dispatch_event(WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        });
                        w.window().set_size(slint::PhysicalSize::new(
                            (width * scale) as u32,
                            (640. * scale) as u32,
                        ));
                        assert_eq!(w.get_guide_two_columns(), width >= 640.);
                        assert!(!w.get_guide_heading().is_empty());
                        assert!(w.get_guide_content_height().is_finite());
                        assert!(w.get_guide_content_height() > 300.);
                        assert!(
                            (w.get_guide_content_height() - w.get_guide_cards_bottom()).abs() < 1.,
                            "no footer or reserved footer space"
                        );
                        let image = w.window().take_snapshot().unwrap();
                        if let Ok(dir) = std::env::var("LEXIFT_GUIDE_RENDER") {
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!("{}-{dark}-{scale}-{width}-{stress}-top", language.code()),
                                image.width(),
                                image.height(),
                                image.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                            );
                            w.set_guide_scroll_y(
                                -(w.get_guide_content_height() - w.get_guide_viewport_height())
                                    .max(0.),
                            );
                            let image = w.window().take_snapshot().unwrap();
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!(
                                    "{}-{dark}-{scale}-{width}-{stress}-bottom",
                                    language.code()
                                ),
                                image.width(),
                                image.height(),
                                image.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                            );
                        }
                    }
                }
            }
        }
    }
    crate::i18n::select(UiLanguage::EnglishUs);
    crate::theme::set_current(ThemePreference::Light);
}

#[test]
fn screenshot_registered_key_is_consumed_by_each_settings_recorder() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let w = crate::SettingsWindow::new().unwrap();
    w.set_active_category(2);
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let e = events.clone();
    w.on_hotkey_change_requested(move |_| e.borrow_mut().push(0));
    let e = events.clone();
    w.on_annotation_hotkey_change_requested(move |_| e.borrow_mut().push(1));
    let e = events.clone();
    w.on_ocr_hotkey_change_requested(move |_| e.borrow_mut().push(2));
    let e = events.clone();
    w.on_screenshot_hotkey_change_requested(move |_| e.borrow_mut().push(3));
    for kind in 0..4 {
        w.invoke_toggle_hotkey_capture(kind);
        w.invoke_toggle_hotkey_capture(kind);
        assert!(
            !w.invoke_capture_registered_shortcut("Alt + S".into()),
            "Cancel exits recording"
        );
        w.invoke_toggle_hotkey_capture(kind);
        assert!(w.invoke_capture_registered_shortcut("Alt + S".into()));
        assert!(
            !w.invoke_capture_registered_shortcut("Alt + S".into()),
            "same event is consumed once"
        );
    }
    assert_eq!(*events.borrow(), vec![0, 1, 2, 3]);
}

#[test]
fn screenshot_settings_render_all_locales_themes_and_dpis() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    for language in UiLanguage::ALL {
        for dark in [false, true] {
            for scale in [1., 1.25] {
                for width in [640., 960.] {
                    crate::i18n::select(language);
                    crate::theme::set_current(if dark {
                        ThemePreference::Dark
                    } else {
                        ThemePreference::Light
                    });
                    let w = crate::SettingsWindow::new().unwrap();
                    crate::theme::apply(&w);
                    assert_eq!(w.global::<crate::Colors>().get_dark(), dark);
                    w.set_active_category(2);
                    w.set_draft_hotkey_label("Alt + X".into());
                    w.set_draft_annotation_hotkey_label("Alt + A".into());
                    w.set_draft_ocr_hotkey_label("Alt + T".into());
                    w.set_draft_screenshot_hotkey_label("Alt + S".into());
                    w.set_screenshot_hotkey_status(crate::i18n::tr("Registered").into());
                    w.window().dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                    w.window().set_size(slint::LogicalSize::new(width, 640.));
                    w.show().unwrap();
                    for bottom in [false, true] {
                        if bottom {
                            w.window().dispatch_event(WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(width - 80., 350.),
                                delta_x: 0.,
                                delta_y: -4000.,
                            });
                        }
                        let image = w.window().take_snapshot().unwrap();
                        assert!(image.width() > 0 && image.height() > 0);
                        if let Ok(dir) = std::env::var("LEXIFT_GUIDE_RENDER") {
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!(
                                    "settings-{}-{dark}-{scale}-{width}-{bottom}",
                                    language.code()
                                ),
                                image.width(),
                                image.height(),
                                image.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                            );
                        }
                    }
                }
            }
        }
    }
    crate::i18n::select(UiLanguage::EnglishUs);
    crate::theme::set_current(ThemePreference::Light);
}
