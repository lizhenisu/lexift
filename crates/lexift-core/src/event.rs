use crate::domain::{
    selection::Selection,
    translation::{TranslateResult, TranslationTaskId},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Started,
    TranslateRequested,
    SelectionCaptured {
        task_id: TranslationTaskId,
        selection: Selection,
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
