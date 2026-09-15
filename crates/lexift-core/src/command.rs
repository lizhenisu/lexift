use crate::domain::{
    geometry::Point,
    translation::{TranslateRequest, TranslationTaskId},
};

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
    Exit,
}
