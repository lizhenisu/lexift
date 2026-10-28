//! Main window lifecycle and view wiring.
use super::*;
pub(super) fn ensure_main_window(
    registry: &mut AppWindowRegistry,
) -> Result<AppWindow, slint::PlatformError> {
    if let Some(main) = &registry.main {
        return Ok(main.clone_strong());
    }
    let main = AppWindow::new()?;
    crate::theme::apply(&main);
    crate::usage_guide::apply(&main, &registry.latest_state);
    fit_main(&main, registry);
    if let Some(handler) = registry.handler.clone() {
        wire_main_callbacks(&main, handler, Rc::clone(&registry.background_mode));
    }
    registry.main = Some(main.clone_strong());
    Ok(main)
}

/// Release the component after the close callback returns; the generation protects a reopen.
pub(super) fn schedule_main_window_destruction() {
    let generation = APP_WINDOW_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        registry.main_show_timer.stop();
        registry.main_close_generation = registry.main_close_generation.wrapping_add(1);
        Some(registry.main_close_generation)
    });
    let Some(generation) = generation else {
        return;
    };
    slint::Timer::single_shot(Duration::ZERO, move || {
        APP_WINDOW_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut()
                && registry.main_close_generation == generation
                && let Some(main) = registry.main.take()
            {
                let _ = main.hide();
                drop(main);
            }
        });
        schedule_idle_memory_trim();
    });
}

pub(super) fn wire_main_callbacks(
    main: &AppWindow,
    handler: Rc<dyn Fn(AppEvent)>,
    background_mode: Rc<Cell<bool>>,
) {
    let main_close_handler = Rc::clone(&handler);
    main.window().on_close_requested(move || {
        match close_policy(background_mode.get()) {
            MainWindowClosePolicy::HideToTray => {
                main_close_handler(AppEvent::MainWindowClosed);
                schedule_main_window_destruction();
            }
            MainWindowClosePolicy::Exit => main_close_handler(AppEvent::ExitRequested),
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    main.on_settings_window_requested(move || handler(AppEvent::SettingsWindowRequested));
}

/// Closing the guide keeps the background runtime alive only when the tray is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MainWindowClosePolicy {
    HideToTray,
    Exit,
}

pub(super) fn close_policy(tray_registered: bool) -> MainWindowClosePolicy {
    if tray_registered {
        MainWindowClosePolicy::HideToTray
    } else {
        MainWindowClosePolicy::Exit
    }
}

impl UiHandle {
    pub fn show_main_window(&self) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let activate_user_requested_window = self.activate_user_requested_window;
        let _ = slint::invoke_from_event_loop(move || {
            cancel_idle_memory_trim();
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let Some(registry) = registry.as_mut() else {
                    return;
                };
                registry.main_close_generation = registry.main_close_generation.wrapping_add(1);
                if let Some(settings) = registry.settings.as_ref() {
                    settings.invoke_close_settings_menu();
                    clear_credential_transient(settings, &credential_generation);
                    clear_settings_toasts(settings, &toast_records);
                }
                match ensure_main_window(registry) {
                    Ok(_) => {
                        app_window::schedule(
                            registry,
                            WindowRole::Auxiliary,
                            true,
                            activate_user_requested_window,
                            0,
                        );
                    }
                    Err(error) => tracing::error!(%error, "main window could not be created"),
                }
            });
        });
    }
}

/// Size the guide while hidden; only explicit show requests invoke this path.
pub(super) fn fit_main(main: &AppWindow, registry: &AppWindowRegistry) {
    let point = (registry.main_cursor_position)().unwrap_or_else(|| {
        let p = main.window().position();
        Point { x: p.x, y: p.y }
    });
    let Some(area) = (registry.main_work_area)(point) else {
        return;
    };
    let scale = main.window().scale_factor().max(0.5);
    let width = 720f32.min((area.right - area.left).max(1) as f32 * 0.9 / scale);
    let height = 640f32.min((area.bottom - area.top).max(1) as f32 * 0.9 / scale);
    main.window()
        .set_size(slint::LogicalSize::new(width, height));
    main.window().set_position(slint::PhysicalPosition::new(
        area.left + ((area.right - area.left) as f32 - width * scale).max(0.) as i32 / 2,
        area.top + ((area.bottom - area.top) as f32 - height * scale).max(0.) as i32 / 2,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform {
        expected_main: Option<Rc<Cell<bool>>>,
    }
    impl Platform for TestPlatform {
        fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
            if let Some(expected) = &self.expected_main {
                APP_WINDOW_REGISTRY.with(|slot| {
                    assert_eq!(
                        slot.borrow().as_ref().unwrap().main.is_some(),
                        expected.get()
                    );
                });
            }
            Ok(())
        }
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    fn test_ui() -> Ui {
        let lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        Ui::new(
            &AppState::default(),
            |_| PassiveWindowPreparation::Ready,
            lifecycle,
        )
        .unwrap()
    }
    #[test]
    fn silent_start_does_not_create_a_guide_but_fallback_still_can() {
        let expected = Rc::new(Cell::new(false));
        slint::platform::set_platform(Box::new(TestPlatform {
            expected_main: Some(expected.clone()),
        }))
        .unwrap();
        let ui = test_ui();
        ui.set_background_mode(true);
        ui.run(false).unwrap();
        // Returning from the loop destroys the registry, including any created components.
        APP_WINDOW_REGISTRY.with(|slot| assert!(slot.borrow().is_none()));
        drop(ui);
        expected.set(true);
        let ui = test_ui();
        ui.set_background_mode(false);
        ui.run(true).unwrap();
        APP_WINDOW_REGISTRY.with(|slot| assert!(slot.borrow().is_none()));
        assert_eq!(close_policy(false), MainWindowClosePolicy::Exit);
    }
    #[test]
    fn guide_reopens_with_latest_keys_and_only_wires_settings() {
        slint::platform::set_platform(Box::new(TestPlatform {
            expected_main: None,
        }))
        .unwrap();
        let ui = test_ui();
        ui.set_background_mode(true);
        let events = Rc::new(RefCell::new(Vec::new()));
        let out = events.clone();
        ui.on_event(move |event| out.borrow_mut().push(event), || None);
        let main = APP_WINDOW_REGISTRY
            .with(|slot| ensure_main_window(slot.borrow_mut().as_mut().unwrap()).unwrap());
        let old = main.as_weak();
        main.invoke_settings_window_requested();
        assert!(matches!(
            events.borrow().as_slice(),
            [AppEvent::SettingsWindowRequested]
        ));
        schedule_main_window_destruction();
        drop(main);
        slint::platform::update_timers_and_animations();
        assert!(old.upgrade().is_none());
        let reopened = APP_WINDOW_REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let key = "Ctrl+Shift+F8".parse().unwrap();
            r.latest_state.runtime_config.hotkey = key;
            r.latest_state.registered_hotkeys.translation = Some(key);
            ensure_main_window(r).unwrap()
        });
        assert_eq!(reopened.get_translation_shortcut(), "Ctrl + Shift + F8");
        assert!(!reopened.get_translation_shortcut_unavailable());
        // An unrelated translation result cannot create a main window after it is closed.
        APP_WINDOW_REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.take();
            r.latest_state.translated_text = "not a home result".into();
            assert!(r.main.is_none());
        });
        drop(reopened);
        drop(ui);
    }
}
