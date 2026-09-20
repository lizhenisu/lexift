use std::sync::{Arc, OnceLock};

use lexift_core::ports::instance::InstanceStatus;
use lexift_core::ports::tray::{TrayHandler, TrayPort};

use crate::{StartupMode, controller::AppController, lifecycle, wiring::AppServices};

pub(crate) fn run(startup_mode: StartupMode) -> Result<(), Box<dyn std::error::Error>> {
    lexift_observability::init();
    lifecycle::on_start();

    #[cfg(not(feature = "m1-demo"))]
    let platform = lexift_platform::PlatformCapabilities::new();
    #[cfg(feature = "m1-demo")]
    let platform = lexift_platform::PlatformCapabilities::mock();
    let instance = platform.instance();
    if let Some(instance) = &instance
        && instance.acquire()? == InstanceStatus::AlreadyRunning
    {
        tracing::info!("another Lexift instance was activated");
        lifecycle::on_exit();
        return Ok(());
    }

    let services = AppServices::for_current_build(platform)?;
    let initial_state = services
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let ui_handle_slot = Arc::new(OnceLock::<lexift_ui::UiHandle>::new());
    let dismiss_handle_slot = Arc::clone(&ui_handle_slot);
    let window_context_monitor =
        match lexift_platform::WindowContextMonitor::new(Arc::new(move || {
            if let Some(handle) = dismiss_handle_slot.get() {
                handle.dismiss_language_menu();
            }
        })) {
            Ok(monitor) => Some(Arc::new(monitor)),
            Err(error) => {
                tracing::warn!(%error, "window context monitor is unavailable");
                None
            }
        };
    let monitor_for_arm = window_context_monitor.clone();
    let monitor_for_disarm = window_context_monitor.clone();
    let ui = lexift_ui::Ui::new(
        &initial_state,
        cfg!(feature = "m1-demo"),
        |window| match lexift_platform::configure_passive_tool_window(&window.window_handle()) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "passive window activation policy is unavailable");
                false
            }
        },
        |window| match lexift_platform::configure_interactive_tool_window(&window.window_handle()) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "interactive tool window policy is unavailable");
                false
            }
        },
        |child, owner| match lexift_platform::set_transient_window_owner(
            &child.window_handle(),
            &owner.window_handle(),
        ) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "transient window owner is unavailable");
                false
            }
        },
        Arc::new(move |owner, transient| {
            monitor_for_arm.as_ref().is_none_or(|monitor| {
                let owner_handle = owner.window_handle();
                let transient_handle = transient.window_handle();
                match monitor.arm_context(&[&owner_handle, &transient_handle]) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(%error, "window context monitor could not be armed");
                        false
                    }
                }
            })
        }),
        Arc::new(move || {
            if let Some(monitor) = &monitor_for_disarm {
                monitor.disarm();
            }
        }),
    )?;
    let _ = ui_handle_slot.set(ui.handle());
    let controller = Arc::new(
        AppController::new(
            services.runtime.handle().clone(),
            Arc::clone(&services.state),
            services.selection.clone(),
            services.screen.clone(),
            Arc::clone(&services.translator),
            Arc::clone(&services.settings_store),
            Arc::new(ui.handle()),
        )
        .with_runtime_manager(Arc::clone(&services.runtime_manager))
        .with_credential_management(
            Arc::clone(&services.credential_store),
            Arc::clone(&services.credential_reference),
            services.clipboard.clone(),
        ),
    );
    if let Some(instance) = &instance {
        let controller_for_activation = Arc::clone(&controller);
        if let Err(error) = instance.set_activation_handler(Arc::new(move || {
            controller_for_activation.dispatch(lexift_core::AppEvent::MainWindowRequested);
        })) {
            tracing::warn!(%error, "second-launch activation is unavailable");
        }
    }
    ui.on_event(
        {
            let controller = Arc::clone(&controller);
            move |event| controller.dispatch(event)
        },
        {
            let screen = services.screen.clone();
            move || {
                let screen = screen.as_ref()?;
                let cursor = screen.cursor_position().ok()?;
                let work_area = screen.work_area_for_point(cursor).ok()?;
                Some((cursor, work_area))
            }
        },
    );
    if let Err(error) = services
        .runtime_manager
        .start_hotkey(controller.translate_hotkey_handler())
    {
        tracing::warn!(%error, "global translate hotkey is unavailable");
    }
    if let Err(error) = services.runtime_manager.reconcile_autostart() {
        tracing::warn!(%error, "launch-at-login registration could not be reconciled");
    }
    let tray_registered = register_tray(services.tray.as_ref(), controller.tray_handler());
    ui.set_background_mode(tray_registered);
    controller.dispatch(lexift_core::AppEvent::Started);

    let result = ui
        .run(startup_mode == StartupMode::Interactive)
        .map_err(Into::into);

    drop(controller);
    drop(ui);
    drop(services);
    lifecycle::on_exit();
    result
}

fn register_tray(tray: Option<&Arc<dyn TrayPort>>, handler: TrayHandler) -> bool {
    let Some(tray) = tray else {
        return false;
    };
    match tray.register(handler) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "system tray is unavailable; closing the main window will exit");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use lexift_core::{Error, Result};

    use super::*;

    struct FailingTray;

    impl TrayPort for FailingTray {
        fn register(&self, _handler: TrayHandler) -> Result<()> {
            Err(Error::new("tray unavailable"))
        }
    }

    #[test]
    fn tray_registration_failure_keeps_background_mode_disabled() {
        let tray: Arc<dyn TrayPort> = Arc::new(FailingTray);
        assert!(!register_tray(Some(&tray), Arc::new(|_| {})));
        assert!(!register_tray(None, Arc::new(|_| {})));
    }
}
