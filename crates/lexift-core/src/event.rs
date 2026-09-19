use crate::domain::{
    selection::Selection,
    settings::Settings,
    translation::{TranslateResult, TranslationTaskId},
};
use crate::ports::credential::{CredentialAccessPurpose, CredentialSecret};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Started,
    SelectionTranslationRequested,
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
    PopupHidden,
    MainWindowRequested,
    SettingsWindowRequested,
    SettingsSaveRequested {
        settings: Settings,
    },
    SettingsSaved {
        settings: Settings,
    },
    SettingsSaveFailed {
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
