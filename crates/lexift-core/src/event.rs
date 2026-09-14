use crate::domain::{
    selection::Selection,
    translation::{TranslateResult, TranslationTaskId},
};

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
    ExitRequested,
}
