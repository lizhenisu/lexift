use crate::domain::{selection::Selection, translation::TranslateResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Started,
    TranslateRequested,
    SelectionCaptured(Selection),
    TranslationStarted,
    TranslationFinished(TranslateResult),
    TranslationFailed(String),
    PopupHidden,
    ExitRequested,
}
