use std::sync::{Arc, Mutex};

#[cfg(any(not(feature = "m1-demo"), test))]
use lexift_core::Error;
use lexift_core::{
    AppState,
    domain::settings::Settings,
    ports::{
        hotkey::HotkeyPort, screen::ScreenPort, selection::SelectionPort, settings::SettingsStore,
        translator::TranslatorPort, tray::TrayPort,
    },
};
use tokio::runtime::{Builder, Runtime};

/// Owns the concrete adapters assembled by the application composition root.
pub(crate) struct AppServices {
    // Rust drops fields in declaration order; stop callbacks before tearing down the runtime.
    pub(crate) tray: Option<Arc<dyn TrayPort>>,
    pub(crate) hotkey: Option<Arc<dyn HotkeyPort>>,
    pub(crate) runtime: Runtime,
    pub(crate) state: Arc<Mutex<AppState>>,
    pub(crate) settings_store: Arc<dyn SettingsStore>,
    pub(crate) selection: Option<Arc<dyn SelectionPort>>,
    pub(crate) screen: Option<Arc<dyn ScreenPort>>,
    pub(crate) translator: Arc<dyn TranslatorPort>,
}

impl AppServices {
    pub(crate) fn for_current_build() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(not(feature = "m1-demo"))]
        let settings_store: Arc<dyn SettingsStore> =
            match lexift_config::FileSettingsStore::for_current_user() {
                Ok(store) => Arc::new(store),
                Err(error) => {
                    tracing::warn!(%error, "settings storage is unavailable; using defaults");
                    Arc::new(UnavailableSettingsStore(error.to_string()))
                }
            };
        #[cfg(feature = "m1-demo")]
        let settings_store: Arc<dyn SettingsStore> = Arc::new(EphemeralSettingsStore::default());
        let settings = load_settings_or_default(settings_store.as_ref());

        #[cfg(not(feature = "m1-demo"))]
        let platform = lexift_platform::PlatformCapabilities::new();
        #[cfg(feature = "m1-demo")]
        let platform = lexift_platform::PlatformCapabilities::mock();

        #[cfg(not(feature = "m1-demo"))]
        let translators = production_translators(std::env::var("LEXIFT_DEEPL_AUTH_KEY").ok())?;
        #[cfg(feature = "m1-demo")]
        let translators = lexift_translate::ProviderRegistry::with_mock();

        Self::from_capabilities(settings, settings_store, platform, translators)
    }

    fn from_capabilities(
        settings: Settings,
        settings_store: Arc<dyn SettingsStore>,
        platform: lexift_platform::PlatformCapabilities,
        translators: lexift_translate::ProviderRegistry,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selection = platform.selection();
        let hotkey = platform.hotkey();
        let screen = platform.screen();
        let tray = platform.tray();
        let translator = translators.default_translator();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lexift-worker")
            .enable_all()
            .build()?;

        Ok(Self {
            tray,
            hotkey,
            runtime,
            state: Arc::new(Mutex::new(AppState::new(settings))),
            settings_store,
            selection,
            screen,
            translator,
        })
    }
}

#[cfg(any(feature = "m1-demo", test))]
#[derive(Default)]
struct EphemeralSettingsStore(Mutex<Settings>);

#[cfg(any(feature = "m1-demo", test))]
impl SettingsStore for EphemeralSettingsStore {
    fn load(&self) -> lexift_core::Result<Settings> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn save(&self, settings: &Settings) -> lexift_core::Result<()> {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings.clone();
        Ok(())
    }
}

fn load_settings_or_default(store: &dyn SettingsStore) -> Settings {
    match store.load() {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "settings could not be loaded; using defaults without overwriting the file");
            Settings::default()
        }
    }
}

#[cfg(not(feature = "m1-demo"))]
struct UnavailableSettingsStore(String);

#[cfg(not(feature = "m1-demo"))]
impl SettingsStore for UnavailableSettingsStore {
    fn load(&self) -> lexift_core::Result<Settings> {
        Ok(Settings::default())
    }

    fn save(&self, _settings: &Settings) -> lexift_core::Result<()> {
        Err(Error::new(self.0.clone()))
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

    struct FailingLoadStore;

    impl SettingsStore for FailingLoadStore {
        fn load(&self) -> lexift_core::Result<Settings> {
            Err(Error::new("malformed settings fixture"))
        }

        fn save(&self, _settings: &Settings) -> lexift_core::Result<()> {
            panic!("startup fallback must not overwrite an unreadable settings file")
        }
    }

    fn settings_store() -> Arc<dyn SettingsStore> {
        Arc::new(EphemeralSettingsStore::default())
    }

    #[test]
    fn services_accept_the_platform_selection_capability() {
        let services = AppServices::from_capabilities(
            Settings::default(),
            settings_store(),
            lexift_platform::PlatformCapabilities::new(),
            lexift_translate::ProviderRegistry::with_mock(),
        )
        .expect("platform capabilities should compose into application services");

        #[cfg(target_os = "windows")]
        assert!(services.selection.is_some());
        #[cfg(not(target_os = "windows"))]
        assert!(services.selection.is_none());
        #[cfg(target_os = "windows")]
        assert!(services.hotkey.is_some());
        #[cfg(target_os = "windows")]
        assert!(services.screen.is_some());
        #[cfg(target_os = "windows")]
        assert!(services.tray.is_some());
    }

    #[test]
    fn services_construct_without_tray_capability() {
        let services = AppServices::from_capabilities(
            Settings::default(),
            settings_store(),
            lexift_platform::PlatformCapabilities::mock(),
            lexift_translate::ProviderRegistry::with_mock(),
        )
        .expect("an unavailable tray must not prevent application construction");

        assert!(services.tray.is_none());
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
            Settings {
                target_language: target_language.clone(),
            },
            settings_store(),
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

    #[test]
    fn unreadable_settings_use_defaults_without_saving() {
        assert_eq!(
            load_settings_or_default(&FailingLoadStore),
            Settings::default()
        );
    }
}
