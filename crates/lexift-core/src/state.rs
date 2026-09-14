use crate::{
    AppCommand, AppEvent,
    domain::{language::Language, translation::TranslateRequest},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TranslationPhase {
    #[default]
    Idle,
    Capturing,
    Translating,
    Success,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub phase: TranslationPhase,
    pub source_text: String,
    pub translated_text: String,
    pub error_message: String,
}

impl AppState {
    /// Applies a domain event and returns the capabilities the app must execute.
    pub fn reduce(&mut self, event: AppEvent) -> Vec<AppCommand> {
        match event {
            AppEvent::Started => Vec::new(),
            AppEvent::TranslateRequested => self.request_translation(),
            AppEvent::SelectionCaptured(selection) => {
                self.source_text = selection.text.clone();
                vec![AppCommand::Translate(TranslateRequest {
                    text: selection.text,
                    target_language: Language("zh-CN".into()),
                })]
            }
            AppEvent::TranslationStarted => {
                self.phase = TranslationPhase::Translating;
                Vec::new()
            }
            AppEvent::TranslationFinished(result) => {
                self.phase = TranslationPhase::Success;
                self.translated_text = result.text;
                self.error_message.clear();
                Vec::new()
            }
            AppEvent::TranslationFailed(message) => {
                self.phase = TranslationPhase::Error;
                self.error_message = message;
                Vec::new()
            }
            AppEvent::PopupHidden => {
                self.phase = TranslationPhase::Idle;
                vec![AppCommand::HidePopup]
            }
            AppEvent::ExitRequested => vec![AppCommand::Exit],
        }
    }

    fn request_translation(&mut self) -> Vec<AppCommand> {
        if matches!(
            self.phase,
            TranslationPhase::Capturing | TranslationPhase::Translating
        ) {
            return Vec::new();
        }

        self.phase = TranslationPhase::Capturing;
        self.source_text.clear();
        self.translated_text.clear();
        self.error_message.clear();
        vec![AppCommand::ShowPopup, AppCommand::CaptureSelection]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{geometry::Point, selection::Selection, translation::TranslateResult};

    #[test]
    fn reduces_the_translation_vertical_slice() {
        let mut state = AppState::default();

        assert_eq!(
            state.reduce(AppEvent::TranslateRequested),
            vec![AppCommand::ShowPopup, AppCommand::CaptureSelection]
        );
        assert_eq!(state.phase, TranslationPhase::Capturing);

        let commands = state.reduce(AppEvent::SelectionCaptured(Selection {
            text: "Hello world".into(),
            anchor: Some(Point { x: 10, y: 20 }),
        }));
        assert_eq!(
            commands,
            vec![AppCommand::Translate(TranslateRequest {
                text: "Hello world".into(),
                target_language: Language("zh-CN".into()),
            })]
        );

        state.reduce(AppEvent::TranslationStarted);
        assert_eq!(state.phase, TranslationPhase::Translating);

        state.reduce(AppEvent::TranslationFinished(TranslateResult {
            text: "你好，世界".into(),
        }));
        assert_eq!(state.phase, TranslationPhase::Success);
        assert_eq!(state.source_text, "Hello world");
        assert_eq!(state.translated_text, "你好，世界");
    }

    #[test]
    fn ignores_duplicate_requests_while_work_is_in_progress() {
        let mut state = AppState::default();
        state.reduce(AppEvent::TranslateRequested);

        assert!(state.reduce(AppEvent::TranslateRequested).is_empty());
        assert_eq!(state.phase, TranslationPhase::Capturing);
    }

    #[test]
    fn records_translation_failures() {
        let mut state = AppState::default();
        state.reduce(AppEvent::TranslateRequested);
        state.reduce(AppEvent::TranslationFailed("provider unavailable".into()));

        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.error_message, "provider unavailable");
    }
}
