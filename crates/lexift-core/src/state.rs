use crate::{
    AppCommand, AppEvent,
    domain::{
        settings::Settings,
        translation::{TranslateRequest, TranslationTaskId},
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TranslationPhase {
    #[default]
    Idle,
    NoSelection,
    Capturing,
    Translating,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub phase: TranslationPhase,
    pub current_translation_task: Option<TranslationTaskId>,
    pub settings: Settings,
    pub source_text: String,
    pub translated_text: String,
    pub error_message: String,
    next_translation_task: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            phase: TranslationPhase::Idle,
            current_translation_task: None,
            settings,
            source_text: String::new(),
            translated_text: String::new(),
            error_message: String::new(),
            next_translation_task: 0,
        }
    }

    /// Applies a domain event and returns the capabilities the app must execute.
    pub fn reduce(&mut self, event: AppEvent) -> Vec<AppCommand> {
        match event {
            AppEvent::Started => Vec::new(),
            AppEvent::SelectionTranslationRequested => self.request_selection_translation(),
            AppEvent::InputTranslationRequested { text } => self.request_input_translation(text),
            AppEvent::SelectionCaptured { task_id, selection } if self.is_current_task(task_id) => {
                self.source_text = selection.text.clone();
                vec![
                    AppCommand::ShowPopup,
                    AppCommand::Translate {
                        task_id,
                        request: TranslateRequest {
                            text: selection.text,
                            target_language: self.settings.target_language.clone(),
                        },
                    },
                ]
            }
            AppEvent::SelectionCaptureEmpty { task_id } if self.is_current_task(task_id) => {
                self.phase = TranslationPhase::NoSelection;
                self.current_translation_task = None;
                self.source_text.clear();
                self.translated_text.clear();
                self.error_message.clear();
                Vec::new()
            }
            AppEvent::SelectionCaptureFailed { task_id, error }
                if self.is_current_task(task_id) =>
            {
                self.phase = TranslationPhase::Error;
                self.error_message = error;
                self.current_translation_task = None;
                vec![AppCommand::ShowPopup]
            }
            AppEvent::TranslationStarted { task_id } if self.is_current_task(task_id) => {
                self.phase = TranslationPhase::Translating;
                Vec::new()
            }
            AppEvent::TranslationFinished { task_id, result } if self.is_current_task(task_id) => {
                self.phase = TranslationPhase::Success;
                self.translated_text = result.text;
                self.error_message.clear();
                self.current_translation_task = None;
                Vec::new()
            }
            AppEvent::TranslationFailed { task_id, error } if self.is_current_task(task_id) => {
                self.phase = TranslationPhase::Error;
                self.error_message = error;
                self.current_translation_task = None;
                Vec::new()
            }
            AppEvent::SelectionCaptured { .. }
            | AppEvent::SelectionCaptureEmpty { .. }
            | AppEvent::SelectionCaptureFailed { .. }
            | AppEvent::TranslationStarted { .. }
            | AppEvent::TranslationFinished { .. }
            | AppEvent::TranslationFailed { .. } => Vec::new(),
            AppEvent::PopupHidden => {
                self.phase = TranslationPhase::Idle;
                self.current_translation_task = None;
                vec![AppCommand::HidePopup]
            }
            AppEvent::ExitRequested => vec![AppCommand::Exit],
        }
    }

    fn request_selection_translation(&mut self) -> Vec<AppCommand> {
        let task_id = self.begin_translation_task();
        self.phase = TranslationPhase::Capturing;
        self.source_text.clear();
        vec![AppCommand::CaptureSelection { task_id }]
    }

    fn request_input_translation(&mut self, text: String) -> Vec<AppCommand> {
        let text = text.trim();
        if text.is_empty() {
            self.current_translation_task = None;
            self.phase = TranslationPhase::Error;
            self.source_text.clear();
            self.translated_text.clear();
            self.error_message = "Translation text cannot be empty".into();
            return Vec::new();
        }

        let task_id = self.begin_translation_task();
        self.phase = TranslationPhase::Idle;
        self.source_text = text.into();
        vec![AppCommand::Translate {
            task_id,
            request: TranslateRequest {
                text: self.source_text.clone(),
                target_language: self.settings.target_language.clone(),
            },
        }]
    }

    fn begin_translation_task(&mut self) -> TranslationTaskId {
        let task_id = self.next_task_id();
        self.current_translation_task = Some(task_id);
        self.translated_text.clear();
        self.error_message.clear();
        task_id
    }

    fn is_current_task(&self, task_id: TranslationTaskId) -> bool {
        self.current_translation_task == Some(task_id)
    }

    fn next_task_id(&mut self) -> TranslationTaskId {
        self.next_translation_task = self.next_translation_task.wrapping_add(1);
        if self.next_translation_task == 0 {
            self.next_translation_task = 1;
        }
        TranslationTaskId::new(self.next_translation_task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        geometry::Point, language::Language, selection::Selection, translation::TranslateResult,
    };

    #[test]
    fn reduces_the_translation_vertical_slice() {
        let mut state = AppState::default();

        assert_eq!(
            state.reduce(AppEvent::SelectionTranslationRequested),
            vec![AppCommand::CaptureSelection {
                task_id: TranslationTaskId::new(1),
            }]
        );
        assert_eq!(state.phase, TranslationPhase::Capturing);
        assert_eq!(
            state.current_translation_task,
            Some(TranslationTaskId::new(1))
        );

        let commands = state.reduce(AppEvent::SelectionCaptured {
            task_id: TranslationTaskId::new(1),
            selection: Selection {
                text: "Hello world".into(),
                anchor: Some(Point { x: 10, y: 20 }),
            },
        });
        assert_eq!(
            commands,
            vec![
                AppCommand::ShowPopup,
                AppCommand::Translate {
                    task_id: TranslationTaskId::new(1),
                    request: TranslateRequest {
                        text: "Hello world".into(),
                        target_language: Language("zh-CN".into()),
                    },
                },
            ]
        );

        state.reduce(AppEvent::TranslationStarted {
            task_id: TranslationTaskId::new(1),
        });
        assert_eq!(state.phase, TranslationPhase::Translating);

        state.reduce(AppEvent::TranslationFinished {
            task_id: TranslationTaskId::new(1),
            result: TranslateResult {
                text: "你好，世界".into(),
            },
        });
        assert_eq!(state.phase, TranslationPhase::Success);
        assert_eq!(state.current_translation_task, None);
        assert_eq!(state.source_text, "Hello world");
        assert_eq!(state.translated_text, "你好，世界");
    }

    #[test]
    fn uses_the_current_target_language_for_translation_requests() {
        let mut state = AppState::new(Settings {
            target_language: Language("de".into()),
        });
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert_eq!(
            state.reduce(AppEvent::SelectionCaptured {
                task_id: TranslationTaskId::new(1),
                selection: Selection {
                    text: "Hello world".into(),
                    anchor: None,
                },
            }),
            vec![
                AppCommand::ShowPopup,
                AppCommand::Translate {
                    task_id: TranslationTaskId::new(1),
                    request: TranslateRequest {
                        text: "Hello world".into(),
                        target_language: Language("de".into()),
                    },
                },
            ]
        );
    }

    #[test]
    fn input_translation_bypasses_selection() {
        let mut state = AppState::default();

        assert_eq!(
            state.reduce(AppEvent::InputTranslationRequested {
                text: "Hello world".into(),
            }),
            vec![AppCommand::Translate {
                task_id: TranslationTaskId::new(1),
                request: TranslateRequest {
                    text: "Hello world".into(),
                    target_language: Language("zh-CN".into()),
                },
            }]
        );
    }

    #[test]
    fn input_translation_uses_current_settings() {
        let mut state = AppState::new(Settings {
            target_language: Language("ja".into()),
        });

        let commands = state.reduce(AppEvent::InputTranslationRequested {
            text: "Hello".into(),
        });

        let AppCommand::Translate { request, .. } = &commands[0] else {
            panic!("input translation should create a translate command");
        };
        assert_eq!(request.target_language, Language("ja".into()));
    }

    #[test]
    fn input_translation_trims_and_records_source_text() {
        let mut state = AppState::default();

        state.reduce(AppEvent::InputTranslationRequested {
            text: "  Hello world\n".into(),
        });

        assert_eq!(state.source_text, "Hello world");
    }

    #[test]
    fn empty_input_invalidates_current_task_without_translating() {
        let mut state = AppState::default();
        state.reduce(AppEvent::InputTranslationRequested {
            text: "old request".into(),
        });

        let commands = state.reduce(AppEvent::InputTranslationRequested {
            text: " \n\t ".into(),
        });

        assert!(commands.is_empty());
        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.current_translation_task, None);
        assert!(state.source_text.is_empty());
        assert!(state.translated_text.is_empty());
        assert_eq!(state.error_message, "Translation text cannot be empty");
    }

    #[test]
    fn newer_input_replaces_the_current_task() {
        let mut state = AppState::default();
        state.reduce(AppEvent::InputTranslationRequested {
            text: "Input A".into(),
        });
        state.reduce(AppEvent::InputTranslationRequested {
            text: "Input B".into(),
        });

        assert_eq!(
            state.current_translation_task,
            Some(TranslationTaskId::new(2))
        );
        assert_eq!(state.source_text, "Input B");
    }

    #[test]
    fn stale_input_result_does_not_overwrite_the_newest_result() {
        let mut state = AppState::default();
        state.reduce(AppEvent::InputTranslationRequested {
            text: "Input A".into(),
        });
        state.reduce(AppEvent::InputTranslationRequested {
            text: "Input B".into(),
        });

        state.reduce(AppEvent::TranslationFinished {
            task_id: TranslationTaskId::new(1),
            result: TranslateResult {
                text: "stale result".into(),
            },
        });
        assert!(state.translated_text.is_empty());

        state.reduce(AppEvent::TranslationFinished {
            task_id: TranslationTaskId::new(2),
            result: TranslateResult {
                text: "current result".into(),
            },
        });
        assert_eq!(state.phase, TranslationPhase::Success);
        assert_eq!(state.translated_text, "current result");
    }

    #[test]
    fn newer_requests_replace_the_current_task() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert_eq!(
            state.reduce(AppEvent::SelectionTranslationRequested),
            vec![AppCommand::CaptureSelection {
                task_id: TranslationTaskId::new(2),
            }]
        );
        assert_eq!(state.phase, TranslationPhase::Capturing);
        assert_eq!(
            state.current_translation_task,
            Some(TranslationTaskId::new(2))
        );
    }

    #[test]
    fn records_translation_failures() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);
        state.reduce(AppEvent::TranslationFailed {
            task_id: TranslationTaskId::new(1),
            error: "provider unavailable".into(),
        });

        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.error_message, "provider unavailable");
    }

    #[test]
    fn selection_capture_failure_opens_the_popup_with_an_error() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert_eq!(
            state.reduce(AppEvent::SelectionCaptureFailed {
                task_id: TranslationTaskId::new(1),
                error: "No selected text was found".into(),
            }),
            vec![AppCommand::ShowPopup]
        );
        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.current_translation_task, None);
        assert_eq!(state.error_message, "No selected text was found");
    }

    #[test]
    fn empty_selection_returns_to_idle_without_showing_the_popup() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert!(
            state
                .reduce(AppEvent::SelectionCaptureEmpty {
                    task_id: TranslationTaskId::new(1),
                })
                .is_empty()
        );
        assert_eq!(state.phase, TranslationPhase::NoSelection);
        assert_eq!(state.current_translation_task, None);
        assert!(state.error_message.is_empty());
    }

    #[test]
    fn input_translation_failure_does_not_open_the_selection_popup() {
        let mut state = AppState::default();
        state.reduce(AppEvent::InputTranslationRequested {
            text: "Hello".into(),
        });

        assert!(
            state
                .reduce(AppEvent::TranslationFailed {
                    task_id: TranslationTaskId::new(1),
                    error: "provider unavailable".into(),
                })
                .is_empty()
        );
        assert_eq!(state.phase, TranslationPhase::Error);
    }

    #[test]
    fn ignores_results_from_superseded_tasks() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert!(
            state
                .reduce(AppEvent::TranslationFinished {
                    task_id: TranslationTaskId::new(1),
                    result: TranslateResult {
                        text: "stale result".into(),
                    },
                })
                .is_empty()
        );
        assert_eq!(state.phase, TranslationPhase::Capturing);
        assert!(state.translated_text.is_empty());
        assert_eq!(
            state.current_translation_task,
            Some(TranslationTaskId::new(2))
        );

        state.reduce(AppEvent::TranslationFinished {
            task_id: TranslationTaskId::new(2),
            result: TranslateResult {
                text: "current result".into(),
            },
        });
        assert_eq!(state.phase, TranslationPhase::Success);
        assert_eq!(state.translated_text, "current result");
    }

    #[test]
    fn ignores_stale_capture_and_failure_events() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert!(
            state
                .reduce(AppEvent::SelectionCaptured {
                    task_id: TranslationTaskId::new(1),
                    selection: Selection {
                        text: "stale selection".into(),
                        anchor: None,
                    },
                })
                .is_empty()
        );
        state.reduce(AppEvent::TranslationFailed {
            task_id: TranslationTaskId::new(1),
            error: "stale error".into(),
        });
        state.reduce(AppEvent::SelectionCaptureFailed {
            task_id: TranslationTaskId::new(1),
            error: "stale capture error".into(),
        });

        assert_eq!(state.phase, TranslationPhase::Capturing);
        assert!(state.source_text.is_empty());
        assert!(state.error_message.is_empty());
    }
}
