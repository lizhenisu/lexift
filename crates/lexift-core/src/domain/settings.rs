use super::{
    language::Language,
    runtime_config::{HotkeyConfig, ProviderConfig, RuntimeConfig},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    TargetLanguage,
    Hotkey,
    Provider,
    LaunchAtLogin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFeedback {
    SettingsSaved(SettingsField),
    SettingsSaveFailed(SettingsField),
    CredentialSaved,
    CredentialRemoved,
    CredentialCopied,
    CredentialOperationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsChange {
    TargetLanguage(Language),
    Hotkey(HotkeyConfig),
    Provider(ProviderConfig),
    LaunchAtLogin(bool),
}

impl SettingsChange {
    pub const fn field(&self) -> SettingsField {
        match self {
            Self::TargetLanguage(_) => SettingsField::TargetLanguage,
            Self::Hotkey(_) => SettingsField::Hotkey,
            Self::Provider(_) => SettingsField::Provider,
            Self::LaunchAtLogin(_) => SettingsField::LaunchAtLogin,
        }
    }

    pub fn apply_to(&self, settings: &mut Settings) {
        match self {
            Self::TargetLanguage(value) => settings.target_language = value.clone(),
            Self::Hotkey(value) => settings.hotkey = *value,
            Self::Provider(value) => settings.provider = *value,
            Self::LaunchAtLogin(value) => settings.launch_at_login = *value,
        }
    }

    pub fn matches(&self, settings: &Settings) -> bool {
        match self {
            Self::TargetLanguage(value) => settings.target_language == *value,
            Self::Hotkey(value) => settings.hotkey == *value,
            Self::Provider(value) => settings.provider == *value,
            Self::LaunchAtLogin(value) => settings.launch_at_login == *value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub target_language: Language,
    pub hotkey: HotkeyConfig,
    pub provider: ProviderConfig,
    pub launch_at_login: bool,
    /// Reference to a system credential. This value is never the credential secret.
    pub deepl_credential_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_language: Language("zh-CN".into()),
            hotkey: HotkeyConfig::default(),
            provider: ProviderConfig::default(),
            launch_at_login: false,
            deepl_credential_id: None,
        }
    }
}

impl Settings {
    pub fn runtime_config(&self) -> RuntimeConfig {
        RuntimeConfig {
            hotkey: self.hotkey,
            provider: self.provider,
            launch_at_login: self.launch_at_login,
        }
    }
}
