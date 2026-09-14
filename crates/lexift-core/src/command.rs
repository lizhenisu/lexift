use crate::domain::translation::{TranslateRequest, TranslationTaskId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    CaptureSelection {
        task_id: TranslationTaskId,
    },
    Translate {
        task_id: TranslationTaskId,
        request: TranslateRequest,
    },
    ShowPopup,
    HidePopup,
    Exit,
}
