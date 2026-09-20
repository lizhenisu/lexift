use std::sync::{Arc, Mutex, RwLock};

#[cfg(any(feature = "m1-demo", test))]
use std::collections::HashMap;

#[cfg(any(not(feature = "m1-demo"), test))]
use lexift_core::Error;
use lexift_core::{
    AppState,
    domain::settings::Settings,
    ports::{
        clipboard::ClipboardPort,
        credential::{CredentialError, CredentialErrorKind, CredentialResult, CredentialStore},
        screen::ScreenPort,
        selection::SelectionPort,
        settings::SettingsStore,
        translator::TranslatorPort,
        tray::TrayPort,
    },
};
use tokio::runtime::{Builder, Runtime};

use crate::runtime::RuntimeManager;

/// Owns the concrete adapters assembled by the application composition root.
pub(crate) struct AppServices {
    // Rust drops fields in declaration order; stop callbacks before tearing down the runtime.
    pub(crate) tray: Option<Arc<dyn TrayPort>>,
    pub(crate) runtime_manager: Arc<RuntimeManager>,
    pub(crate) runtime: Runtime,
    pub(crate) state: Arc<Mutex<AppState>>,
    pub(crate) settings_store: Arc<dyn SettingsStore>,
    pub(crate) credential_store: Arc<dyn CredentialStore>,
    pub(crate) credential_reference: Arc<RwLock<Option<String>>>,
    pub(crate) clipboard: Option<Arc<dyn ClipboardPort>>,
    pub(crate) selection: Option<Arc<dyn SelectionPort>>,
    pub(crate) screen: Option<Arc<dyn ScreenPort>>,
    pub(crate) translator: Arc<dyn TranslatorPort>,
}

impl AppServices {
    pub(crate) fn for_current_build(
        platform: lexift_platform::PlatformCapabilities,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
        let credential_store = platform
            .credential_store()
            .unwrap_or_else(|| Arc::new(UnavailableCredentialStore) as Arc<dyn CredentialStore>);
        #[cfg(feature = "m1-demo")]
        let credential_store: Arc<dyn CredentialStore> =
            Arc::new(EphemeralCredentialStore::default());
        let credential_reference = Arc::new(RwLock::new(settings.deepl_credential_id.clone()));
        let credential_configured = credential_is_configured(
            credential_store.as_ref(),
            settings.deepl_credential_id.as_deref(),
        );

        Self::from_capabilities(
            settings,
            settings_store,
            credential_store,
            credential_reference,
            credential_configured,
            platform,
        )
    }

    fn from_capabilities(
        settings: Settings,
        settings_store: Arc<dyn SettingsStore>,
        credential_store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
        credential_configured: bool,
        platform: lexift_platform::PlatformCapabilities,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selection = platform.selection();
        #[cfg(not(feature = "m1-demo"))]
        let hotkey = platform.hotkey();
        let screen = platform.screen();
        let tray = platform.tray();
        let clipboard = platform.clipboard();
        #[cfg(not(feature = "m1-demo"))]
        let autostart = platform.autostart();
        #[cfg(not(feature = "m1-demo"))]
        let runtime_manager = Arc::new(RuntimeManager::production(
            settings.runtime_config(),
            hotkey,
            autostart,
            Arc::clone(&credential_store),
            Arc::clone(&credential_reference),
        )?);
        #[cfg(feature = "m1-demo")]
        let runtime_manager = Arc::new(RuntimeManager::demo(
            settings.runtime_config(),
            lexift_translate::ProviderRegistry::with_mock().default_translator(),
        ));
        let translator = runtime_manager.translator();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lexift-worker")
            .enable_all()
            .build()?;

        Ok(Self {
            tray,
            runtime_manager,
            runtime,
            state: Arc::new(Mutex::new(AppState::with_credential_status(
                settings,
                credential_configured,
            ))),
            settings_store,
            credential_store,
            credential_reference,
            clipboard,
            selection,
            screen,
            translator,
        })
    }
}

fn credential_is_configured(store: &dyn CredentialStore, id: Option<&str>) -> bool {
    let Some(id) = id else {
        return false;
    };
    match store.get(id) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), "credential lookup failed during startup");
            false
        }
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

#[cfg(any(feature = "m1-demo", test))]
#[derive(Default)]
struct EphemeralCredentialStore(Mutex<HashMap<String, String>>);

#[cfg(any(feature = "m1-demo", test))]
impl CredentialStore for EphemeralCredentialStore {
    fn get(&self, id: &str) -> CredentialResult<Option<String>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned())
    }

    fn set(&self, id: &str, secret: &str) -> CredentialResult<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.into(), secret.into());
        Ok(())
    }

    fn delete(&self, id: &str) -> CredentialResult<()> {
        let removed = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
        if removed.is_some() {
            Ok(())
        } else {
            Err(CredentialError::new(
                CredentialErrorKind::Missing,
                "Credential does not exist",
            ))
        }
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
struct UnavailableCredentialStore;

#[cfg(not(feature = "m1-demo"))]
impl CredentialStore for UnavailableCredentialStore {
    fn get(&self, _id: &str) -> CredentialResult<Option<String>> {
        Err(CredentialError::new(
            CredentialErrorKind::PlatformFailure,
            "Secure credential storage is unavailable",
        ))
    }

    fn set(&self, _id: &str, _secret: &str) -> CredentialResult<()> {
        Err(CredentialError::new(
            CredentialErrorKind::PlatformFailure,
            "Secure credential storage is unavailable",
        ))
    }

    fn delete(&self, _id: &str) -> CredentialResult<()> {
        Err(CredentialError::new(
            CredentialErrorKind::PlatformFailure,
            "Secure credential storage is unavailable",
        ))
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

    fn test_services(
        settings: Settings,
        platform: lexift_platform::PlatformCapabilities,
    ) -> Result<AppServices, Box<dyn std::error::Error>> {
        AppServices::from_capabilities(
            settings,
            settings_store(),
            Arc::new(EphemeralCredentialStore::default()),
            Arc::new(RwLock::new(None)),
            false,
            platform,
        )
    }

    #[test]
    fn services_accept_the_platform_selection_capability() {
        let services = test_services(
            Settings::default(),
            lexift_platform::PlatformCapabilities::new(),
        )
        .expect("platform capabilities should compose into application services");

        #[cfg(target_os = "windows")]
        assert!(services.selection.is_some());
        #[cfg(not(target_os = "windows"))]
        assert!(services.selection.is_none());
        #[cfg(target_os = "windows")]
        assert!(services.screen.is_some());
        #[cfg(target_os = "windows")]
        assert!(services.tray.is_some());
        #[cfg(target_os = "windows")]
        assert!(services.clipboard.is_some());
    }

    #[test]
    fn services_construct_without_tray_capability() {
        let services = test_services(
            Settings::default(),
            lexift_platform::PlatformCapabilities::mock(),
        )
        .expect("an unavailable tray must not prevent application construction");

        assert!(services.tray.is_none());
        assert!(services.clipboard.is_none());
    }

    #[test]
    fn missing_deepl_credential_is_not_configured() {
        let store = EphemeralCredentialStore::default();
        assert!(!credential_is_configured(&store, None));
        assert!(!credential_is_configured(&store, Some("deepl-primary")));
    }

    #[cfg(not(feature = "m1-demo"))]
    #[test]
    fn missing_runtime_credential_fails_translation_without_blocking_startup() {
        use lexift_core::domain::{language::Language, translation::TranslateRequest};

        let store: Arc<dyn CredentialStore> = Arc::new(EphemeralCredentialStore::default());
        let reference = Arc::new(RwLock::new(Some("deepl-primary".into())));
        let translator = crate::runtime::TranslatorRuntime::credential_backed(
            lexift_core::domain::runtime_config::ProviderConfig::DeepL,
            store,
            reference,
        )
        .expect("HTTP infrastructure should initialize");
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let error = runtime
            .block_on(translator.translate(TranslateRequest {
                text: "Hello".into(),
                target_language: Language("zh-CN".into()),
            }))
            .expect_err("missing credential must fail only the requested translation");

        assert_eq!(error.to_string(), "Translation credential is missing");
    }

    #[test]
    fn injects_configured_settings_into_core_state() {
        let target_language = lexift_core::domain::language::Language("ja".into());
        let services = test_services(
            Settings {
                target_language: target_language.clone(),
                ..Settings::default()
            },
            lexift_platform::PlatformCapabilities::new(),
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
