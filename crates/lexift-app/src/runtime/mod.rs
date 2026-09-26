mod hotkey_manager;
mod translator_runtime;

use std::sync::{Arc, RwLock};

use lexift_core::ports::autostart::AutostartPort;
#[cfg(not(feature = "m1-demo"))]
use lexift_core::ports::credential::CredentialStore;
#[cfg(any(not(feature = "m1-demo"), test))]
use lexift_core::ports::hotkey::HotkeyPort;
use lexift_core::ports::translator::TranslatorPort;
use lexift_core::{
    Result,
    domain::{runtime_config::RuntimeConfig, settings::SettingsField},
};

pub(crate) use translator_runtime::TranslatorRuntime;

use hotkey_manager::HotkeyRuntimeManager;

pub(crate) struct RuntimeManager {
    hotkey: HotkeyRuntimeManager,
    autostart: Option<Arc<dyn AutostartPort>>,
    translator: Arc<TranslatorRuntime>,
    current: RwLock<RuntimeConfig>,
}

impl RuntimeManager {
    pub(crate) fn start_annotation_hotkey(
        &self,
        handler: lexift_core::ports::hotkey::HotkeyHandler,
    ) -> lexift_core::Result<()> {
        self.hotkey.start_annotation(handler)
    }
    #[cfg(not(feature = "m1-demo"))]
    pub(crate) fn production(
        config: RuntimeConfig,
        hotkey: Option<Arc<dyn HotkeyPort>>,
        autostart: Option<Arc<dyn AutostartPort>>,
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
    ) -> Result<Self> {
        let translator = Arc::new(TranslatorRuntime::credential_backed(
            config.provider,
            store,
            credential_reference,
        )?);
        Ok(Self {
            hotkey: HotkeyRuntimeManager::new(hotkey),
            autostart,
            translator,
            current: RwLock::new(config),
        })
    }

    #[cfg(feature = "m1-demo")]
    pub(crate) fn demo(config: RuntimeConfig, translator: Arc<dyn TranslatorPort>) -> Self {
        Self {
            hotkey: HotkeyRuntimeManager::new(None),
            autostart: None,
            translator: Arc::new(TranslatorRuntime::fixed(config.provider, translator)),
            current: RwLock::new(config),
        }
    }

    #[cfg(test)]
    pub(crate) fn testing(
        config: RuntimeConfig,
        hotkey: Option<Arc<dyn HotkeyPort>>,
        translator: Arc<dyn TranslatorPort>,
    ) -> Self {
        Self {
            hotkey: HotkeyRuntimeManager::new(hotkey),
            autostart: None,
            translator: Arc::new(TranslatorRuntime::fixed(config.provider, translator)),
            current: RwLock::new(config),
        }
    }

    pub(crate) fn translator(&self) -> Arc<dyn TranslatorPort> {
        self.translator.clone()
    }

    pub(crate) fn start_hotkey(
        &self,
        handler: lexift_core::ports::hotkey::HotkeyHandler,
    ) -> Result<()> {
        self.hotkey.start(
            self.current
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hotkey,
            handler,
        )
    }

    pub(crate) fn apply(&self, field: SettingsField, config: RuntimeConfig) -> Result<()> {
        match field {
            SettingsField::TargetLanguage | SettingsField::SelectionToolbar => {}
            SettingsField::Hotkey => self.hotkey.apply(config.hotkey)?,
            SettingsField::Provider => {
                self.translator.validate(config.provider)?;
                self.translator.commit(config.provider);
            }
            SettingsField::LaunchAtLogin => self
                .autostart
                .as_ref()
                .ok_or_else(|| lexift_core::Error::new("Launch at login is unavailable"))?
                .set_enabled(config.launch_at_login)?,
        }
        let mut current = self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match field {
            SettingsField::TargetLanguage | SettingsField::SelectionToolbar => {}
            SettingsField::Hotkey => current.hotkey = config.hotkey,
            SettingsField::Provider => current.provider = config.provider,
            SettingsField::LaunchAtLogin => {
                current.launch_at_login = config.launch_at_login;
            }
        }
        Ok(())
    }

    /// Repairs a stale startup registration after the executable path changes.
    pub(crate) fn reconcile_autostart(&self) -> Result<()> {
        let enabled = self
            .current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .launch_at_login;
        let Some(autostart) = &self.autostart else {
            return if enabled {
                Err(lexift_core::Error::new("Launch at login is unavailable"))
            } else {
                Ok(())
            };
        };
        if autostart.is_enabled()? != enabled {
            autostart.set_enabled(enabled)?;
        }
        Ok(())
    }
}

#[cfg(all(test, not(feature = "m1-demo")))]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use lexift_core::{
        Result,
        domain::{
            runtime_config::{HotkeyConfig, RuntimeConfig},
            settings::SettingsField,
        },
        ports::{
            autostart::AutostartPort,
            credential::{CredentialResult, CredentialStore},
            hotkey::{HotkeyHandler, HotkeyPort},
        },
    };

    use super::RuntimeManager;

    struct EmptyCredentialStore;

    impl CredentialStore for EmptyCredentialStore {
        fn get(&self, _id: &str) -> CredentialResult<Option<String>> {
            Ok(None)
        }

        fn set(&self, _id: &str, _secret: &str) -> CredentialResult<()> {
            Ok(())
        }

        fn delete(&self, _id: &str) -> CredentialResult<()> {
            Ok(())
        }
    }

    struct AcceptingHotkey;

    #[derive(Default)]
    struct RecordingAutostart {
        enabled: Mutex<bool>,
        writes: Mutex<Vec<bool>>,
    }

    impl AutostartPort for RecordingAutostart {
        fn set_enabled(&self, enabled: bool) -> Result<()> {
            *self.enabled.lock().unwrap() = enabled;
            self.writes.lock().unwrap().push(enabled);
            Ok(())
        }

        fn is_enabled(&self) -> Result<bool> {
            Ok(*self.enabled.lock().unwrap())
        }
    }

    impl HotkeyPort for AcceptingHotkey {
        fn register_translate_hotkey(
            &self,
            _config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            Ok(())
        }

        fn replace_translate_hotkey(
            &self,
            _config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            Ok(())
        }

        fn unregister_translate_hotkey(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unrelated_runtime_changes_do_not_require_a_provider_credential() {
        let manager = RuntimeManager::production(
            RuntimeConfig::default(),
            Some(Arc::new(AcceptingHotkey)),
            None,
            Arc::new(EmptyCredentialStore),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();
        manager.start_hotkey(Arc::new(|| {})).unwrap();

        manager
            .apply(SettingsField::TargetLanguage, RuntimeConfig::default())
            .unwrap();
        let config = RuntimeConfig {
            hotkey: "Ctrl + Shift + 7".parse().unwrap(),
            ..RuntimeConfig::default()
        };
        manager.apply(SettingsField::Hotkey, config).unwrap();
        assert_eq!(
            manager
                .apply(SettingsField::Provider, RuntimeConfig::default())
                .unwrap_err()
                .to_string(),
            "DeepL API key is not configured"
        );
    }

    #[test]
    fn launch_at_login_changes_only_the_autostart_capability() {
        let autostart = Arc::new(RecordingAutostart::default());
        let manager = RuntimeManager::production(
            RuntimeConfig::default(),
            None,
            Some(autostart.clone()),
            Arc::new(EmptyCredentialStore),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();
        let config = RuntimeConfig {
            launch_at_login: true,
            ..RuntimeConfig::default()
        };

        manager.apply(SettingsField::LaunchAtLogin, config).unwrap();

        assert_eq!(*autostart.writes.lock().unwrap(), vec![true]);
    }

    #[test]
    fn startup_reconciliation_repairs_a_missing_registration() {
        let autostart = Arc::new(RecordingAutostart::default());
        let manager = RuntimeManager::production(
            RuntimeConfig {
                launch_at_login: true,
                ..RuntimeConfig::default()
            },
            None,
            Some(autostart.clone()),
            Arc::new(EmptyCredentialStore),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();

        manager.reconcile_autostart().unwrap();

        assert_eq!(*autostart.writes.lock().unwrap(), vec![true]);
    }
}
