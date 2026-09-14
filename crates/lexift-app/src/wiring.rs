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
    pub(crate) selection: Option<Arc<dyn SelectionPort>>,
    pub(crate) translator: Arc<dyn TranslatorPort>,
}

impl AppServices {
    pub(crate) fn for_current_build() -> Result<Self, Box<dyn std::error::Error>> {
        let settings = lexift_config::load()?;

        #[cfg(not(feature = "m1-demo"))]
        let platform = lexift_platform::PlatformCapabilities::new();
        #[cfg(feature = "m1-demo")]
        let platform = lexift_platform::PlatformCapabilities::mock();

        #[cfg(not(feature = "m1-demo"))]
        let translators = lexift_translate::ProviderRegistry::new()?;
        #[cfg(feature = "m1-demo")]
        let translators = lexift_translate::ProviderRegistry::with_mock();

        Self::from_capabilities(settings, platform, translators)
    }

    fn from_capabilities(
        settings: lexift_config::AppConfig,
        platform: lexift_platform::PlatformCapabilities,
        translators: lexift_translate::ProviderRegistry,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selection = platform.selection();
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
            state: Arc::new(Mutex::new(AppState::new(settings.settings))),
            selection,
            translator,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_allow_a_translator_without_selection() {
        let services = AppServices::from_capabilities(
            lexift_config::AppConfig::default(),
            lexift_platform::PlatformCapabilities::new(),
            lexift_translate::ProviderRegistry::with_mock(),
        )
        .expect("selection must be optional during M2");

        assert!(services.selection.is_none());
    }

    #[test]
    fn injects_configured_settings_into_core_state() {
        let target_language = lexift_core::domain::language::Language("ja".into());
        let services = AppServices::from_capabilities(
            lexift_config::AppConfig {
                settings: lexift_core::domain::settings::Settings {
                    target_language: target_language.clone(),
                },
                ..Default::default()
            },
            lexift_platform::PlatformCapabilities::new(),
            lexift_translate::ProviderRegistry::with_mock(),
        )
        .expect("configured settings should initialize app state");

        let state = services
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.settings.target_language, target_language);
    }
}
