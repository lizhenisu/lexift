use lexift_core::domain::{
    language::Language,
    runtime_config::{HotkeyConfig, HotkeyKey, HotkeyModifiers, ProviderConfig},
    settings::{Settings, ThemePreference},
};
use serde::{Deserialize, Serialize};

use crate::migration::CURRENT_SCHEMA_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LlmFile {
    connections: Vec<ConnectionFile>,
    models: Vec<ModelFile>,
    prompt: String,
    #[serde(default)]
    catalog: Option<Vec<CatalogFile>>,
    #[serde(default = "enabled_default")]
    deepl_enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ConnectionFile {
    provider: String,
    address: String,
    credential_id: Option<String>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default = "native_protocol")]
    protocol: String,
    #[serde(default)]
    protocols: Vec<ProtocolFile>,
}
fn enabled_default() -> bool {
    true
}
fn native_protocol() -> String {
    "native".into()
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProtocolFile {
    protocol: String,
    base_url: String,
    authentication: String,
    headers: Vec<HeaderFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HeaderFile {
    name: String,
    credential_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CatalogFile {
    provider: String,
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    reasoning: Vec<String>,
    issue: Option<String>,
    enabled: bool,
    available: bool,
    endpoint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ModelFile {
    #[serde(default = "default_streaming")]
    streaming: bool,
    provider: String,
    model: String,
    reasoning: String,
    temperature: Option<u16>,
    top_p: Option<u16>,
}
fn default_streaming() -> bool {
    true
}
impl LlmFile {
    fn from_domain(s: &lexift_core::domain::llm::LlmSettings) -> Self {
        Self {
            connections: s
                .connections
                .iter()
                .map(|c| ConnectionFile {
                    provider: c.provider.id().into(),
                    address: c.address.clone(),
                    credential_id: c.credential_id.clone(),
                    enabled: c.enabled,
                    protocol: c.protocol.id().into(),
                    protocols: c
                        .protocols
                        .iter()
                        .map(|p| ProtocolFile {
                            protocol: p.protocol.id().into(),
                            base_url: p.base_url.clone(),
                            authentication: format!("{:?}", p.authentication),
                            headers: p
                                .headers
                                .iter()
                                .map(|h| HeaderFile {
                                    name: h.name.clone(),
                                    credential_id: h.credential_id.clone(),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
            models: s
                .models
                .iter()
                .map(|m| ModelFile {
                    provider: m.provider.id().into(),
                    model: m.model.clone(),
                    streaming: m.streaming,
                    reasoning: m.parameters.reasoning.clone(),
                    temperature: m.parameters.temperature,
                    top_p: m.parameters.top_p,
                })
                .collect(),
            prompt: s.prompt.clone(),
            deepl_enabled: s.deepl_enabled,
            catalog: Some(
                s.catalog
                    .iter()
                    .map(|m| CatalogFile {
                        provider: m.provider.id().into(),
                        id: m.model.id.clone(),
                        aliases: m.model.aliases.clone(),
                        reasoning: m.model.reasoning.clone(),
                        issue: m.model.issue.clone(),
                        enabled: m.enabled,
                        available: m.available,
                        endpoint: m.endpoint.clone(),
                    })
                    .collect(),
            ),
        }
    }
    fn into_domain(self) -> lexift_core::Result<lexift_core::domain::llm::LlmSettings> {
        use lexift_core::domain::llm::*;
        let legacy_catalog = self.catalog.is_none();
        let mut settings = LlmSettings {
            connections: self
                .connections
                .into_iter()
                .map(|c| {
                    Ok(LlmConnection {
                        provider: LlmProvider::parse(&c.provider)?,
                        address: c.address,
                        credential_id: c.credential_id,
                        enabled: c.enabled,
                        protocol: LlmProtocol::parse(&c.protocol)?,
                        protocols: c
                            .protocols
                            .into_iter()
                            .map(|p| {
                                Ok(ProtocolSettings {
                                    protocol: LlmProtocol::parse(&p.protocol)?,
                                    base_url: p.base_url,
                                    authentication: match p.authentication.as_str() {
                                        "Automatic" => Authentication::Automatic,
                                        "Headers" => Authentication::Headers,
                                        "None" => Authentication::None,
                                        _ => {
                                            return Err(lexift_core::Error::new(
                                                "Invalid authentication mode",
                                            ));
                                        }
                                    },
                                    headers: p
                                        .headers
                                        .into_iter()
                                        .map(|h| HeaderReference {
                                            name: h.name,
                                            credential_id: h.credential_id,
                                        })
                                        .collect(),
                                })
                            })
                            .collect::<lexift_core::Result<_>>()?,
                    })
                })
                .collect::<lexift_core::Result<_>>()?,
            models: self
                .models
                .into_iter()
                .map(|m| {
                    Ok(TranslationModel {
                        streaming: m.streaming,
                        provider: LlmProvider::parse(&m.provider)?,
                        model: m.model,
                        parameters: GenerationParameters {
                            reasoning: m.reasoning,
                            temperature: m.temperature,
                            top_p: m.top_p,
                        },
                    })
                })
                .collect::<lexift_core::Result<_>>()?,
            prompt: self.prompt,
            deepl_enabled: self.deepl_enabled,
            catalog: self
                .catalog
                .unwrap_or_default()
                .into_iter()
                .map(|m| {
                    Ok(CatalogModel {
                        provider: LlmProvider::parse(&m.provider)?,
                        model: LlmModel {
                            id: m.id,
                            aliases: m.aliases,
                            reasoning: m.reasoning,
                            issue: m.issue,
                        },
                        enabled: m.enabled,
                        available: m.available,
                        endpoint: m.endpoint,
                    })
                })
                .collect::<lexift_core::Result<_>>()?,
        };
        if legacy_catalog {
            for m in &settings.models {
                settings.catalog.push(CatalogModel {
                    provider: m.provider,
                    model: LlmModel {
                        id: m.model.clone(),
                        aliases: Vec::new(),
                        reasoning: Vec::new(),
                        issue: None,
                    },
                    enabled: true,
                    available: true,
                    endpoint: settings.connection(m.provider).endpoint_key(),
                });
            }
        }
        settings.validate()?;
        Ok(settings)
    }
}

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
    #[serde(default = "default_annotation_hotkey")]
    annotation_hotkey: HotkeyFile,
    #[serde(default = "default_ocr_hotkey")]
    ocr_hotkey: HotkeyFile,
    #[serde(default)]
    ocr_language: Option<String>,
    #[serde(default = "default_true")]
    ocr_enabled: bool,
    #[serde(default)]
    ocr_korean_auto: bool,
    #[serde(default = "default_provider")]
    provider: String,
    #[serde(default)]
    ollama: OllamaFile,
    #[serde(default)]
    llm: Option<LlmFile>,
    #[serde(default)]
    launch_at_login: bool,
    #[serde(default = "default_selection_toolbar")]
    selection_toolbar: bool,
    #[serde(default)]
    clipboard_history_enabled: bool,
    #[serde(default = "default_history_limit")]
    clipboard_history_limit: u16,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OllamaFile {
    address: String,
    #[serde(default)]
    model: String,
}
impl Default for OllamaFile {
    fn default() -> Self {
        let c = lexift_core::domain::ollama::OllamaConfig::default();
        Self {
            address: c.address,
            model: c.model,
        }
    }
}
impl OllamaFile {
    fn into_config(self) -> lexift_core::Result<lexift_core::domain::ollama::OllamaConfig> {
        let c = lexift_core::domain::ollama::OllamaConfig {
            address: self.address,
            model: self.model,
        };
        c.validate()?;
        Ok(c)
    }
}

impl Default for HotkeyFile {
    fn default() -> Self {
        Self::from(HotkeyConfig::default())
    }
}

fn default_ocr_hotkey() -> HotkeyFile {
    HotkeyConfig::ocr_default().into()
}

fn default_annotation_hotkey() -> HotkeyFile {
    HotkeyFile::from(HotkeyConfig::annotation_default())
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

fn default_history_limit() -> u16 {
    50
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
                annotation_hotkey: settings.annotation_hotkey.into(),
                ocr_hotkey: settings.ocr_hotkey.into(),
                ocr_language: settings.ocr_language.clone(),
                ocr_enabled: settings.ocr_enabled,
                ocr_korean_auto: settings.ocr_korean_auto,
                provider: settings.provider.id().into(),
                ollama: OllamaFile {
                    address: settings.ollama.address.clone(),
                    model: settings.ollama.model.clone(),
                },
                llm: Some(LlmFile::from_domain(&settings.llm)),
                launch_at_login: settings.launch_at_login,
                selection_toolbar: settings.selection_toolbar,
                clipboard_history_enabled: settings.clipboard_history_enabled,
                clipboard_history_limit: settings.clipboard_history_limit,
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
        let annotation_key: HotkeyKey = self.settings.annotation_hotkey.key.parse()?;
        let annotation_hotkey = HotkeyConfig::new(
            HotkeyModifiers {
                control: self.settings.annotation_hotkey.control,
                alt: self.settings.annotation_hotkey.alt,
                shift: self.settings.annotation_hotkey.shift,
                meta: self.settings.annotation_hotkey.meta,
            },
            annotation_key,
        )?;
        let ollama = self.settings.ollama.into_config()?;
        let llm = match self.settings.llm {
            Some(file) => file.into_domain()?,
            None => {
                let mut settings = lexift_core::domain::llm::LlmSettings::default();
                settings.connections[0].address = ollama.address.clone();
                if !ollama.model.is_empty() {
                    settings
                        .models
                        .push(lexift_core::domain::llm::TranslationModel {
                            streaming: true,
                            provider: lexift_core::domain::llm::LlmProvider::Ollama,
                            model: ollama.model.clone(),
                            parameters: Default::default(),
                        });
                }
                for m in &settings.models {
                    settings
                        .catalog
                        .push(lexift_core::domain::llm::CatalogModel {
                            provider: m.provider,
                            model: lexift_core::domain::llm::LlmModel {
                                id: m.model.clone(),
                                aliases: Vec::new(),
                                reasoning: Vec::new(),
                                issue: None,
                            },
                            enabled: true,
                            available: true,
                            endpoint: settings.connection(m.provider).endpoint_key(),
                        });
                }
                settings
            }
        };
        if let Some(language) = &self.settings.ocr_language {
            lexift_core::domain::settings::SettingsChange::OcrLanguage(Some(language.clone()))
                .validate_translation_change(&Settings::default())?;
        }
        Ok(Settings {
            target_language: Language(self.settings.target_language),
            ui_language: self.settings.ui_language.parse()?,
            hotkey,
            annotation_hotkey,
            ocr_hotkey: HotkeyConfig::new(
                lexift_core::domain::runtime_config::HotkeyModifiers {
                    control: self.settings.ocr_hotkey.control,
                    alt: self.settings.ocr_hotkey.alt,
                    shift: self.settings.ocr_hotkey.shift,
                    meta: self.settings.ocr_hotkey.meta,
                },
                self.settings.ocr_hotkey.key.parse()?,
            )?,
            ocr_language: self.settings.ocr_language,
            ocr_enabled: self.settings.ocr_enabled,
            ocr_korean_auto: self.settings.ocr_korean_auto,
            provider: self.settings.provider.parse()?,
            ollama,
            llm,
            launch_at_login: self.settings.launch_at_login,
            selection_toolbar: self.settings.selection_toolbar,
            clipboard_history_enabled: self.settings.clipboard_history_enabled,
            clipboard_history_limit: self.settings.clipboard_history_limit.clamp(10, 200),
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

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::ui_language::UiLanguage;
    #[test]
    fn ocr_enablement_defaults_on_and_round_trips_without_moving_existing_fields() {
        let legacy =
            "schema_version = 8\n[settings]\ntarget_language = 'zh-CN'\nocr_language = 'ja'\n";
        let mut settings = toml::from_str::<ConfigFile>(legacy)
            .unwrap()
            .into_settings()
            .unwrap();
        assert!(settings.ocr_enabled);
        settings.ocr_enabled = false;
        let serialized = toml::to_string(&ConfigFile::from_settings(&settings)).unwrap();
        let restored = toml::from_str::<ConfigFile>(&serialized)
            .unwrap()
            .into_settings()
            .unwrap();
        assert!(!restored.ocr_enabled);
        assert_eq!(restored.ocr_language.as_deref(), Some("ja"));
    }
    #[test]
    fn v7_migration_enables_existing_choice_and_v8_catalog_round_trips() {
        let legacy = "schema_version = 7\n[settings]\ntarget_language = 'zh-CN'\nprovider = 'lmstudio'\n[settings.llm]\nprompt = 'Translate to {target_language}'\n[[settings.llm.connections]]\nprovider = 'lmstudio'\naddress = 'http://127.0.0.1:52341'\ncredential_id = 'key-reference'\n[[settings.llm.models]]\nprovider = 'lmstudio'\nmodel = 'service-real-id'\nreasoning = 'off'\ntemperature = 25\n";
        let mut settings = toml::from_str::<ConfigFile>(legacy)
            .unwrap()
            .into_settings()
            .unwrap();
        use lexift_core::domain::llm::*;
        assert!(
            settings
                .llm
                .model_enabled(LlmProvider::LmStudio, "service-real-id")
        );
        assert_eq!(
            settings
                .llm
                .selected(LlmProvider::LmStudio)
                .unwrap()
                .parameters
                .temperature,
            Some(25)
        );
        let mut c = settings.llm.connection(LlmProvider::LmStudio);
        c.protocol = LlmProtocol::Responses;
        c.enabled = false;
        c.protocols.push(ProtocolSettings {
            protocol: LlmProtocol::Responses,
            base_url: "https://gateway.example/v1".into(),
            authentication: Authentication::Headers,
            headers: vec![HeaderReference {
                name: "Authorization".into(),
                credential_id: "header-reference".into(),
            }],
        });
        settings.llm.connections = vec![c];
        settings.llm.catalog[0].available = false;
        let file = ConfigFile::from_settings(&settings);
        let serialized = toml::to_string(&file).unwrap();
        assert!(serialized.contains("header-reference"));
        assert!(!serialized.contains("Bearer "));
        assert_eq!(file.into_settings().unwrap(), settings);
    }
    #[test]
    fn v6_ollama_migrates_into_separate_service_and_translation_settings() {
        use lexift_core::domain::llm::LlmProvider;
        let file: ConfigFile = toml::from_str("schema_version = 6\n[settings]\ntarget_language = 'zh-CN'\nprovider = 'ollama'\n[settings.ollama]\naddress = 'http://127.0.0.1:11434'\nmodel = 'saved-model'\n").unwrap();
        let settings = file.into_settings().unwrap();
        assert_eq!(
            settings.llm.connection(LlmProvider::Ollama).address,
            settings.ollama.address
        );
        assert_eq!(
            settings.llm.selected(LlmProvider::Ollama).unwrap().model,
            "saved-model"
        );
        assert_eq!(
            settings
                .llm
                .selected(LlmProvider::Ollama)
                .unwrap()
                .parameters,
            Default::default()
        );
        assert_eq!(
            ConfigFile::from_settings(&settings)
                .into_settings()
                .unwrap(),
            settings
        );
    }
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
    #[test]
    fn streaming_is_per_model_and_missing_legacy_field_defaults_on() {
        let mut settings = Settings::default();
        for (name, streaming) in [("one", false), ("two", true)] {
            settings
                .llm
                .models
                .push(lexift_core::domain::llm::TranslationModel {
                    provider: lexift_core::domain::llm::LlmProvider::LmStudio,
                    model: name.into(),
                    streaming,
                    parameters: Default::default(),
                });
        }
        let text = toml::to_string(&ConfigFile::from_settings(&settings)).unwrap();
        let file: ConfigFile = toml::from_str(&text).unwrap();
        assert_eq!(file.into_settings().unwrap(), settings);
        let legacy = text
            .lines()
            .filter(|line| !line.starts_with("streaming"))
            .collect::<Vec<_>>()
            .join("\n");
        let loaded: ConfigFile = toml::from_str(&legacy).unwrap();
        assert!(
            loaded
                .into_settings()
                .unwrap()
                .llm
                .models
                .iter()
                .all(|m| m.streaming)
        );
    }
    #[test]
    fn korean_auto_defaults_off_and_round_trips_independently() {
        let file = ConfigFile::from_settings(&Settings::default());
        let text = toml::to_string(&file).unwrap();
        let old = text
            .lines()
            .filter(|line| !line.starts_with("ocr_korean_auto"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: ConfigFile = toml::from_str(&old).unwrap();
        assert!(!parsed.into_settings().unwrap().ocr_korean_auto);
        let settings = Settings {
            ocr_korean_auto: true,
            ..Settings::default()
        };
        let encoded = toml::to_string(&ConfigFile::from_settings(&settings)).unwrap();
        let decoded: ConfigFile = toml::from_str(&encoded).unwrap();
        assert!(decoded.into_settings().unwrap().ocr_korean_auto);
    }
}
