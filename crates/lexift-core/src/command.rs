use crate::domain::{
    geometry::Point,
    language::Language,
    runtime_config::RuntimeConfig,
    settings::{Settings, SettingsChange, SettingsFeedback},
    translation::{PopupSessionId, TranslateRequest, TranslationTaskId},
};
use crate::ports::credential::{CredentialAccessPurpose, CredentialSecret};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    CaptureSelection {
        task_id: TranslationTaskId,
    },
    CaptureToolbarSelection {
        generation: u64,
        anchor: Point,
    },
    CopyToolbarText {
        text: String,
    },
    Translate {
        task_id: TranslationTaskId,
        request: TranslateRequest,
    },
    TranslatePopup {
        session_id: PopupSessionId,
        task_id: TranslationTaskId,
        request: TranslateRequest,
    },
    ShowPopup {
        session_id: PopupSessionId,
        anchor: Option<Point>,
    },
    HidePopup {
        session_id: PopupSessionId,
    },
    CopyPopupText {
        session_id: PopupSessionId,
        text: String,
    },
    SpeakPopupText {
        session_id: PopupSessionId,
        source: bool,
        text: String,
        language: Option<Language>,
    },
    StopPopupSpeech {
        session_id: PopupSessionId,
    },
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
