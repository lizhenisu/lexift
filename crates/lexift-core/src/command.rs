use crate::domain::{
    geometry::Point,
    settings::Settings,
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
    ShowMainWindow,
    ShowSettingsWindow,
    HideSettingsWindow,
    PersistSettings {
        settings: Settings,
    },
    Exit,
}
