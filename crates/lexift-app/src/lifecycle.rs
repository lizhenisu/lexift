pub(crate) fn on_start() {
    tracing::info!("Lexift starting");
}

pub(crate) fn on_exit() {
    tracing::info!("Lexift stopped");
}
