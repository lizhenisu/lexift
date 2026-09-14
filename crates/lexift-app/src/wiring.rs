use lexift_core::AppState;

/// Owns the concrete adapters assembled by the application composition root.
pub(crate) struct AppServices {
    pub(crate) state: AppState,
    _settings: lexift_config::AppConfig,
    _platform: lexift_platform::PlatformCapabilities,
    _translators: lexift_translate::ProviderRegistry,
}

impl AppServices {
    pub(crate) fn new() -> Result<Self, lexift_core::Error> {
        let settings = lexift_config::load()?;
        let platform = lexift_platform::PlatformCapabilities::new();
        let translators = lexift_translate::ProviderRegistry::new();

        Ok(Self {
            state: AppState::ready("Workspace initialized"),
            _settings: settings,
            _platform: platform,
            _translators: translators,
        })
    }
}
