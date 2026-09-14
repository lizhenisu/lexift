pub(crate) fn init() {
    let _ = tracing_subscriber::fmt().with_target(false).try_init();
}
