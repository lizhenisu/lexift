use lexift_core::domain::{
    language::Language,
    runtime_config::{HotkeyConfig, HotkeyKey, HotkeyModifiers, ProviderConfig},
    settings::Settings,
};
use serde::{Deserialize, Serialize};

use crate::migration::CURRENT_SCHEMA_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ConfigFile {
    pub(crate) schema_version: u32,
    pub(crate) settings: SettingsFile,
    #[serde(default)]
    pub(crate) credentials: CredentialReferencesFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SettingsFile {
    target_language: String,
    #[serde(default)]
    hotkey: HotkeyFile,
    #[serde(default = "default_provider")]
    provider: String,
    #[serde(default)]
    launch_at_login: bool,
    #[serde(default = "default_selection_toolbar")]
    selection_toolbar: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HotkeyFile {
    control: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    key: String,
}

impl Default for HotkeyFile {
    fn default() -> Self {
        Self::from(HotkeyConfig::default())
    }
}

impl From<HotkeyConfig> for HotkeyFile {
    fn from(value: HotkeyConfig) -> Self {
        Self {
            control: value.modifiers.control,
            alt: value.modifiers.alt,
            shift: value.modifiers.shift,
            meta: value.modifiers.meta,
            key: value.key.to_string(),
        }
    }
}

fn default_provider() -> String {
    ProviderConfig::default().id().into()
}

fn default_selection_toolbar() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CredentialReferencesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deepl: Option<String>,
}

impl ConfigFile {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            settings: SettingsFile {
                target_language: settings.target_language.0.clone(),
                hotkey: settings.hotkey.into(),
                provider: settings.provider.id().into(),
                launch_at_login: settings.launch_at_login,
                selection_toolbar: settings.selection_toolbar,
            },
            credentials: CredentialReferencesFile {
                deepl: settings.deepl_credential_id.clone(),
            },
        }
    }

    pub(crate) fn into_settings(self) -> lexift_core::Result<Settings> {
        let key: HotkeyKey = self.settings.hotkey.key.parse()?;
        let hotkey = HotkeyConfig::new(
            HotkeyModifiers {
                control: self.settings.hotkey.control,
                alt: self.settings.hotkey.alt,
                shift: self.settings.hotkey.shift,
                meta: self.settings.hotkey.meta,
            },
            key,
        )?;
        Ok(Settings {
            target_language: Language(self.settings.target_language),
            hotkey,
            provider: self.settings.provider.parse()?,
            launch_at_login: self.settings.launch_at_login,
            selection_toolbar: self.settings.selection_toolbar,
            deepl_credential_id: self.credentials.deepl,
        })
    }
}
