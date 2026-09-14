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
    pub(crate) fn for_current_build() -> Result<Self, Box<dyn std::error::Error>> {
        let settings = lexift_config::load()?;

        #[cfg(not(feature = "m1-demo"))]
        let platform = lexift_platform::PlatformCapabilities::new();
        #[cfg(feature = "m1-demo")]
        let platform = lexift_platform::PlatformCapabilities::mock();

        #[cfg(not(feature = "m1-demo"))]
        let translators = lexift_translate::ProviderRegistry::new();
        #[cfg(feature = "m1-demo")]
        let translators = lexift_translate::ProviderRegistry::with_mock();

        let selection = platform.selection().ok_or_else(|| {
            lexift_core::Error::new(
                "no production selection adapter is configured; use --features m1-demo for the M1 demo",
            )
        })?;
        let translator = translators.default_translator().ok_or_else(|| {
            lexift_core::Error::new(
                "no production translator is configured; use --features m1-demo for the M1 demo",
            )
        })?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lexift-worker")
            .enable_all()
            .build()?;

        Ok(Self {
            runtime,
            state: Arc::new(Mutex::new(AppState::default())),
            selection,
            translator,
            _settings: settings,
        })
    }
}
