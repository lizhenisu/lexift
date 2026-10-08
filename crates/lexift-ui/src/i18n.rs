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
pub(crate) fn language() -> UiLanguage {
    CURRENT.get()
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

/// Shared software/native-canvas raster fixture output, enabled only in tests.
#[cfg(test)]
pub(crate) fn write_test_bitmap(
    directory: &str,
    name: &str,
    width: u32,
    height: u32,
    pixels: impl IntoIterator<Item = [u8; 4]>,
) {
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
    for [r, g, b, a] in pixels {
        bmp.extend_from_slice(&[b, g, r, a]);
    }
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        std::path::Path::new(&directory).join(format!("{name}.bmp")),
        bmp,
    )
    .unwrap();
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
        write_test_bitmap(
            &directory,
            name,
            width,
            height,
            pixels.into_iter().map(|p| [p.r, p.g, p.b, 255]),
        );
    }
    #[test]
    fn settings_toggle_targets_keyboard_and_collapsed_drafts_are_preserved() {
        use slint::platform::{PointerEventButton, WindowEvent};
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        adapter.set_size(slint::PhysicalSize::new(960, 1000));
        settings.set_active_category(0);
        settings.set_selection_toolbar(false);
        let draw = || {
            adapter.request_redraw();
            adapter.draw_if_needed(|renderer| {
                renderer.render(&mut vec![slint::Rgb8Pixel::default(); 960 * 1000], 960);
            });
        };
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            settings
                .window()
                .dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
            settings
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        };
        draw();
        click(350., 676.);
        assert!(
            settings.get_selection_toolbar(),
            "the label is a toggle target"
        );
        assert!(
            !settings.get_launch_at_login(),
            "adjacent rows must stay unchanged"
        );
        settings
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
        settings
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
        assert!(
            !settings.get_selection_toolbar(),
            "label click focuses the switch for keyboard use"
        );
        // Core state refreshes must still reach the control after a pointer operation.
        settings.set_selection_toolbar(true);
        draw();
        click(890., 676.);
        assert!(!settings.get_selection_toolbar());
        settings.set_active_category(1);
        settings.set_llm_translation_provider_index(1);
        settings.set_llm_prompt("A draft with {target_language}".into());
        settings.set_llm_temperature_custom(true);
        settings.set_llm_temperature(73);
        for language in UiLanguage::ALL {
            slint::select_bundled_translation(language.code()).unwrap();
            for (name, expanded) in [("compact", false), ("expanded", true)] {
                settings.set_llm_parameters_expanded(expanded);
                settings.set_llm_prompt_expanded(expanded);
                settings.invoke_scroll_model_settings(false);
                snapshot(
                    &adapter,
                    &format!("{}-model-{name}", language.code()),
                    960,
                    1000,
                );
                assert_eq!(settings.get_llm_prompt(), "A draft with {target_language}");
                assert_eq!(settings.get_llm_temperature(), 73);
            }
        }
        slint::select_bundled_translation("en-US").unwrap();
        settings.set_translation_settings_open(false);
        settings.invoke_open_translation_settings();
        draw();
        assert!(settings.get_translation_settings_open());
        assert_eq!(settings.get_llm_temperature(), 73);
        settings.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        assert!(
            !settings.get_translation_settings_open(),
            "Escape closes the settings modal"
        );
    }

    #[test]
    fn navigation_and_visual_choices_have_independent_keyboard_targets() {
        use slint::Model;
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        use std::cell::Cell;
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        slint::select_bundled_translation("zh-CN").unwrap();
        adapter.set_size(slint::PhysicalSize::new(960, 1000));
        let theme = Rc::new(Cell::new(-1));
        let chosen = theme.clone();
        settings.on_theme_selected(move |index| chosen.set(index));
        let draw = || {
            slint::platform::update_timers_and_animations();
            adapter.request_redraw();
            adapter.draw_if_needed(|renderer| {
                renderer.render(&mut vec![slint::Rgb8Pixel::default(); 960 * 1000], 960);
            });
        };
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            settings
                .window()
                .dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
            settings
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
            draw();
        };
        let key = |text: slint::SharedString| {
            settings
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            settings
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
            draw();
        };
        draw();
        click(70., 120.);
        key(Key::DownArrow.into());
        assert_eq!(
            settings.get_active_category(),
            0,
            "moving focus does not activate a page"
        );
        snapshot(&adapter, "navigation-browse-focus", 960, 1000);
        key(Key::Return.into());
        assert_eq!(settings.get_active_category(), 2);
        key(Key::DownArrow.into());
        key(" ".into());
        assert_eq!(settings.get_active_category(), 1);
        click(70., 120.);
        click(300., 250.);
        assert_eq!(theme.get(), 0);
        key(Key::Tab.into());
        snapshot(&adapter, "theme-keyboard-focus", 960, 1000);
        key(" ".into());
        assert_eq!(
            theme.get(),
            1,
            "Tab reaches the next theme without changing it until Space"
        );
        settings.set_provider_enabled_states(slint::ModelRc::new(slint::VecModel::from(vec![
            true, false, true, true,
        ])));
        settings.set_active_category(5);
        draw();
        click(270., 115.);
        key(Key::Tab.into());
        assert_eq!(settings.get_provider_detail_index(), 0);
        key(" ".into());
        assert_eq!(settings.get_provider_detail_index(), 1);
        assert!(
            !settings.get_provider_enabled_states().row_data(1).unwrap(),
            "selecting a provider is not enabling it"
        );
        snapshot(&adapter, "provider-keyboard-focus", 960, 1000);
        slint::select_bundled_translation("en-US").unwrap();
    }

    #[test]
    fn settings_menus_browse_without_saving_and_restore_keyboard_focus() {
        use slint::platform::{Key, WindowEvent};
        use std::cell::Cell;
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        slint::select_bundled_translation("en-US").unwrap();
        adapter.set_size(slint::PhysicalSize::new(960, 1000));
        settings.set_active_category(1);
        settings.set_llm_translation_provider_index(1);
        settings.set_llm_selected_model("model-0".into());
        settings.set_translation_choices(slint::ModelRc::new(slint::VecModel::from(
            (0..12)
                .map(|i| crate::TranslationChoice {
                    provider: 1,
                    id: format!("model-{i}").into(),
                    usable: true,
                    top: i * 48,
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        settings.set_translation_list_height(12 * 48);
        let selected = Rc::new(Cell::new(0));
        let count = selected.clone();
        settings.on_translation_selected(move |_| count.set(count.get() + 1));
        let weak = settings.as_weak();
        settings.on_translation_move(move |delta| {
            if let Some(w) = weak.upgrade() {
                w.set_translation_highlight((w.get_translation_highlight() + delta).rem_euclid(12));
            }
        });
        let draw = || {
            slint::platform::update_timers_and_animations();
            adapter.request_redraw();
            adapter.draw_if_needed(|renderer| {
                let size = adapter.size();
                renderer.render(
                    &mut vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize],
                    size.width as usize,
                );
            });
        };
        let key = |text: slint::SharedString| {
            settings
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            settings
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
            draw();
        };
        draw();
        settings.invoke_open_translation_selector();
        draw();
        key(Key::DownArrow.into());
        assert_eq!(selected.get(), 0);
        assert_eq!(
            settings.get_llm_selected_model(),
            "model-0",
            "browsing is not saving"
        );
        key(Key::Escape.into());
        assert!(!settings.get_translation_selector_open());
        key(Key::Return.into());
        assert!(
            settings.get_translation_selector_open(),
            "Escape restores trigger focus"
        );
        key(Key::DownArrow.into());
        key(Key::Return.into());
        assert_eq!(selected.get(), 1);
        settings.set_translation_selector_open(false);
        // Manual settings overlays use the same keyboard behavior and return focus.
        settings.set_active_category(0);
        adapter.set_size(slint::PhysicalSize::new(580, 1000));
        draw();
        let language = Rc::new(std::cell::Cell::new(-1));
        let committed = language.clone();
        settings.on_ui_language_selected(move |index| committed.set(index));
        settings.invoke_focus_app_language();
        key(Key::Return.into());
        key(Key::End.into());
        assert_eq!(language.get(), -1);
        key(Key::Escape.into());
        key(Key::Return.into());
        key(Key::End.into());
        key(Key::Return.into());
        assert_eq!(language.get(), 11);
    }

    #[test]
    fn sampling_rows_keep_independent_targets_in_narrow_and_scaled_layouts() {
        use slint::platform::{PointerEventButton, WindowEvent};
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        slint::select_bundled_translation("en-US").unwrap();
        settings.set_active_category(1);
        settings.set_llm_translation_provider_index(1);
        settings.set_llm_parameters_expanded(true);
        settings.set_llm_reasoning_options(slint::ModelRc::new(slint::VecModel::from(vec![
            "Off".into(),
            "Low".into(),
            "High".into(),
        ])));
        settings.set_llm_reasoning_index(0);
        for (scale, logical_width) in [(1., 960), (1.25, 580)] {
            settings
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let width = (logical_width as f32 * scale) as u32;
            let height = (1100. * scale) as u32;
            adapter.set_size(slint::PhysicalSize::new(width, height));
            settings.set_translation_settings_open(true);
            settings.set_llm_temperature_custom(true);
            settings.set_llm_top_p_custom(true);
            settings.set_llm_temperature_available(true);
            settings.invoke_scroll_model_settings(false);
            let render = || {
                slint::platform::update_timers_and_animations();
                adapter.request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                });
                pixels
            };
            let pixels = render();
            snapshot(&adapter, &format!("sampling-probe-{scale}"), width, height);
            // Find the two colored switch interiors; sliders end before this column.
            let switch_x =
                (logical_width as f32 + 620.0_f32.min(logical_width as f32 - 32.0)) / 2.0 - 76.0;
            let x = (switch_x * scale) as usize;
            let color = settings.global::<crate::Colors>().get_primary();
            let matches = |p: slint::Rgb8Pixel| {
                p.r == color.red() && p.g == color.green() && p.b == color.blue()
            };
            let mut runs = Vec::new();
            let mut start = None;
            for y in 0..height as usize {
                if matches(pixels[y * width as usize + x]) {
                    start.get_or_insert(y);
                } else if let Some(begin) = start.take()
                    && y - begin > (20. * scale) as usize
                {
                    runs.push((begin + y) / 2);
                }
            }
            assert_eq!(
                runs.len(),
                2,
                "both switches remain fully visible at scale {scale}"
            );
            let click = |y: usize| {
                let position = slint::LogicalPosition::new(switch_x, y as f32 / scale);
                settings
                    .window()
                    .dispatch_event(WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    });
                settings
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
            };
            click(runs[0]);
            assert!(!settings.get_llm_temperature_custom());
            assert!(
                settings.get_llm_top_p_custom(),
                "temperature must not toggle Top-P"
            );
            settings.set_llm_temperature_available(false);
            render();
            click(runs[0]);
            assert!(
                !settings.get_llm_temperature_custom(),
                "disabled switch cannot toggle"
            );
            settings.set_llm_temperature_available(true);
            for language in UiLanguage::ALL {
                slint::select_bundled_translation(language.code()).unwrap();
                settings.set_translation_settings_open(true);
                settings.set_llm_temperature_custom(true);
                settings.invoke_scroll_model_settings(false);
                snapshot(
                    &adapter,
                    &format!("{}-responsive-parameters-{scale}", language.code()),
                    width,
                    height,
                );
                for page in [0, 2] {
                    settings.set_active_category(page);
                    snapshot(
                        &adapter,
                        &format!("{}-responsive-page-{page}-{scale}", language.code()),
                        width,
                        height,
                    );
                }
                settings.set_active_category(1);
            }
            slint::select_bundled_translation("en-US").unwrap();
        }
    }

    #[test]
    fn settings_button_rings_follow_keyboard_focus_not_mouse_click_history() {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        let last = Rc::new(RefCell::new(None));
        struct IncrementalPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for IncrementalPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let adapter = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
                *self.0.borrow_mut() = Some(adapter.clone());
                Ok(adapter)
            }
        }
        slint::platform::set_platform(Box::new(IncrementalPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        slint::select_bundled_translation("en-US").unwrap();
        settings.set_active_category(5);
        settings.set_credential_configured(true);
        let actions = Rc::new(std::cell::Cell::new(0));
        let count = actions.clone();
        settings.on_credential_reveal_requested(move || count.set(count.get() + 1));
        let count = actions.clone();
        settings.on_credential_copy_requested(move || count.set(count.get() + 1));
        let count = actions.clone();
        settings.on_credential_edit_requested(move || count.set(count.get() + 1));
        for scale in [1., 1.25] {
            settings
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let (width, height) = ((960. * scale) as u32, (1000. * scale) as u32);
            adapter.set_size(slint::PhysicalSize::new(width, height));
            let framebuffer =
                RefCell::new(vec![slint::Rgb8Pixel::default(); (width * height) as usize]);
            let draw = || {
                slint::platform::update_timers_and_animations();
                adapter.request_redraw();
                let mut pixels = framebuffer.borrow_mut();
                adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                });
                pixels.clone()
            };
            let click = |x, y| {
                let position = slint::LogicalPosition::new(x, y);
                settings
                    .window()
                    .dispatch_event(WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    });
                settings
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
                settings.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(500., 700.),
                });
            };
            let rings = |pixels: &[slint::Rgb8Pixel], index: usize| {
                let left = ((804. + index as f32 * 32.) * scale) as usize;
                let right = ((836. + index as f32 * 32.) * scale) as usize;
                ((196. * scale) as usize..(228. * scale) as usize)
                    .flat_map(|y| (left..right).map(move |x| y * width as usize + x))
                    .filter(|i| {
                        let p = pixels[*i];
                        p.b.saturating_sub(p.r) > 60 && p.g > 50 && p.b > 150
                    })
                    .count()
            };
            draw();
            let before = actions.get();
            for index in 0..4 {
                click(820. + index as f32 * 32., 212.);
                if index == 3 {
                    assert!(settings.get_credential_delete_dialog());
                    settings.invoke_finish_credential_delete(false);
                }
                let pixels = draw();
                for button in 0..4 {
                    assert_eq!(
                        rings(&pixels, button),
                        0,
                        "mouse ring {index}/{button} at {scale}"
                    );
                }
            }
            assert_eq!(
                actions.get(),
                before + 3,
                "all three non-destructive actions were hit"
            );
            if let Ok(directory) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
                super::write_test_bitmap(
                    &directory,
                    &format!("button-mouse-history-{scale}"),
                    width,
                    height,
                    draw().into_iter().map(|p| [p.r, p.g, p.b, 255]),
                );
            }
            click(852., 212.);
            draw();
            settings.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Tab.into(),
            });
            settings.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Tab.into(),
            });
            let pixels = draw();
            if let Ok(directory) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
                super::write_test_bitmap(
                    &directory,
                    &format!("button-keyboard-focus-{scale}"),
                    width,
                    height,
                    pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                );
            }
            assert!(
                rings(&pixels, 2) > 20,
                "Tab reveals the next button focus at {scale}"
            );
            assert_eq!(rings(&pixels, 1), 0);
            settings.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Shift.into(),
            });
            settings.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Tab.into(),
            });
            settings.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Tab.into(),
            });
            settings.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Shift.into(),
            });
            let pixels = draw();
            assert!(rings(&pixels, 1) > 20, "Shift+Tab returns focus to copy");
            assert_eq!(rings(&pixels, 2), 0);
            settings.invoke_select_category(0);
            settings.invoke_select_category(5);
            let pixels = draw();
            for button in 0..4 {
                assert_eq!(rings(&pixels, button), 0, "old page focus must not revive");
            }
            click(852., 212.);
            let pixels = draw();
            for button in 0..4 {
                assert_eq!(rings(&pixels, button), 0);
            }
            // Replacing a focused conditional editor may trigger Slint fallback
            // traversal. Without a user key it must not enter keyboard mode.
            settings.set_credential_editing(true);
            draw();
            settings.invoke_focus_credential_edit();
            draw();
            settings.set_credential_editing(false);
            let pixels = draw();
            for button in 0..4 {
                assert_eq!(rings(&pixels, button), 0);
            }
            let nav_edge =
                pixels[(264. * scale) as usize * width as usize + (50. * scale) as usize];
            assert!(
                nav_edge.b.saturating_sub(nav_edge.r) < 60,
                "automatic focus fallback must not reveal a navigation outline"
            );
        }
    }

    #[test]
    fn settings_scrollbar_appears_only_when_content_overflows() {
        use slint::platform::WindowEvent;
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        settings.set_active_category(0);
        for scale in [1.0, 1.25] {
            settings
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            for (logical_height, overflow) in [(1000., false), (320., true)] {
                let width = (960. * scale) as u32;
                let height = (logical_height * scale) as u32;
                adapter.set_size(slint::PhysicalSize::new(width, height));
                slint::platform::update_timers_and_animations();
                adapter.request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                });
                let thumb_x = width - (8. * scale) as u32;
                let margin_x = width - (20. * scale) as u32;
                let contrasting = ((90. * scale) as u32..height - (12. * scale) as u32)
                    .filter(|y| {
                        let thumb = pixels[(y * width + thumb_x) as usize];
                        let margin = pixels[(y * width + margin_x) as usize];
                        thumb.r.abs_diff(margin.r) > 15
                            || thumb.g.abs_diff(margin.g) > 15
                            || thumb.b.abs_diff(margin.b) > 15
                    })
                    .count();
                assert_eq!(
                    contrasting > 20,
                    overflow,
                    "scrollbar visibility at scale {scale} and height {logical_height}"
                );
                snapshot(
                    &adapter,
                    &format!("general-scrollbar-{scale}-{logical_height}"),
                    width,
                    height,
                );
            }
        }
    }

    #[test]
    fn cancelled_settings_close_restores_existing_editor_and_insertion_point() {
        use slint::platform::{Key, WindowEvent};
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        adapter.set_size(slint::PhysicalSize::new(960, 1000));
        settings.set_llm_translation_provider_index(1);
        settings.set_llm_prompt_expanded(true);
        settings.set_llm_header_editing(true);
        let key = |text: slint::SharedString| {
            settings
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            settings
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
            slint::platform::update_timers_and_animations();
        };
        for scale in [1.0, 1.25] {
            settings
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let width = (960.0 * scale) as u32;
            let height = (1000.0 * scale) as u32;
            adapter.set_size(slint::PhysicalSize::new(width, height));
            for target in [4, 5, 6, 7] {
                settings.set_active_category(if target == 4 { 1 } else { 5 });
                settings.set_llm_advanced(target != 4);
                settings.set_translation_settings_open(target == 4);
                settings.set_llm_prompt("abcd".into());
                settings.set_llm_base_url("abcd".into());
                settings.set_llm_header_name("abcd".into());
                settings.set_llm_header_value("abcd".into());
                // Conditional editors are materialized by layout before a real user can focus them.
                adapter.request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                });
                settings.invoke_focus_settings_editor(target);
                slint::platform::update_timers_and_animations();
                key(Key::Home.into());
                key(Key::RightArrow.into());
                if target % 2 != 0 {
                    settings.window().dispatch_event(WindowEvent::KeyPressed {
                        text: Key::Shift.into(),
                    });
                    key(Key::RightArrow.into());
                    settings.window().dispatch_event(WindowEvent::KeyReleased {
                        text: Key::Shift.into(),
                    });
                }
                settings.invoke_confirm_settings_close();
                assert!(settings.get_settings_close_dialog());
                // Both supported safe defaults must return to the original editor.
                key(if target % 2 == 0 {
                    Key::Escape.into()
                } else {
                    Key::Return.into()
                });
                assert!(!settings.get_settings_close_dialog());
                key("X".into());
                let fields = [
                    settings.get_llm_prompt(),
                    settings.get_llm_base_url(),
                    settings.get_llm_header_name(),
                    settings.get_llm_header_value(),
                ];
                for (index, value) in fields.iter().enumerate() {
                    assert_eq!(
                        value.as_str(),
                        if index as i32 + 4 == target {
                            if target % 2 == 0 { "aXbcd" } else { "aXcd" }
                        } else {
                            "abcd"
                        },
                        "cancelled close must retain the insertion point of editor {target}"
                    );
                }
                snapshot(
                    &adapter,
                    &format!("close-return-editor-{target}-{scale}"),
                    width,
                    height,
                );
            }
        }
    }

    #[test]
    fn credential_deletion_requires_confirmation_and_prompt_reset_is_reversible() {
        use slint::platform::WindowEvent;
        use std::cell::Cell;
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let settings = crate::SettingsWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        adapter.set_size(slint::PhysicalSize::new(960, 1000));
        let removed = Rc::new(Cell::new(0));
        let count = removed.clone();
        settings.on_credential_remove_requested(move || count.set(count.get() + 1));
        let weak = settings.as_weak();
        settings.on_llm_reset_prompt(move || {
            weak.upgrade()
                .unwrap()
                .set_llm_prompt(lexift_core::domain::llm::DEFAULT_TRANSLATION_PROMPT.into());
        });
        settings.set_active_category(5);
        settings.set_provider_detail_index(3);
        settings.set_llm_service_index(2);
        settings.set_deepseek_environment_key(true);
        settings.invoke_request_credential_delete();
        assert!(
            !settings.get_credential_delete_dialog(),
            "environment-only keys cannot be removed"
        );
        settings.set_llm_key_configured(true);
        settings.invoke_request_credential_delete();
        assert!(settings.get_credential_delete_dialog());
        assert_eq!(removed.get(), 0);
        // The safe default button is Cancel, including Enter on initial focus.
        for event in [
            WindowEvent::KeyPressed {
                text: slint::platform::Key::Return.into(),
            },
            WindowEvent::KeyReleased {
                text: slint::platform::Key::Return.into(),
            },
        ] {
            settings.window().dispatch_event(event);
        }
        assert!(!settings.get_credential_delete_dialog());
        assert_eq!(removed.get(), 0);
        settings.invoke_request_credential_delete();
        settings.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        assert!(!settings.get_credential_delete_dialog());
        assert_eq!(removed.get(), 0);
        settings.invoke_request_credential_delete();
        settings.set_provider_detail_index(2);
        settings.invoke_finish_credential_delete(true);
        assert_eq!(
            removed.get(),
            0,
            "a stale confirmation cannot delete another provider's key"
        );
        settings.set_provider_detail_index(3);
        settings.invoke_request_credential_delete();
        settings.invoke_finish_credential_delete(true);
        settings.invoke_finish_credential_delete(true);
        assert_eq!(removed.get(), 1, "confirmation dispatches exactly once");
        settings.set_credential_busy(true);
        settings.invoke_request_credential_delete();
        assert!(!settings.get_credential_delete_dialog());
        settings.set_credential_busy(false);
        settings.set_llm_prompt("Custom {target_language} prompt".into());
        settings.invoke_reset_prompt_with_undo();
        assert!(settings.get_prompt_can_undo_reset());
        settings.invoke_undo_prompt_reset();
        assert_eq!(settings.get_llm_prompt(), "Custom {target_language} prompt");
        assert!(!settings.get_prompt_can_undo_reset());
        settings.invoke_reset_prompt_with_undo();
        settings.set_llm_prompt("New edits {target_language}".into());
        slint::platform::update_timers_and_animations();
        assert!(!settings.get_prompt_can_undo_reset());
        settings.invoke_undo_prompt_reset();
        assert_eq!(settings.get_llm_prompt(), "New edits {target_language}");
        for language in UiLanguage::ALL {
            slint::select_bundled_translation(language.code()).unwrap();
            for (scale, dark, width) in [(1., false, 960), (1.25, true, 725)] {
                settings
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                settings
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                settings.set_active_category(5);
                settings.invoke_request_credential_delete();
                snapshot(
                    &adapter,
                    &format!("{}-delete-key-{scale}", language.code()),
                    width,
                    1000,
                );
                settings.invoke_finish_credential_delete(false);
                settings.set_active_category(1);
                settings.set_llm_translation_provider_index(1);
                settings.set_llm_prompt("Custom {target_language} prompt".into());
                settings.invoke_reset_prompt_with_undo();
                settings.set_llm_prompt_expanded(true);
                settings.invoke_scroll_model_settings(false);
                snapshot(
                    &adapter,
                    &format!("{}-prompt-reset-{scale}", language.code()),
                    width,
                    1000,
                );
                settings.invoke_undo_prompt_reset();
            }
        }
        settings.invoke_request_credential_delete();
        settings.invoke_reset_settings_view();
        assert!(!settings.get_credential_delete_dialog());
        assert!(!settings.get_prompt_can_undo_reset());
        slint::select_bundled_translation("en-US").unwrap();
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
        let tray = crate::TrayMenuWindow::new().unwrap();
        let tray_adapter = last.borrow().as_ref().unwrap().clone();
        let selection = crate::SelectionToolbarWindow::new().unwrap();
        let canvas = crate::AnnotationCanvas::new().unwrap();
        let canvas_adapter = last.borrow().as_ref().unwrap().clone();
        popup.set_source_text("Unchanged input".into());
        popup.set_translated_text("Unchanged result".into());
        panel.set_values(crate::AnnotationValues {
            size: 12,
            text: "Keep watermark".into(),
            ..Default::default()
        });
        for language in UiLanguage::ALL {
            select(language);
            slint::select_bundled_translation(language.code()).unwrap();
            assert_eq!(popup.get_source_text(), "Unchanged input");
            assert_eq!(panel.get_values().text, "Keep watermark");
            assert_eq!(panel.get_values().size, 12);
            assert_eq!(popup.get_translated_text(), "Unchanged result");
            let labels = tray_labels();
            assert!(!labels.settings.is_empty());
            tray.set_open_label(labels.open.into());
            tray.set_settings_label(labels.settings.into());
            tray.set_quit_label(labels.quit.into());
            for dark in [false, true] {
                tray.global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                tray.set_selected(if dark { 1 } else { -1 });
                assert!(tray.get_menu_width() >= 236.);
                snapshot(
                    &tray_adapter,
                    &format!("{}-tray-{dark}", language.code()),
                    tray.get_menu_width().ceil() as u32,
                    tray.get_menu_height().ceil() as u32,
                );
            }
            assert_eq!(tr("unknown fallback"), "unknown fallback");
            settings.set_appearance_language_index(language.index());
            for (width, dark) in [(960, false), (580, true)] {
                settings
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                for page in [0, 2, 1, 5] {
                    settings.set_active_category(page);
                    snapshot(
                        &settings_adapter,
                        &format!("{}-settings-{page}-{width}", language.code()),
                        width,
                        1000,
                    );
                }
            }
            if matches!(
                language,
                UiLanguage::EnglishUs | UiLanguage::SimplifiedChinese | UiLanguage::German
            ) {
                settings.set_active_category(1);
                settings.global::<crate::Colors>().set_preference(0);
                snapshot(
                    &settings_adapter,
                    &format!("{}-translation-full", language.code()),
                    960,
                    1500,
                );
                settings.global::<crate::Colors>().set_preference(1);
                snapshot(
                    &settings_adapter,
                    &format!("{}-translation-narrow-full", language.code()),
                    580,
                    1800,
                );
                settings.set_active_category(5);
                panel.set_tool(0);
                panel.set_menu(false);
                panel.set_values(crate::AnnotationValues {
                    size: 4,
                    rounding: 21,
                    ..Default::default()
                });
                snapshot(
                    &panel_adapter,
                    &format!("{}-geometry-wide", language.code()),
                    960,
                    68,
                );
                snapshot(
                    &panel_adapter,
                    &format!("{}-geometry-narrow", language.code()),
                    440,
                    158,
                );
                panel.set_tool(2);
                panel.set_values(crate::AnnotationValues {
                    size: 4,
                    rounding: 21,
                    shape: 1,
                    strength: 50.,
                    ..Default::default()
                });
                snapshot(
                    &panel_adapter,
                    &format!("{}-spotlight-wide", language.code()),
                    960,
                    68,
                );
                snapshot(
                    &panel_adapter,
                    &format!("{}-spotlight-narrow", language.code()),
                    440,
                    158,
                );
                toolbar.set_can_undo(true);
                snapshot(
                    &toolbar_adapter,
                    &format!("{}-toolbar-active", language.code()),
                    650,
                    54,
                );
                panel.set_values(crate::AnnotationValues {
                    size: 12,
                    text: "Keep watermark".into(),
                    ..Default::default()
                });
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
            assert_eq!(settings.get_active_category(), 5);
        }
        panel.set_menu(false);
        for tool in [3, 4] {
            panel.set_tool(tool);
            panel.set_values(crate::AnnotationValues {
                size: 12,
                color_index: if tool == 4 { 6 } else { 0 },
                ..Default::default()
            });
            snapshot(&panel_adapter, &format!("brush-{tool}-narrow"), 400, 158);
        }
        for tool in [5, 6, 7] {
            panel.set_tool(tool);
            panel.set_values(crate::AnnotationValues {
                size: if tool == 7 { 2 } else { 5 },
                zoom: 150,
                head: tool == 5,
                line_end: if tool == 5 { 3 } else { 0 },
                erase: tool == 7,
                antialias: true,
                ..Default::default()
            });
            snapshot(
                &panel_adapter,
                &format!("line-{tool}-wide"),
                if tool == 7 {
                    1120
                } else if tool == 6 {
                    1030
                } else {
                    960
                },
                58,
            );
            snapshot(
                &panel_adapter,
                &format!("line-{tool}-narrow"),
                500,
                if tool == 7 { 138 } else { 148 },
            );
        }
        let canvas_events = Rc::new(RefCell::new(Vec::new()));
        let recorded = canvas_events.clone();
        canvas.on_pointer(move |kind, _, _| recorded.borrow_mut().push(kind));
        let mut document = lexift_core::domain::annotation::Session::new();
        document.add(lexift_core::domain::annotation::Object {
            erased: Vec::new(),
            id: Default::default(),
            points: Vec::new(),
            bounds: lexift_core::domain::annotation::Bounds::from_corners((20., 20.), (140., 90.)),
            kind: lexift_core::domain::annotation::Kind::Rectangle,
            style: Default::default(),
            extra: Default::default(),
        });
        canvas.set_frame(crate::annotation_render::preview_image(
            &crate::annotation_render::render(
                &document,
                lexift_core::domain::geometry::Rect {
                    left: 0,
                    top: 0,
                    right: 200,
                    bottom: 120,
                },
                1.,
                None,
                None,
            )
            .unwrap(),
        ));
        canvas_adapter.set_size(slint::PhysicalSize::new(200, 120));
        snapshot(&canvas_adapter, "annotation-canvas", 200, 120);
        use slint::platform::{PointerEventButton, WindowEvent};
        let a = slint::LogicalPosition::new(20., 20.);
        let b = slint::LogicalPosition::new(140., 90.);
        canvas
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: a });
        canvas.window().dispatch_event(WindowEvent::PointerPressed {
            position: a,
            button: PointerEventButton::Left,
        });
        canvas
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: b });
        canvas
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: b,
                button: PointerEventButton::Left,
            });
        assert!(canvas_events.borrow().contains(&0));
        assert!(canvas_events.borrow().contains(&1));
        assert!(canvas_events.borrow().contains(&2));
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
        drop((
            settings, popup, panel, main, menu, toolbar, selection, canvas, tray,
        ));
        let reopened = crate::SettingsWindow::new().unwrap();
        apply();
        assert_eq!(reopened.get_active_category(), 0);
    }
}
