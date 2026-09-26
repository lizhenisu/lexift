//! Runtime UI translation from the same bundled PO catalogs used by Slint.
use lexift_core::{domain::ui_language::UiLanguage, ports::tray::TrayMenuLabels};
use std::cell::Cell;
thread_local! { static CURRENT: Cell<UiLanguage> = const { Cell::new(UiLanguage::EnglishUs) }; }
include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

pub(crate) fn select(language: UiLanguage) -> bool {
    let changed = CURRENT.replace(language) != language;
    apply();
    changed
}
/// A component must exist before Slint registers its bundled translation table.
/// Calling this before every first show also handles background-only startup.
pub(crate) fn apply() {
    let _ = slint::select_bundled_translation(CURRENT.get().code());
}
pub(crate) fn tr(key: &str) -> String {
    lookup(CURRENT.get().code(), key).unwrap_or(key).into()
}
pub(crate) fn detail(key: &str, detail: &str) -> String {
    tr(key).replace("{0}", detail)
}
pub(crate) fn error(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    use lexift_core::domain::message::MessageId;
    for (id, key) in [
        (MessageId::Copied, "Copied"),
        (MessageId::EmptyApiKey, "DeepL API key cannot be empty"),
        (MessageId::MissingApiKey, "DeepL API key is not configured"),
        (
            MessageId::EmptyTranslation,
            "Translation text cannot be empty",
        ),
        (MessageId::NothingToRead, "There is no text to read"),
    ] {
        if text == id.id() {
            return tr(key);
        }
    }
    if let Some(value) = lookup(CURRENT.get().code(), text) {
        return value.into();
    }
    detail("Operation failed: {0}", text)
}
pub(crate) fn tray_labels() -> TrayMenuLabels {
    TrayMenuLabels {
        open: tr("Open Lexift"),
        settings: tr("Settings"),
        quit: tr("Quit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{
        ComponentHandle,
        platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        },
    };
    use std::{cell::RefCell, rc::Rc};
    slint::slint! {
        export component NumericProbe inherits Window {
            out property <string> formatted: 1.5;
            public function parse(text: string) -> float { return text.to-float(); }
        }
    }
    struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }
    fn snapshot(window: &Rc<MinimalSoftwareWindow>, name: &str, width: u32, height: u32) {
        let Ok(directory) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") else {
            return;
        };
        window.set_size(slint::PhysicalSize::new(width, height));
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        }));
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        for value in [
            54 + width * height * 4,
            0,
            54,
            40,
            width,
            (-(height as i32)) as u32,
        ] {
            bmp.extend_from_slice(&value.to_le_bytes());
        }
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        for value in [0u32, width * height * 4, 0, 0, 0, 0] {
            bmp.extend_from_slice(&value.to_le_bytes());
        }
        for pixel in pixels {
            bmp.extend_from_slice(&[pixel.b, pixel.g, pixel.r, 255]);
        }
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{name}.bmp")),
            bmp,
        )
        .unwrap();
    }
    #[test]
    fn all_languages_update_existing_windows_without_losing_user_state() {
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let settings_adapter = last.borrow().as_ref().unwrap().clone();
        let popup = crate::TranslationPopup::new().unwrap();
        let popup_adapter = last.borrow().as_ref().unwrap().clone();
        let panel = crate::AnnotationPanel::new().unwrap();
        let panel_adapter = last.borrow().as_ref().unwrap().clone();
        let main = crate::AppWindow::new().unwrap();
        let main_adapter = last.borrow().as_ref().unwrap().clone();
        let menu = crate::PopupLanguageMenuWindow::new().unwrap();
        let menu_adapter = last.borrow().as_ref().unwrap().clone();
        let toolbar = crate::AnnotationToolbar::new().unwrap();
        let toolbar_adapter = last.borrow().as_ref().unwrap().clone();
        let selection = crate::SelectionToolbarWindow::new().unwrap();
        popup.set_source_text("Unchanged input".into());
        popup.set_translated_text("Unchanged result".into());
        settings.set_screenshot_line_width("7".into());
        panel.set_values(crate::AnnotationValues {
            size: 12,
            text: "Keep watermark".into(),
            ..Default::default()
        });
        for language in UiLanguage::ALL {
            select(language);
            slint::select_bundled_translation(language.code()).unwrap();
            assert_eq!(settings.get_screenshot_line_width(), "7");
            assert_eq!(popup.get_source_text(), "Unchanged input");
            assert_eq!(panel.get_values().text, "Keep watermark");
            assert_eq!(panel.get_values().size, 12);
            assert_eq!(popup.get_translated_text(), "Unchanged result");
            assert!(!tray_labels().settings.is_empty());
            assert_eq!(tr("unknown fallback"), "unknown fallback");
            settings.set_appearance_language_index(language.index());
            for (width, dark) in [(960, false), (580, true)] {
                settings
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                for page in 0..5 {
                    settings.set_active_category(page);
                    snapshot(
                        &settings_adapter,
                        &format!("{}-settings-{page}-{width}", language.code()),
                        width,
                        1000,
                    );
                }
            }
            for dark in [false, true] {
                popup
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                panel
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                snapshot(
                    &popup_adapter,
                    &format!("{}-popup-{dark}", language.code()),
                    620,
                    460,
                );
                panel.set_heading(tr("Magnifier").into());
                panel.set_tool(7);
                panel.set_fields(slint::ModelRc::new(slint::VecModel::from(vec![
                    1, 0, 2, 7, 8, 9, 10,
                ])));
                snapshot(
                    &panel_adapter,
                    &format!("{}-annotation-{dark}", language.code()),
                    620,
                    260,
                );
            }
            main.set_translated_text("Unchanged result".into());
            snapshot(
                &main_adapter,
                &format!("{}-main", language.code()),
                720,
                600,
            );
            snapshot(
                &menu_adapter,
                &format!("{}-menu", language.code()),
                250,
                360,
            );
            snapshot(
                &toolbar_adapter,
                &format!("{}-toolbar", language.code()),
                650,
                64,
            );
            assert_eq!(settings.get_active_category(), 4);
        }
        let numeric = NumericProbe::new().unwrap();
        select(UiLanguage::German);
        assert_eq!(numeric.get_formatted(), "1,5");
        assert_eq!(numeric.invoke_parse("1,5".into()), 1.5);
        select(UiLanguage::EnglishUs);
        assert_eq!(numeric.get_formatted(), "1.5");
        assert_eq!(numeric.invoke_parse("1.5".into()), 1.5);
        select(UiLanguage::SimplifiedChinese);
        assert_eq!(tr("Settings"), "设置");
        assert_eq!(
            error(lexift_core::domain::message::MessageId::EmptyApiKey.id()),
            "DeepL API 密钥不能为空"
        );
        assert_eq!(error("HTTP 503"), "操作失败：HTTP 503");
        select(UiLanguage::EnglishUs);
        drop((settings, popup, panel, main, menu, toolbar, selection));
        let reopened = crate::SettingsWindow::new().unwrap();
        apply();
        assert_eq!(reopened.get_screenshot_line_width(), "3");
    }
}
