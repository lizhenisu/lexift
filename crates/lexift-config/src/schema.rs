use lexift_core::domain::{
    language::Language,
    runtime_config::{HotkeyConfig, HotkeyKey, HotkeyModifiers, ProviderConfig},
    settings::{Settings, ThemePreference},
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
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default = "default_ui_language")]
    ui_language: String,
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

fn default_ui_language() -> String {
    "en-US".into()
}

fn default_theme() -> String {
    "light".into()
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
                ui_language: settings.ui_language.code().into(),
                hotkey: settings.hotkey.into(),
                provider: settings.provider.id().into(),
                launch_at_login: settings.launch_at_login,
                selection_toolbar: settings.selection_toolbar,
                theme: match settings.theme {
                    ThemePreference::Light => "light",
                    ThemePreference::Dark => "dark",
                    ThemePreference::System => "system",
                }
                .into(),
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
            ui_language: self.settings.ui_language.parse()?,
            hotkey,
            provider: self.settings.provider.parse()?,
            launch_at_login: self.settings.launch_at_login,
            selection_toolbar: self.settings.selection_toolbar,
            theme: match self.settings.theme.as_str() {
                "light" => ThemePreference::Light,
                "dark" => ThemePreference::Dark,
                "system" => ThemePreference::System,
                value => {
                    return Err(lexift_core::Error::new(format!(
                        "Unsupported theme {value}"
                    )));
                }
            },
            deepl_credential_id: self.credentials.deepl,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::ui_language::UiLanguage;
    #[test]
    fn all_ui_languages_round_trip_independently_of_target() {
        for ui_language in UiLanguage::ALL {
            let settings = Settings {
                ui_language,
                target_language: Language("fr".into()),
                ..Default::default()
            };
            let text = toml::to_string(&ConfigFile::from_settings(&settings)).unwrap();
            let loaded: ConfigFile = toml::from_str(&text).unwrap();
            assert_eq!(loaded.into_settings().unwrap(), settings);
        }
    }
    #[test]
    fn old_config_defaults_to_english_and_unknown_language_is_rejected() {
        let text = toml::to_string(&ConfigFile::from_settings(&Settings::default())).unwrap();
        let legacy = text
            .lines()
            .filter(|s| !s.starts_with("ui_language"))
            .collect::<Vec<_>>()
            .join("\n");
        let file: ConfigFile = toml::from_str(&legacy).unwrap();
        assert_eq!(
            file.into_settings().unwrap().ui_language,
            UiLanguage::EnglishUs
        );
        let invalid: ConfigFile = toml::from_str(&text.replace("en-US", "unknown")).unwrap();
        assert!(invalid.into_settings().is_err());
    }
}
