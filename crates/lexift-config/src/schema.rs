use lexift_core::domain::{language::Language, settings::Settings};
use serde::{Deserialize, Serialize};

use crate::migration::CURRENT_SCHEMA_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ConfigFile {
    pub(crate) schema_version: u32,
    pub(crate) settings: SettingsFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SettingsFile {
    target_language: String,
}

impl ConfigFile {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            settings: SettingsFile {
                target_language: settings.target_language.0.clone(),
            },
        }
    }

    pub(crate) fn into_settings(self) -> Settings {
        Settings {
            target_language: Language(self.settings.target_language),
        }
    }
}
