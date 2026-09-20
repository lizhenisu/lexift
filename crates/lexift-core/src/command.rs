use crate::domain::{
    geometry::Point,
    runtime_config::RuntimeConfig,
    settings::{Settings, SettingsChange, SettingsFeedback},
    translation::{TranslateRequest, TranslationTaskId},
};
use crate::ports::credential::{CredentialAccessPurpose, CredentialSecret};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    CaptureSelection {
        task_id: TranslationTaskId,
    },
    Translate {
        task_id: TranslationTaskId,
        request: TranslateRequest,
    },
    ShowPopup {
        anchor: Option<Point>,
    },
    HidePopup,
    ShowMainWindow,
    ShowSettingsWindow,
    HideSettingsWindow,
    ShowSettingsFeedback {
        feedback: SettingsFeedback,
    },
    PersistSettings {
        settings: Settings,
        change: SettingsChange,
    },
    ApplyRuntimeConfig {
        settings: Settings,
        previous_settings: Settings,
        config: RuntimeConfig,
        change: SettingsChange,
    },
    PersistCredential {
        credential_id: String,
        secret: CredentialSecret,
    },
    RemoveCredential {
        credential_id: String,
    },
    AccessCredential {
        credential_id: String,
        purpose: CredentialAccessPurpose,
        generation: u64,
    },
    ClearCredentialDraft,
    Exit,
}
