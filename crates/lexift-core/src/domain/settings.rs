use super::{
    language::Language,
    runtime_config::{HotkeyConfig, ProviderConfig, RuntimeConfig},
    ui_language::UiLanguage,
};

/// Persisted appearance preference; System resolves in the UI backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    TargetLanguage,
    Hotkey,
    AnnotationHotkey,
    Provider,
    Ollama,
    Llm,
    ProviderEnabled(ProviderConfig),
    TranslationSelection,
    TranslationOptions,
    LaunchAtLogin,
    SelectionToolbar,
    ClipboardHistoryEnabled,
    ClipboardHistoryLimit,
    Theme,
    UiLanguage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFeedback {
    SettingsSaved(SettingsField),
    SettingsSaveFailed(SettingsField),
    TranslationSettingsCompleted {
        request_id: u64,
        field: SettingsField,
        saved: bool,
    },
    ProviderEnabledCompleted {
        request_id: u64,
        provider: ProviderConfig,
        saved: bool,
    },
    CredentialSaved,
    CredentialRemoved,
    CredentialCopied {
        generation: u64,
    },
    CredentialOperationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsChange {
    TargetLanguage(Language),
    Hotkey(HotkeyConfig),
    AnnotationHotkey(HotkeyConfig),
    Provider(ProviderConfig),
    Ollama(super::ollama::OllamaConfig),
    Llm(super::llm::LlmSettings),
    /// Toggle one provider without applying connection drafts.
    ProviderEnabled {
        request_id: u64,
        provider: ProviderConfig,
        enabled: bool,
    },
    /// Select a service and model together without replacing connection/catalog state.
    TranslationSelection {
        request_id: u64,
        provider: ProviderConfig,
        model: Option<super::llm::TranslationModel>,
    },
    /// Edit one model and the shared prompt without changing the selected service.
    TranslationOptions {
        request_id: u64,
        model: super::llm::TranslationModel,
        prompt: String,
    },
    LaunchAtLogin(bool),
    SelectionToolbar(bool),
    ClipboardHistoryEnabled(bool),
    ClipboardHistoryLimit(u16),
    Theme(ThemePreference),
    UiLanguage(UiLanguage),
}

impl SettingsChange {
    pub const fn field(&self) -> SettingsField {
        match self {
            Self::TargetLanguage(_) => SettingsField::TargetLanguage,
            Self::Hotkey(_) => SettingsField::Hotkey,
            Self::AnnotationHotkey(_) => SettingsField::AnnotationHotkey,
            Self::Provider(_) => SettingsField::Provider,
            Self::Ollama(_) => SettingsField::Ollama,
            Self::Llm(_) => SettingsField::Llm,
            Self::ProviderEnabled { provider, .. } => SettingsField::ProviderEnabled(*provider),
            Self::TranslationSelection { .. } => SettingsField::TranslationSelection,
            Self::TranslationOptions { .. } => SettingsField::TranslationOptions,
            Self::LaunchAtLogin(_) => SettingsField::LaunchAtLogin,
            Self::SelectionToolbar(_) => SettingsField::SelectionToolbar,
            Self::ClipboardHistoryEnabled(_) => SettingsField::ClipboardHistoryEnabled,
            Self::ClipboardHistoryLimit(_) => SettingsField::ClipboardHistoryLimit,
            Self::Theme(_) => SettingsField::Theme,
            Self::UiLanguage(_) => SettingsField::UiLanguage,
        }
    }

    /// Validate targeted translation edits against the current configuration at dispatch time.
    pub fn validate_translation_change(&self, settings: &Settings) -> crate::Result<()> {
        if let Self::ClipboardHistoryLimit(value) = self
            && !(10..=200).contains(value)
        {
            return Err(crate::Error::new(
                "Clipboard history limit must be between 10 and 200",
            ));
        }
        if !matches!(
            self,
            Self::TranslationSelection { .. } | Self::TranslationOptions { .. }
        ) {
            return Ok(());
        }
        if let Self::TranslationSelection {
            provider, model, ..
        } = self
        {
            let valid = match (provider, model) {
                (ProviderConfig::DeepL, None) => settings.llm.deepl_enabled,
                (provider, Some(model)) => {
                    provider.id() == model.provider.id()
                        && settings.llm.connection(model.provider).enabled
                        && settings.llm.model_enabled(model.provider, &model.model)
                }
                _ => false,
            };
            if !valid {
                return Err(crate::Error::new(
                    "Selected model is not enabled or needs confirmation in Provider settings",
                ));
            }
        }
        let mut candidate = settings.clone();
        self.apply_to(&mut candidate);
        candidate.llm.validate()
    }

    pub fn feedback(&self, saved: bool) -> SettingsFeedback {
        match self {
            Self::ProviderEnabled {
                request_id,
                provider,
                ..
            } => SettingsFeedback::ProviderEnabledCompleted {
                request_id: *request_id,
                provider: *provider,
                saved,
            },
            Self::TranslationSelection { request_id, .. }
            | Self::TranslationOptions { request_id, .. } => {
                SettingsFeedback::TranslationSettingsCompleted {
                    request_id: *request_id,
                    field: self.field(),
                    saved,
                }
            }
            _ if saved => SettingsFeedback::SettingsSaved(self.field()),
            _ => SettingsFeedback::SettingsSaveFailed(self.field()),
        }
    }

    pub fn apply_to(&self, settings: &mut Settings) {
        match self {
            Self::TargetLanguage(value) => settings.target_language = value.clone(),
            Self::Hotkey(value) => settings.hotkey = *value,
            Self::AnnotationHotkey(value) => settings.annotation_hotkey = *value,
            Self::Provider(value) => settings.provider = *value,
            Self::Ollama(value) => settings.ollama = value.clone(),
            Self::Llm(value) => {
                let previous = settings.llm.clone();
                settings.llm = value.clone();
                settings.llm.deepl_enabled = previous.deepl_enabled;
                for c in &mut settings.llm.connections {
                    c.enabled = previous.connection(c.provider).enabled;
                }
            }
            Self::ProviderEnabled {
                provider, enabled, ..
            } => settings.llm.set_provider_enabled(*provider, *enabled),
            Self::TranslationSelection {
                provider, model, ..
            } => {
                settings.provider = *provider;
                if let Some(model) = model {
                    let mut selected = model.clone();
                    if let Some(saved) = settings
                        .llm
                        .models
                        .iter()
                        .find(|m| m.provider == model.provider && m.model == model.model)
                    {
                        selected.parameters = saved.parameters.clone();
                        selected.streaming = saved.streaming;
                    }
                    settings
                        .llm
                        .models
                        .retain(|m| m.provider != model.provider || m.model != model.model);
                    settings.llm.models.push(selected);
                }
            }
            Self::TranslationOptions { model, prompt, .. } => {
                if let Some(saved) = settings
                    .llm
                    .models
                    .iter_mut()
                    .find(|m| m.provider == model.provider && m.model == model.model)
                {
                    saved.parameters = model.parameters.clone();
                    saved.streaming = model.streaming;
                } else {
                    settings.llm.models.insert(0, model.clone());
                }
                settings.llm.prompt = prompt.clone();
            }
            Self::LaunchAtLogin(value) => settings.launch_at_login = *value,
            Self::SelectionToolbar(value) => settings.selection_toolbar = *value,
            Self::ClipboardHistoryEnabled(value) => settings.clipboard_history_enabled = *value,
            Self::ClipboardHistoryLimit(value) => settings.clipboard_history_limit = *value,
            Self::Theme(value) => settings.theme = *value,
            Self::UiLanguage(value) => settings.ui_language = *value,
        }
    }

    pub fn matches(&self, settings: &Settings) -> bool {
        match self {
            Self::TargetLanguage(value) => settings.target_language == *value,
            Self::Hotkey(value) => settings.hotkey == *value,
            Self::AnnotationHotkey(value) => settings.annotation_hotkey == *value,
            Self::Provider(value) => settings.provider == *value,
            Self::Ollama(value) => settings.ollama == *value,
            Self::Llm(value) => settings.llm == *value,
            Self::ProviderEnabled {
                provider, enabled, ..
            } => settings.llm.provider_enabled(*provider) == *enabled,
            Self::TranslationSelection {
                provider, model, ..
            } => {
                settings.provider == *provider
                    && model.as_ref().is_none_or(|m| {
                        settings
                            .llm
                            .selected(m.provider)
                            .is_some_and(|s| s.model == m.model)
                    })
            }
            Self::TranslationOptions { model, prompt, .. } => {
                settings.llm.prompt == *prompt && settings.llm.models.iter().any(|m| m == model)
            }
            Self::LaunchAtLogin(value) => settings.launch_at_login == *value,
            Self::SelectionToolbar(value) => settings.selection_toolbar == *value,
            Self::ClipboardHistoryEnabled(value) => settings.clipboard_history_enabled == *value,
            Self::ClipboardHistoryLimit(value) => settings.clipboard_history_limit == *value,
            Self::Theme(value) => settings.theme == *value,
            Self::UiLanguage(value) => settings.ui_language == *value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub target_language: Language,
    pub hotkey: HotkeyConfig,
    pub annotation_hotkey: HotkeyConfig,
    pub provider: ProviderConfig,
    pub ollama: super::ollama::OllamaConfig,
    pub llm: super::llm::LlmSettings,
    pub launch_at_login: bool,
    pub selection_toolbar: bool,
    pub clipboard_history_enabled: bool,
    pub clipboard_history_limit: u16,
    pub theme: ThemePreference,
    pub ui_language: UiLanguage,
    /// Reference to a system credential. This value is never the credential secret.
    pub deepl_credential_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_language: Language("zh-CN".into()),
            hotkey: HotkeyConfig::default(),
            annotation_hotkey: HotkeyConfig::annotation_default(),
            provider: ProviderConfig::default(),
            ollama: super::ollama::OllamaConfig::default(),
            llm: super::llm::LlmSettings::default(),
            launch_at_login: false,
            selection_toolbar: true,
            clipboard_history_enabled: false,
            clipboard_history_limit: 50,
            theme: ThemePreference::default(),
            ui_language: UiLanguage::default(),
            deepl_credential_id: None,
        }
    }
}

impl Settings {
    pub fn runtime_config(&self) -> RuntimeConfig {
        RuntimeConfig {
            hotkey: self.hotkey,
            annotation_hotkey: self.annotation_hotkey,
            provider: self.provider,
            ollama: self.ollama.clone(),
            llm: self.llm.clone(),
            launch_at_login: self.launch_at_login,
        }
    }
}

#[cfg(test)]
mod translation_change_tests {
    use super::*;
    use crate::domain::llm::*;
    fn fixture() -> Settings {
        let mut settings = Settings::default();
        for id in ["first", "second"] {
            settings.llm.catalog.push(CatalogModel {
                provider: LlmProvider::Ollama,
                model: LlmModel {
                    id: id.into(),
                    aliases: vec![],
                    reasoning: vec!["off".into()],
                    issue: None,
                },
                enabled: true,
                available: true,
                endpoint: settings.llm.connection(LlmProvider::Ollama).endpoint_key(),
            });
            settings.llm.models.push(TranslationModel {
                streaming: true,
                provider: LlmProvider::Ollama,
                model: id.into(),
                parameters: GenerationParameters {
                    temperature: Some(42),
                    ..Default::default()
                },
            });
        }
        settings
    }
    #[test]
    fn atomic_selection_preserves_latest_connections_catalog_and_parameters() {
        let mut settings = fixture();
        settings.llm.prompt = "Latest {target_language}".into();
        let catalog = settings.llm.catalog.clone();
        let connections = settings.llm.connections.clone();
        let change = SettingsChange::TranslationSelection {
            request_id: 1,
            provider: ProviderConfig::Ollama,
            model: Some(TranslationModel {
                streaming: true,
                provider: LlmProvider::Ollama,
                model: "first".into(),
                parameters: GenerationParameters::default(),
            }),
        };
        change.validate_translation_change(&settings).unwrap();
        change.apply_to(&mut settings);
        assert_eq!(settings.provider, ProviderConfig::Ollama);
        assert!(change.matches(&settings));
        assert_eq!(
            settings.llm.selected(LlmProvider::Ollama).unwrap().model,
            "first"
        );
        assert_eq!(
            settings
                .llm
                .selected(LlmProvider::Ollama)
                .unwrap()
                .parameters
                .temperature,
            Some(42)
        );
        assert_eq!(settings.llm.catalog, catalog);
        assert_eq!(settings.llm.connections, connections);
        assert_eq!(settings.llm.prompt, "Latest {target_language}");
    }
    #[test]
    fn options_merge_does_not_select_a_model_or_replace_unrelated_settings() {
        let mut settings = fixture();
        let provider = settings.provider;
        let selected = settings
            .llm
            .selected(LlmProvider::Ollama)
            .unwrap()
            .model
            .clone();
        let mut model = settings.llm.models[0].clone();
        model.parameters.top_p = Some(75);
        let change = SettingsChange::TranslationOptions {
            request_id: 2,
            model,
            prompt: "New {target_language}".into(),
        };
        change.validate_translation_change(&settings).unwrap();
        change.apply_to(&mut settings);
        assert!(change.matches(&settings));
        assert_eq!(settings.provider, provider);
        assert_eq!(
            settings.llm.selected(LlmProvider::Ollama).unwrap().model,
            selected
        );
        assert_eq!(settings.llm.models[1].parameters.top_p, None);
    }
    #[test]
    fn selection_revalidates_disabled_missing_and_changed_endpoints() {
        let mut settings = fixture();
        let change = SettingsChange::TranslationSelection {
            request_id: 1,
            provider: ProviderConfig::Ollama,
            model: Some(settings.llm.models[0].clone()),
        };
        settings.llm.catalog[0].available = false;
        assert!(change.validate_translation_change(&settings).is_err());
        settings.llm.catalog[0].available = true;
        settings.llm.connections[0].enabled = false;
        assert!(change.validate_translation_change(&settings).is_err());
        settings.llm.connections[0].enabled = true;
        settings.llm.catalog[0].endpoint = "old".into();
        assert!(change.validate_translation_change(&settings).is_err());
        assert!(
            SettingsChange::TranslationSelection {
                request_id: 1,
                provider: ProviderConfig::DeepL,
                model: None
            }
            .validate_translation_change(&settings)
            .is_ok()
        );
        settings.llm.deepl_enabled = false;
        assert!(
            SettingsChange::TranslationSelection {
                request_id: 1,
                provider: ProviderConfig::DeepL,
                model: None
            }
            .validate_translation_change(&settings)
            .is_err()
        );
    }
}
