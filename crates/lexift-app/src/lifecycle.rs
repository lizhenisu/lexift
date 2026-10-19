pub(crate) fn on_start() {
    tracing::info!("Lexift starting");
}

pub(crate) fn on_exit() {
    lexift_platform::shutdown_screenshot_element_queries();
    tracing::info!("Lexift stopped");
}
