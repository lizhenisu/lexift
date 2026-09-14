use std::sync::{Arc, Mutex};

use lexift_core::{
    AppState,
    ports::{selection::SelectionPort, translator::TranslatorPort},
};
use tokio::runtime::{Builder, Runtime};

/// Owns the concrete adapters assembled by the application composition root.
pub(crate) struct AppServices {
    pub(crate) runtime: Runtime,
    pub(crate) state: Arc<Mutex<AppState>>,
    pub(crate) selection: Arc<dyn SelectionPort>,
    pub(crate) translator: Arc<dyn TranslatorPort>,
    _settings: lexift_config::AppConfig,
}

impl AppServices {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let settings = lexift_config::load()?;
        let platform = lexift_platform::PlatformCapabilities::new();
        let translators = lexift_translate::ProviderRegistry::new();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lexift-worker")
            .enable_all()
            .build()?;

        Ok(Self {
            runtime,
            state: Arc::new(Mutex::new(AppState::default())),
            selection: platform.selection(),
            translator: translators.default_translator(),
            _settings: settings,
        })
    }
}
