use crate::domain::{
    language::Language,
    runtime_config::RuntimeConfig,
    selection::Selection,
    settings::{Settings, SettingsChange},
    translation::{PopupSessionId, TranslateResult, TranslationTaskId},
};
use crate::ports::credential::{CredentialAccessPurpose, CredentialSecret};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Started,
    SelectionTranslationRequested,
    SelectionInteractionStarted,
    SelectionGestureCompleted {
        anchor: crate::domain::geometry::Point,
    },
    SelectionToolbarCaptured {
        generation: u64,
        selection: Selection,
    },
    SelectionToolbarCaptureEmpty {
        generation: u64,
    },
    SelectionToolbarTranslateRequested,
    SelectionToolbarCopyRequested,
    SelectionToolbarDismissRequested,
    InputTranslationRequested {
        text: String,
    },
    SelectionCaptured {
        task_id: TranslationTaskId,
        selection: Selection,
    },
    SelectionCaptureEmpty {
        task_id: TranslationTaskId,
    },
    SelectionCaptureFailed {
        task_id: TranslationTaskId,
        error: String,
    },
    TranslationStarted {
        task_id: TranslationTaskId,
    },
    TranslationFinished {
        task_id: TranslationTaskId,
        result: TranslateResult,
    },
    TranslationFailed {
        task_id: TranslationTaskId,
        error: String,
    },
    PopupTranslationRequested {
        session_id: PopupSessionId,
        text: String,
        source_language: Option<Language>,
        target_language: Language,
    },
    PopupTranslationStarted {
        session_id: PopupSessionId,
        task_id: TranslationTaskId,
    },
    PopupTranslationFinished {
        session_id: PopupSessionId,
        task_id: TranslationTaskId,
        result: TranslateResult,
    },
    PopupTranslationFailed {
        session_id: PopupSessionId,
        task_id: TranslationTaskId,
        error: String,
    },
    PopupPinChanged {
        session_id: PopupSessionId,
        pinned: bool,
    },
    PopupClosed {
        session_id: PopupSessionId,
    },
    PopupCopyRequested {
        session_id: PopupSessionId,
        text: String,
    },
    PopupCopyFinished {
        session_id: PopupSessionId,
        error: Option<String>,
    },
    PopupFeedbackCleared {
        session_id: PopupSessionId,
    },
    PopupSpeechRequested {
        session_id: PopupSessionId,
        source: bool,
        text: String,
        language: Option<Language>,
    },
    PopupSpeechStateChanged {
        session_id: PopupSessionId,
        source: bool,
        speaking: bool,
        error: Option<String>,
    },
    MainWindowRequested,
    SettingsWindowRequested,
    SettingsChangeRequested {
        change: SettingsChange,
    },
    RuntimeConfigChanged {
        settings: Settings,
        config: RuntimeConfig,
        change: SettingsChange,
    },
    RuntimeConfigUpdated {
        settings: Settings,
        config: RuntimeConfig,
        change: SettingsChange,
    },
    RuntimeConfigUpdateFailed {
        change: SettingsChange,
        error: String,
    },
    SettingsSaveFailed {
        change: SettingsChange,
        error: String,
    },
    CredentialSaveRequested {
        secret: CredentialSecret,
    },
    CredentialSaved {
        credential_id: String,
    },
    CredentialSaveFailed {
        error: String,
    },
    CredentialRemoveRequested,
    CredentialRemoved,
    CredentialRemoveFailed {
        error: String,
    },
    CredentialAccessRequested {
        purpose: CredentialAccessPurpose,
        generation: u64,
    },
    CredentialAccessSucceeded {
        purpose: CredentialAccessPurpose,
    },
    CredentialAccessFailed {
        error: String,
    },
    ExitRequested,
}
