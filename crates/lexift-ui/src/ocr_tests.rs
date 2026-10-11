//! Actual Slint overlay pixels at both supported test scales.
use crate::OcrSelectionWindow;
use slint::{
    ComponentHandle, Model,
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

#[test]
fn provider_resource_page_is_separate_and_renders_all_locales() {
    use crate::SettingsWindow;
    use lexift_core::{
        AppEvent, AppState,
        domain::{
            ocr::ResourceOperation, service_provider::ServiceProviderId, settings::SettingsChange,
            ui_language::UiLanguage,
        },
    };
    let slot = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(slot.clone()))).unwrap();
    let view = SettingsWindow::new().unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let sent = events.clone();
    crate::llm_settings::wire(
        &view,
        Default::default(),
        None,
        Rc::new(move |e| sent.borrow_mut().push(e)),
    );
    let sent = events.clone();
    crate::ocr::provider::wire_settings(&view, Rc::new(move |event| sent.borrow_mut().push(event)));
    view.set_provider_detail_index(1);
    view.set_llm_service_index(0);
    view.invoke_llm_service_selected(1);
    view.set_llm_base_url("http://unsaved.example/v1".into());
    view.invoke_llm_base_edited();
    view.set_provider_detail_index(4);
    view.invoke_llm_service_selected(4);
    view.set_active_category(5);
    let mut state = AppState::default();
    state.reduce(AppEvent::Ocr(
        lexift_core::domain::ocr::OcrEvent::ManageResources(ResourceOperation::Check),
    ));
    crate::ocr::provider::apply_settings(&view, &state);
    assert_eq!(view.get_ocr_language_index(), 0);
    state.desired_settings.ocr_language = None;
    crate::ocr::provider::apply_settings(&view, &state);
    assert_eq!(view.get_ocr_language_index(), 1);
    state.desired_settings.ocr_language = Some("auto".into());
    crate::ocr::provider::apply_settings(&view, &state);
    assert_eq!(view.get_provider_detail_id(), "paddleocr");
    assert!(!view.get_llm_connection_dirty());
    view.invoke_llm_enable_requested(false);
    let id = events
        .borrow()
        .iter()
        .find_map(|e| match e {
            AppEvent::SettingsChangeRequested {
                change:
                    SettingsChange::ProviderEnabled {
                        request_id,
                        provider: ServiceProviderId::PaddleOcr,
                        enabled: false,
                    },
            } => Some(*request_id),
            _ => None,
        })
        .unwrap();
    assert!(view.get_llm_enable_saving());
    assert!(
        view.get_ocr_provider_enabled(),
        "green dot follows persisted state until save succeeds"
    );
    assert!(!crate::llm_settings::enable_completed(
        &view,
        id + 1,
        ServiceProviderId::PaddleOcr,
        true
    ));
    assert!(crate::llm_settings::enable_completed(
        &view,
        id,
        ServiceProviderId::PaddleOcr,
        false
    ));
    assert!(view.get_llm_enabled());
    state.reduce(AppEvent::Ocr(
        lexift_core::domain::ocr::OcrEvent::ResourceCompleted {
            request_id: state.ocr.resources.request_id,
            result: Ok(lexift_core::ports::ocr::OcrResources {
                ready: true,
                status: lexift_core::ports::ocr::OcrResourceStatus::Ready,
                version: "v1".into(),
                download_bytes: 28579850,
                installed_bytes: 41213107,
                korean: Some(Box::new(lexift_core::ports::ocr::OcrResources {
                    ready: true,
                    status: lexift_core::ports::ocr::OcrResourceStatus::Ready,
                    version: "korean-v1".into(),
                    download_bytes: 8332170,
                    installed_bytes: 13467974,
                    entries: vec![],
                    korean: None,
                })),
                entries: [
                    ("det.onnx", 4748769),
                    ("universal.onnx", 21154056),
                    ("onnxruntime.dll", 14855240),
                ]
                .into_iter()
                .map(|(id, bytes)| lexift_core::ports::ocr::OcrResourceEntry {
                    id: id.into(),
                    bytes,
                })
                .collect(),
            }),
        },
    ));
    crate::ocr::provider::apply_settings(&view, &state);
    assert!(view.get_ocr_resource_ready());
    assert_eq!(view.get_ocr_download_size_compact(), "27.3 MiB");
    assert!(view.get_ocr_download_size().contains("28579850 bytes"));
    assert_eq!(view.get_ocr_model_rows().row_count(), 3);
    assert!(view.get_ocr_korean_ready());
    assert!(!view.get_ocr_korean_auto());
    assert!(view.get_ocr_korean_auto_enabled());
    view.invoke_ocr_korean_auto_toggled(true);
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        AppEvent::SettingsChangeRequested {
            change: SettingsChange::OcrKoreanAuto(true)
        }
    )));
    view.invoke_ocr_korean_download_requested();
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        AppEvent::Ocr(lexift_core::domain::ocr::OcrEvent::ManageResources(
            ResourceOperation::DownloadKorean
        ))
    )));
    view.show().unwrap();
    let surface = slot.borrow().as_ref().unwrap().clone();
    crate::i18n::select(UiLanguage::SimplifiedChinese);
    view.window().set_size(slint::LogicalSize::new(960., 1400.));
    // Exercise the relocated control through actual input, including its focus
    // and disabled behavior, rather than only invoking the bridge callback.
    for scale in [1., 1.25] {
        use slint::platform::{PointerEventButton, WindowEvent};
        view.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        WindowAdapter::set_size(
            surface.as_ref(),
            slint::LogicalSize::new(960., 1400.).into(),
        );
        crate::ocr::provider::apply_settings(&view, &state);
        surface.request_redraw();
        let size = surface.size();
        let mut pixels = vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
        surface.draw_if_needed(|r| {
            r.render(&mut pixels, size.width as usize);
        });
        events.borrow_mut().clear();
        let help_position = slint::LogicalPosition::new(518., 835.);
        view.window().dispatch_event(WindowEvent::PointerMoved {
            position: help_position,
        });
        view.window().dispatch_event(WindowEvent::PointerPressed {
            position: help_position,
            button: PointerEventButton::Left,
        });
        view.window().dispatch_event(WindowEvent::PointerReleased {
            position: help_position,
            button: PointerEventButton::Left,
        });
        assert!(events.borrow().is_empty(), "help must not save the setting");
        let click = || {
            let position = slint::LogicalPosition::new(890., 835.);
            view.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            view.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        };
        click();
        assert!(matches!(
            events.borrow().last(),
            Some(AppEvent::SettingsChangeRequested {
                change: SettingsChange::OcrKoreanAuto(true)
            })
        ));
        view.set_ocr_korean_auto(true);
        view.window()
            .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
        view.window()
            .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
        assert!(matches!(
            events.borrow().last(),
            Some(AppEvent::SettingsChangeRequested {
                change: SettingsChange::OcrKoreanAuto(false)
            })
        ));
        view.set_ocr_korean_auto_enabled(false);
        events.borrow_mut().clear();
        click();
        assert!(
            events.borrow().is_empty(),
            "disabled extension switch ignores clicks"
        );
        view.window().dispatch_event(WindowEvent::PointerMoved {
            position: help_position,
        });
        surface.request_redraw();
        surface.draw_if_needed(|r| {
            r.render(&mut pixels, size.width as usize);
        });
        let tooltip_background =
            pixels[(852. * scale) as usize * size.width as usize + (600. * scale) as usize];
        assert!(tooltip_background.r < 80, "disabled switch still has help");
    }
    crate::ocr::provider::apply_settings(&view, &state);
    view.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(200., 100.),
        });
    view.invoke_clear_settings_hint();
    let directory = std::env::var("LEXIFT_OCR_PROVIDER_SNAPSHOT_DIR").ok();
    for language in UiLanguage::ALL {
        crate::i18n::select(language);
        crate::ocr::provider::apply_settings(&view, &state);
        for (scale, width) in [(1., 960.), (1.25, 700.)] {
            view.window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            // Keep the extension and its automatic-detection control in the render.
            WindowAdapter::set_size(
                surface.as_ref(),
                slint::LogicalSize::new(width, 1400.).into(),
            );
            for dark in [false, true] {
                view.global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                surface.request_redraw();
                let size = surface.size();
                let mut pixels =
                    vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                surface.draw_if_needed(|r| {
                    r.render(&mut pixels, size.width as usize);
                });
                assert!(
                    pixels.iter().any(|p| p.b > p.r),
                    "provider has rendered content"
                );
                if let Some(directory) = &directory {
                    crate::i18n::write_test_bitmap(
                        directory,
                        &format!("provider-{}-{}-{}", language.code(), scale, dark),
                        size.width,
                        size.height,
                        pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                    );
                }
            }
        }
    }
    // Render every management state, including a long download failure, without network access.
    crate::i18n::select(UiLanguage::SimplifiedChinese);
    crate::ocr::provider::apply_settings(&view, &state);
    view.global::<crate::Colors>().set_preference(0);
    view.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1. });
    WindowAdapter::set_size(
        surface.as_ref(),
        slint::LogicalSize::new(960., 1400.).into(),
    );
    view.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(518., 835.),
        });
    surface.request_redraw();
    let mut hint_pixels = vec![slint::Rgb8Pixel::default(); 960 * 1400];
    surface.draw_if_needed(|r| {
        r.render(&mut hint_pixels, 960);
    });
    if let Some(directory) = &directory {
        crate::i18n::write_test_bitmap(
            directory,
            "korean-help",
            960,
            1400,
            hint_pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
        );
    }
    view.window().set_size(slint::LogicalSize::new(960., 960.));
    use lexift_core::ports::ocr::OcrResourceStatus;
    for (name, status, operation, error) in [
        ("absent", OcrResourceStatus::NotInstalled, None, ""),
        (
            "checking",
            OcrResourceStatus::Ready,
            Some(ResourceOperation::Check),
            "",
        ),
        (
            "downloading",
            OcrResourceStatus::NotInstalled,
            Some(ResourceOperation::Download),
            "",
        ),
        ("damaged", OcrResourceStatus::Damaged, None, ""),
        (
            "deleting",
            OcrResourceStatus::Ready,
            Some(ResourceOperation::Remove),
            "",
        ),
        ("pending", OcrResourceStatus::PendingRemoval, None, ""),
        (
            "failure",
            OcrResourceStatus::NotInstalled,
            None,
            "OCR resources: HTTP status client error (404 Not Found) for url (https://github.com/lizhenisu/lexift/releases/download/ocr-v2/lexift-ocr-v2-windows-x86_64.zip)",
        ),
    ] {
        let mut sample = state.clone();
        let info = sample.ocr.resources.info.as_mut().unwrap();
        info.status = status;
        info.ready = status == OcrResourceStatus::Ready;
        if status == OcrResourceStatus::NotInstalled {
            info.installed_bytes = 0;
        }
        sample.ocr.resources.busy = operation;
        sample.ocr.resources.received = 14289925;
        sample.ocr.resources.error = error.into();
        crate::ocr::provider::apply_settings(&view, &sample);
        if operation == Some(ResourceOperation::Download) {
            assert!((view.get_ocr_download_progress() - 0.5).abs() < 0.001);
        }
        assert_eq!(view.get_ocr_resource_note_error(), !error.is_empty());
        surface.request_redraw();
        let size = surface.size();
        let mut pixels = vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
        surface.draw_if_needed(|r| {
            r.render(&mut pixels, size.width as usize);
        });
        if let Some(directory) = &directory {
            crate::i18n::write_test_bitmap(
                directory,
                &format!("ocr-state-{name}"),
                size.width,
                size.height,
                pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
            );
        }
    }
    let mut completed = state.clone();
    completed.ocr.error.clear();
    crate::ocr::provider::apply_settings(&view, &completed);
    assert!(view.get_ocr_resource_note().is_empty());
    assert!(!view.get_ocr_resource_note_error());
    crate::ocr::provider::apply_settings(&view, &state);
    crate::i18n::select(UiLanguage::EnglishUs);
    view.global::<crate::Colors>().set_preference(0);
    view.set_provider_detail_index(1);
    view.set_llm_service_index(0);
    view.invoke_llm_service_selected(1);
    assert_eq!(view.get_llm_base_url(), "http://unsaved.example/v1");
    assert!(view.get_llm_connection_dirty());
    if let Some(directory) = directory {
        surface.request_redraw();
        let size = surface.size();
        let mut pixels = vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
        surface.draw_if_needed(|r| {
            r.render(&mut pixels, size.width as usize);
        });
        crate::i18n::write_test_bitmap(
            &directory,
            "llm-after-ocr",
            size.width,
            size.height,
            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
        );
    }
}
