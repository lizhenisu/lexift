use crate::domain::{
    geometry::Point,
    settings::Settings,
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
    PersistSettings {
        settings: Settings,
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
