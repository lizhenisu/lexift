//! Stable capability identities for provider settings, independent of translation selection.
use super::{runtime_config::ProviderConfig, settings::Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceProviderId {
    Translation(ProviderConfig),
    PaddleOcr,
}
impl ServiceProviderId {
    pub const ALL: [Self; 5] = [
        Self::Translation(ProviderConfig::DeepL),
        Self::Translation(ProviderConfig::Ollama),
        Self::Translation(ProviderConfig::LmStudio),
        Self::Translation(ProviderConfig::DeepSeek),
        Self::PaddleOcr,
    ];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Translation(provider) => provider.id(),
            Self::PaddleOcr => "paddleocr",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
    pub fn enabled(self, settings: &Settings) -> bool {
        match self {
            Self::Translation(p) => settings.llm.provider_enabled(p),
            Self::PaddleOcr => settings.ocr_enabled,
        }
    }
    pub fn set_enabled(self, settings: &mut Settings, enabled: bool) {
        match self {
            Self::Translation(p) => settings.llm.set_provider_enabled(p, enabled),
            Self::PaddleOcr => settings.ocr_enabled = enabled,
        }
    }
}
impl From<ProviderConfig> for ServiceProviderId {
    fn from(value: ProviderConfig) -> Self {
        Self::Translation(value)
    }
}
