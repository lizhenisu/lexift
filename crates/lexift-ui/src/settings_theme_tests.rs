//! Exercise the actual Settings controls, rather than a duplicate palette showcase.
use slint::platform::{
    Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.0.borrow_mut() = Some(window.clone());
        Ok(window)
    }
}
fn render(adapter: &MinimalSoftwareWindow, name: &str) -> Vec<slint::Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    adapter.request_redraw();
    let size = adapter.size();
    let mut pixels = vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
    assert!(adapter.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, size.width as usize);
    }));
    if let Ok(directory) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
        crate::i18n::write_test_bitmap(
            &directory,
            name,
            size.width,
            size.height,
            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
        );
    }
    pixels
}
fn blend(color: slint::Color, base: slint::Color) -> [u8; 3] {
    let a = color.alpha() as u32;
    let channel = |v: u8, b: u8| ((v as u32 * a + b as u32 * (255 - a) + 127) / 255) as u8;
    [
        channel(color.red(), base.red()),
        channel(color.green(), base.green()),
        channel(color.blue(), base.blue()),
    ]
}
fn pixel(pixels: &[slint::Rgb8Pixel], scale: f32, x: f32, y: f32, expected: [u8; 3], label: &str) {
    let stride = (960.0 * scale) as usize;
    let p = pixels[(y * scale) as usize * stride + (x * scale) as usize];
    let actual = [p.r, p.g, p.b];
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
        "{label} at {x},{y}, scale {scale}: {actual:?} != {expected:?}"
    );
}
#[test]
fn settings_interaction_colors_follow_theme_across_pages_and_overlays() {
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
    let w = crate::SettingsWindow::new().unwrap();
    slint::select_bundled_translation("en-US").unwrap();
    let adapter = last.borrow().as_ref().unwrap().clone();
    w.set_llm_translation_provider_index(1);
    w.set_llm_selected_model("Translation model".into());
    w.set_translation_choices(ModelRc::new(VecModel::from(vec![
        crate::TranslationChoice {
            provider: 1,
            id: "Translation model".into(),
            group: "Ollama".into(),
            usable: true,
            thinking: true,
            ..Default::default()
        },
    ])));
    w.set_translation_list_height(76);
    let models = ModelRc::new(VecModel::from(vec![crate::ProviderModelRow {
        id: "Translation model".into(),
        enabled: true,
        usable: true,
        text: true,
        thinking: true,
        ..Default::default()
    }]));
    w.set_llm_enabled_catalog(models.clone());
    w.set_llm_catalog(models);
    w.set_llm_catalog_count(1);
    w.set_provider_enabled_states(ModelRc::new(VecModel::from(vec![true, true, true, true])));
    w.set_llm_reasoning_options(ModelRc::new(VecModel::from(vec![
        "Off".into(),
        "High".into(),
    ])));
    w.set_llm_reasoning_index(0);
    w.set_llm_temperature_available(true);
    w.set_llm_top_p_available(true);
    w.set_llm_prompt("Translate to {target_language}.".into());
    w.set_llm_probe_models(ModelRc::new(VecModel::from(vec![
        "Translation model".into(),
    ])));
    w.set_llm_probe_index(0);
    w.set_llm_probe_can_start(true);
    for scale in [1.0, 1.25] {
        w.window().dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        adapter.set_size(slint::PhysicalSize::new(
            (960.0 * scale) as u32,
            (1000.0 * scale) as u32,
        ));
        for theme in [0, 1] {
            w.global::<crate::Colors>().set_preference(theme);
            let colors = w.global::<crate::Colors>();
            let surface = colors.get_surface();
            let selected = blend(colors.get_primary_container(), surface);
            let hover = blend(colors.get_icon_button_hover(), surface);
            let pressed = blend(colors.get_icon_button_pressed(), surface);
            let prefix = format!("settings-colors-{theme}-{}", (scale * 100.0) as i32);
            w.set_active_category(1);
            render(&adapter, "layout");
            w.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(10.0, 900.0),
            });
            let pixels = render(&adapter, &format!("{prefix}-translation"));
            pixel(&pixels, scale, 22.0, 232.0, selected, "selected navigation");
            // Empty padding in the real selector avoids text and logo pixels.
            w.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(549.0, 308.0),
            });
            let pixels = render(&adapter, &format!("{prefix}-hover"));
            pixel(&pixels, scale, 549.0, 308.0, hover, "selector hover");
            w.window().dispatch_event(WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(549.0, 308.0),
                button: PointerEventButton::Left,
            });
            let pixels = render(&adapter, &format!("{prefix}-pressed"));
            pixel(&pixels, scale, 549.0, 308.0, pressed, "selector pressed");
            w.window().dispatch_event(WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(549.0, 308.0),
                button: PointerEventButton::Left,
            });
            let pixels = render(&adapter, &format!("{prefix}-picker"));
            pixel(&pixels, scale, 549.0, 308.0, selected, "expanded selector");
            pixel(&pixels, scale, 462.0, 449.0, selected, "selected model row");
            w.set_translation_selector_open(false);
            w.set_settings_menu_open(true);
            let pixels = render(&adapter, &format!("{prefix}-language"));
            pixel(
                &pixels,
                scale,
                580.0,
                196.0,
                selected,
                "expanded language field",
            );
            w.invoke_close_settings_menu();
            w.set_translation_settings_open(true);
            let pixels = render(&adapter, &format!("{prefix}-translation-modal"));
            let underlying = colors.get_surface();
            pixel(
                &pixels,
                scale,
                10.0,
                700.0,
                blend(
                    if theme == 1 {
                        slint::Color::from_argb_u8(128, 0, 0, 0)
                    } else {
                        slint::Color::from_argb_u8(77, 32, 33, 36)
                    },
                    underlying,
                ),
                "modal scrim",
            );
            w.set_translation_settings_open(false);
            for category in [0, 2, 5] {
                w.set_active_category(category);
                render(&adapter, &format!("{prefix}-page-{category}"));
            }
            w.set_provider_detail_index(1);
            w.set_llm_enabled_count(1);
            render(&adapter, &format!("{prefix}-provider"));
            w.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(820.0, 212.0),
            });
            let pixels = render(&adapter, &format!("{prefix}-button-hover"));
            pixel(&pixels, scale, 812.0, 220.0, hover, "text button hover");
            w.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(920.0, 306.0),
            });
            let pixels = render(&adapter, &format!("{prefix}-icon-hover"));
            pixel(&pixels, scale, 914.0, 294.0, hover, "icon button hover");
            pixel(
                &pixels,
                scale,
                812.0,
                220.0,
                blend(surface, surface),
                "unrelated button stays idle",
            );
            w.window().dispatch_event(WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(920.0, 306.0),
                button: PointerEventButton::Left,
            });
            let pixels = render(&adapter, &format!("{prefix}-icon-pressed"));
            pixel(&pixels, scale, 914.0, 294.0, pressed, "icon button pressed");
            w.window().dispatch_event(WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(920.0, 306.0),
                button: PointerEventButton::Left,
            });
            w.set_llm_advanced(false);
            w.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(10.0, 900.0),
            });
            w.set_llm_search_open(true);
            w.set_llm_filter_open(true);
            render(&adapter, &format!("{prefix}-filters"));
            w.set_llm_search_open(false);
            w.set_llm_filter_open(false);
            let original_catalog = w.get_llm_catalog();
            w.set_llm_catalog(ModelRc::new(VecModel::from(
                (0..20)
                    .map(|i| crate::ProviderModelRow {
                        id: format!("Model {i:02}").into(),
                        enabled: true,
                        usable: true,
                        text: true,
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
            )));
            w.set_llm_catalog_count(20);
            for dialog in 0..3 {
                w.set_llm_advanced(dialog == 0);
                w.set_llm_model_dialog(dialog == 1);
                w.set_llm_probe_dialog(dialog == 2);
                let before = render(&adapter, &format!("{prefix}-dialog-{dialog}"));
                if dialog == 1 {
                    let thumb = blend(colors.get_scrollbar_thumb(), surface);
                    let stride = (960.0 * scale) as usize;
                    let sample = |image: &[slint::Rgb8Pixel], x: f32, y: f32| {
                        image[(y * scale) as usize * stride + (x * scale) as usize]
                    };
                    let y = (300..750)
                        .find(|y| {
                            let p = sample(&before, 811.0, *y as f32);
                            [p.r, p.g, p.b]
                                .iter()
                                .zip(thumb)
                                .all(|(a, b)| a.abs_diff(b) <= 2)
                        })
                        .expect("overflowing model list must use the app theme scrollbar");
                    let start = slint::LogicalPosition::new(811.0, y as f32 + 2.0);
                    w.window()
                        .dispatch_event(WindowEvent::PointerMoved { position: start });
                    w.window().dispatch_event(WindowEvent::PointerPressed {
                        position: start,
                        button: PointerEventButton::Left,
                    });
                    w.window().dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(811.0, 750.0),
                    });
                    w.window().dispatch_event(WindowEvent::PointerReleased {
                        position: slint::LogicalPosition::new(811.0, 750.0),
                        button: PointerEventButton::Left,
                    });
                    let after = render(&adapter, &format!("{prefix}-models-scrolled"));
                    let changed = (330..740)
                        .flat_map(|y| (160..780).map(move |x| (x, y)))
                        .filter(|(x, y)| {
                            let a = sample(&before, *x as f32, *y as f32);
                            let b = sample(&after, *x as f32, *y as f32);
                            (a.r, a.g, a.b) != (b.r, b.g, b.b)
                        })
                        .count();
                    assert!(
                        changed > 100,
                        "dragging the themed scrollbar must move model rows"
                    );
                    w.window().dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(10.0, 900.0),
                    });
                }
            }
            w.set_llm_probe_dialog(false);
            w.set_llm_catalog(original_catalog);
            w.set_llm_catalog_count(1);
        }
    }
    // System resolution must update every semantic interaction role too.
    w.global::<crate::Colors>().set_preference(2);
    for scheme in [
        i_slint_core::items::ColorScheme::Dark,
        i_slint_core::items::ColorScheme::Light,
    ] {
        i_slint_core::context::with_global_context(
            || unreachable!(),
            |context| context.set_color_scheme(scheme),
        )
        .unwrap();
        w.set_llm_model_dialog(false);
        w.set_llm_advanced(false);
        w.set_active_category(1);
        let pixels = render(&adapter, "system-resolution");
        let colors = w.global::<crate::Colors>();
        pixel(
            &pixels,
            1.25,
            22.0,
            232.0,
            blend(colors.get_primary_container(), colors.get_surface()),
            "system selected navigation",
        );
    }
}
