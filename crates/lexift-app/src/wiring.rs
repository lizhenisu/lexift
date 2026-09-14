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
        let translators = production_translators(std::env::var("LEXIFT_DEEPL_AUTH_KEY").ok())?;
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
        let translator = translators.default_translator();
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

#[cfg(not(feature = "m1-demo"))]
fn production_translators(
    deepl_auth_key: Option<String>,
) -> Result<lexift_translate::ProviderRegistry, lexift_core::Error> {
    match deepl_auth_key.filter(|key| !key.trim().is_empty()) {
        Some(auth_key) => lexift_translate::ProviderRegistry::with_deepl_api(auth_key),
        None => Ok(lexift_translate::ProviderRegistry::new()),
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

    #[cfg(not(feature = "m1-demo"))]
    #[test]
    fn missing_deepl_credential_uses_the_unconfigured_translator() {
        let providers = production_translators(None)
            .expect("a missing credential must not prevent application startup");
        assert!(!providers.has_configured_provider());

        let providers = production_translators(Some("   ".into()))
            .expect("a blank credential must not prevent application startup");
        assert!(!providers.has_configured_provider());
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
