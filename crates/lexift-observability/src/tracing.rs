use std::{fs, sync::OnceLock};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub(crate) fn init() {
    let Some(log_dir) =
        dirs::data_local_dir().map(|directory| directory.join("Lexift").join("logs"))
    else {
        init_console_only();
        return;
    };

    if fs::create_dir_all(&log_dir).is_err() {
        init_console_only();
        return;
    }

    let appender = match RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("lexift")
        .filename_suffix("log")
        .max_log_files(5)
        .build(log_dir)
    {
        Ok(appender) => appender,
        Err(_) => {
            init_console_only();
            return;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(guard);

    let filter = default_filter();
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(writer);

    #[cfg(debug_assertions)]
    let result = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .try_init();

    #[cfg(not(debug_assertions))]
    let result = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .try_init();

    let _ = result;
}

fn init_console_only() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(default_filter())
        .with_target(true)
        .try_init();
}

fn default_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}
