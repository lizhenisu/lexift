use std::sync::{Arc, Mutex, RwLock};

#[cfg(any(feature = "m1-demo", test))]
use std::collections::HashMap;

#[cfg(any(not(feature = "m1-demo"), test))]
use lexift_core::Error;
#[cfg(not(feature = "m1-demo"))]
use lexift_core::ports::translator::TranslationFuture;
use lexift_core::{
    AppState,
    domain::settings::Settings,
    ports::{
        clipboard::ClipboardPort,
        credential::{CredentialError, CredentialErrorKind, CredentialResult, CredentialStore},
        hotkey::HotkeyPort,
        screen::ScreenPort,
        selection::SelectionPort,
        settings::SettingsStore,
        translator::TranslatorPort,
        tray::TrayPort,
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
    pub(crate) credential_store: Arc<dyn CredentialStore>,
    pub(crate) credential_reference: Arc<RwLock<Option<String>>>,
    pub(crate) clipboard: Option<Arc<dyn ClipboardPort>>,
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

        #[cfg(not(feature = "m1-demo"))]
        let translator: Arc<dyn TranslatorPort> = Arc::new(CredentialBackedTranslator::new(
            Arc::clone(&credential_store),
            Arc::clone(&credential_reference),
        )?);
        #[cfg(feature = "m1-demo")]
        let translator = lexift_translate::ProviderRegistry::with_mock().default_translator();

        Self::from_capabilities(
            settings,
            settings_store,
            credential_store,
            credential_reference,
            credential_configured,
            platform,
            translator,
        )
    }

    fn from_capabilities(
        settings: Settings,
        settings_store: Arc<dyn SettingsStore>,
        credential_store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
        credential_configured: bool,
        platform: lexift_platform::PlatformCapabilities,
        translator: Arc<dyn TranslatorPort>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selection = platform.selection();
        let hotkey = platform.hotkey();
        let screen = platform.screen();
        let tray = platform.tray();
        let clipboard = platform.clipboard();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lexift-worker")
            .enable_all()
            .build()?;

        Ok(Self {
            tray,
            hotkey,
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

#[cfg(not(feature = "m1-demo"))]
struct CredentialBackedTranslator {
    store: Arc<dyn CredentialStore>,
    credential_reference: Arc<RwLock<Option<String>>>,
    factory: lexift_translate::DeepLTranslatorFactory,
}

#[cfg(not(feature = "m1-demo"))]
impl CredentialBackedTranslator {
    fn new(
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
    ) -> lexift_core::Result<Self> {
        Ok(Self {
            store,
            credential_reference,
            factory: lexift_translate::DeepLTranslatorFactory::new()?,
        })
    }
}

#[cfg(not(feature = "m1-demo"))]
impl TranslatorPort for CredentialBackedTranslator {
    fn translate(
        &self,
        request: lexift_core::domain::translation::TranslateRequest,
    ) -> TranslationFuture<'_> {
        let credential_id = self
            .credential_reference
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let translator = credential_id
            .ok_or_else(|| Error::new("Translation provider is not configured"))
            .and_then(|credential_id| {
                self.store
                    .get(&credential_id)
                    .map_err(credential_lookup_error)
                    .and_then(|secret| {
                        secret.ok_or_else(|| Error::new("Translation credential is missing"))
                    })
            })
            .map(|secret| self.factory.translator(secret));
        Box::pin(async move {
            match translator {
                Ok(translator) => translator.translate(request).await,
                Err(error) => Err(error),
            }
        })
    }
}

#[cfg(not(feature = "m1-demo"))]
fn credential_lookup_error(error: CredentialError) -> Error {
    let message = match error.kind() {
        CredentialErrorKind::Missing => "Translation credential is missing",
        CredentialErrorKind::PermissionDenied => "Credential access was denied",
        CredentialErrorKind::PlatformFailure => "Credential storage is unavailable",
        CredentialErrorKind::InvalidFormat => "Stored translation credential is invalid",
    };
    Error::new(message)
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
            lexift_translate::ProviderRegistry::with_mock().default_translator(),
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
        assert!(services.hotkey.is_some());
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
        let translator = CredentialBackedTranslator::new(store, reference)
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
