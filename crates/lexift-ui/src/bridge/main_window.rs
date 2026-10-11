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
    main.set_show_selection_demo(registry.show_selection_demo);
    binding::apply_main(&main, &mapper::view_state(&registry.latest_state));
    if let Some(handler) = registry.handler.clone() {
        wire_main_callbacks(&main, handler, Rc::clone(&registry.background_mode));
    }
    registry.main = Some(main.clone_strong());
    Ok(main)
}

/// Defer full dirtying until winit has applied the restore/resize event. Coalescing
/// prevents redundant repairs; the weak handle cannot resurrect a closed window.
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
    let selection_handler = Rc::clone(&handler);
    main.on_selection_translation_requested(move || {
        selection_handler(AppEvent::SelectionTranslationRequested);
    });
    let input_handler = Rc::clone(&handler);
    main.on_input_translation_requested(move |text| {
        input_handler(AppEvent::InputTranslationRequested {
            text: text.to_string(),
        });
    });
    main.on_settings_window_requested(move || handler(AppEvent::SettingsWindowRequested));
}

/// History controls from a retired Settings instance cannot clear or re-enable data.
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
