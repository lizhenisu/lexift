use crate::domain::translation::TranslateRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    CaptureSelection,
    Translate(TranslateRequest),
    ShowPopup,
    HidePopup,
    Exit,
}
