use std::sync::Arc;

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
    let ui = lexift_ui::Ui::new(&initial_state, cfg!(feature = "m1-demo"))?;
    let controller = Arc::new(AppController::new(
        services.runtime.handle().clone(),
        Arc::clone(&services.state),
        services.selection.clone(),
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
    controller.dispatch(lexift_core::AppEvent::Started);

    let result = ui.run().map_err(Into::into);

    lifecycle::on_exit();
    result
}
