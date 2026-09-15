use std::sync::Arc;

use lexift_core::ports::tray::{TrayHandler, TrayPort};

use crate::{controller::AppController, lifecycle, wiring::AppServices};

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    lexift_observability::init();
    lifecycle::on_start();

    let services = AppServices::for_current_build()?;
    let initial_state = services
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let ui = lexift_ui::Ui::new(&initial_state, cfg!(feature = "m1-demo"), |window| {
        if let Err(error) = lexift_platform::configure_translation_popup(&window.window_handle()) {
            tracing::warn!(%error, "popup activation policy is unavailable");
        }
    })?;
    let controller = Arc::new(AppController::new(
        services.runtime.handle().clone(),
        Arc::clone(&services.state),
        services.selection.clone(),
        services.screen.clone(),
        Arc::clone(&services.translator),
        Arc::new(ui.handle()),
    ));
    ui.on_event({
        let controller = Arc::clone(&controller);
        move |event| controller.dispatch(event)
    });
    if let Some(hotkey) = &services.hotkey
        && let Err(error) = hotkey.register_translate_hotkey(controller.translate_hotkey_handler())
    {
        tracing::warn!(%error, "global translate hotkey is unavailable");
    }
    let tray_registered = register_tray(services.tray.as_ref(), controller.tray_handler());
    ui.set_background_mode(tray_registered);
    controller.dispatch(lexift_core::AppEvent::Started);

    let result = ui.run().map_err(Into::into);

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
