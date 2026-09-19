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
    pub settings_saving: bool,
    pub settings_error_message: String,
    pub credential_configured: bool,
    pub credential_busy: bool,
    pub credential_error_message: String,
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
            settings_saving: false,
            settings_error_message: String::new(),
            credential_configured: false,
            credential_busy: false,
            credential_error_message: String::new(),
            next_translation_task: 0,
        }
    }

    pub fn with_credential_status(settings: Settings, credential_configured: bool) -> Self {
        let mut state = Self::new(settings);
        state.credential_configured = credential_configured;
        state
    }

    /// Applies a domain event and returns the capabilities the app must execute.
    pub fn reduce(&mut self, event: AppEvent) -> Vec<AppCommand> {
        match event {
            AppEvent::Started => Vec::new(),
            AppEvent::SelectionTranslationRequested => self.request_selection_translation(),
            AppEvent::InputTranslationRequested { text } => self.request_input_translation(text),
            AppEvent::SelectionCaptured { task_id, selection } if self.is_current_task(task_id) => {
                let anchor = selection.anchor;
                self.source_text = selection.text.clone();
                vec![
                    AppCommand::ShowPopup { anchor },
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
                vec![AppCommand::HidePopup]
            }
            AppEvent::SelectionCaptureFailed { task_id, error }
                if self.is_current_task(task_id) =>
            {
                self.phase = TranslationPhase::Error;
                self.error_message = error;
                self.current_translation_task = None;
                vec![AppCommand::ShowPopup { anchor: None }]
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
            AppEvent::MainWindowRequested => vec![AppCommand::ShowMainWindow],
            AppEvent::SettingsWindowRequested => {
                self.settings_error_message.clear();
                self.credential_error_message.clear();
                vec![AppCommand::ShowSettingsWindow]
            }
            AppEvent::SettingsSaveRequested { mut settings }
                if !self.settings_saving && !self.credential_busy =>
            {
                settings.deepl_credential_id = self.settings.deepl_credential_id.clone();
                self.settings_saving = true;
                self.settings_error_message.clear();
                vec![AppCommand::PersistSettings { settings }]
            }
            AppEvent::SettingsSaveRequested { .. } => Vec::new(),
            AppEvent::SettingsSaved { settings } => {
                self.settings = settings;
                self.settings_saving = false;
                self.settings_error_message.clear();
                vec![AppCommand::HideSettingsWindow]
            }
            AppEvent::SettingsSaveFailed { error } => {
                self.settings_saving = false;
                self.settings_error_message = error;
                Vec::new()
            }
            AppEvent::CredentialSaveRequested { secret }
                if !self.credential_busy && !self.settings_saving =>
            {
                let secret = secret.expose().trim().to_owned();
                if secret.is_empty() {
                    self.credential_error_message = "DeepL API key cannot be empty".into();
                    return Vec::new();
                }
                self.credential_busy = true;
                self.credential_error_message.clear();
                vec![AppCommand::PersistCredential {
                    credential_id: "deepl-primary".into(),
                    secret: crate::ports::credential::CredentialSecret::new(secret),
                }]
            }
            AppEvent::CredentialSaveRequested { .. } => Vec::new(),
            AppEvent::CredentialSaved { credential_id } => {
                self.settings.deepl_credential_id = Some(credential_id);
                self.credential_configured = true;
                self.credential_busy = false;
                self.credential_error_message.clear();
                vec![AppCommand::ClearCredentialDraft]
            }
            AppEvent::CredentialSaveFailed { error } => {
                self.credential_busy = false;
                self.credential_error_message = error;
                Vec::new()
            }
            AppEvent::CredentialRemoveRequested
                if !self.credential_busy && !self.settings_saving =>
            {
                let Some(credential_id) = self.settings.deepl_credential_id.clone() else {
                    self.credential_configured = false;
                    self.credential_error_message.clear();
                    return Vec::new();
                };
                self.credential_busy = true;
                self.credential_error_message.clear();
                vec![AppCommand::RemoveCredential { credential_id }]
            }
            AppEvent::CredentialRemoveRequested => Vec::new(),
            AppEvent::CredentialRemoved => {
                self.settings.deepl_credential_id = None;
                self.credential_configured = false;
                self.credential_busy = false;
                self.credential_error_message.clear();
                vec![AppCommand::ClearCredentialDraft]
            }
            AppEvent::CredentialRemoveFailed { error } => {
                self.credential_busy = false;
                self.credential_error_message = error;
                Vec::new()
            }
            AppEvent::CredentialAccessRequested {
                purpose,
                generation,
            } if !self.credential_busy && !self.settings_saving => {
                let Some(credential_id) = self.settings.deepl_credential_id.clone() else {
                    self.credential_configured = false;
                    self.credential_error_message = "DeepL API key is not configured".into();
                    return Vec::new();
                };
                self.credential_busy = true;
                self.credential_error_message.clear();
                vec![AppCommand::AccessCredential {
                    credential_id,
                    purpose,
                    generation,
                }]
            }
            AppEvent::CredentialAccessRequested { .. } => Vec::new(),
            AppEvent::CredentialAccessSucceeded { .. } => {
                self.credential_busy = false;
                self.credential_error_message.clear();
                Vec::new()
            }
            AppEvent::CredentialAccessFailed { error } => {
                self.credential_busy = false;
                self.credential_error_message = error;
                Vec::new()
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
                AppCommand::ShowPopup {
                    anchor: Some(Point { x: 10, y: 20 }),
                },
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
            ..Settings::default()
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
                AppCommand::ShowPopup { anchor: None },
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
            ..Settings::default()
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
            vec![AppCommand::ShowPopup { anchor: None }]
        );
        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.current_translation_task, None);
        assert_eq!(state.error_message, "No selected text was found");
    }

    #[test]
    fn empty_selection_hides_the_previous_popup() {
        let mut state = AppState::default();
        state.reduce(AppEvent::SelectionTranslationRequested);

        assert_eq!(
            state.reduce(AppEvent::SelectionCaptureEmpty {
                task_id: TranslationTaskId::new(1),
            }),
            vec![AppCommand::HidePopup]
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

    #[test]
    fn main_window_request_produces_show_command() {
        let mut state = AppState::default();
        assert_eq!(
            state.reduce(AppEvent::MainWindowRequested),
            vec![AppCommand::ShowMainWindow]
        );
    }

    #[test]
    fn settings_window_request_produces_show_command() {
        let mut state = AppState::default();
        assert_eq!(
            state.reduce(AppEvent::SettingsWindowRequested),
            vec![AppCommand::ShowSettingsWindow]
        );
    }

    #[test]
    fn settings_save_persists_before_committing() {
        let mut state = AppState::default();
        let requested = Settings {
            target_language: Language("ja".into()),
            ..Settings::default()
        };
        assert_eq!(
            state.reduce(AppEvent::SettingsSaveRequested {
                settings: requested.clone(),
            }),
            vec![AppCommand::PersistSettings {
                settings: requested.clone(),
            }]
        );
        assert!(state.settings_saving);
        assert_eq!(state.settings, Settings::default());

        assert_eq!(
            state.reduce(AppEvent::SettingsSaved {
                settings: requested.clone(),
            }),
            vec![AppCommand::HideSettingsWindow]
        );
        assert_eq!(state.settings, requested);
        assert!(!state.settings_saving);
    }

    #[test]
    fn settings_save_failure_preserves_committed_settings() {
        let committed = Settings {
            target_language: Language("de".into()),
            ..Settings::default()
        };
        let mut state = AppState::new(committed.clone());
        state.reduce(AppEvent::SettingsSaveRequested {
            settings: Settings {
                target_language: Language("fr".into()),
                ..Settings::default()
            },
        });

        assert!(
            state
                .reduce(AppEvent::SettingsSaveFailed {
                    error: "Could not save settings".into(),
                })
                .is_empty()
        );
        assert_eq!(state.settings, committed);
        assert!(!state.settings_saving);
        assert_eq!(state.settings_error_message, "Could not save settings");
        assert_eq!(state.phase, TranslationPhase::Idle);
    }

    #[test]
    fn credential_save_exposes_only_status_and_reference() {
        use crate::ports::credential::CredentialSecret;

        let mut state = AppState::default();
        let commands = state.reduce(AppEvent::CredentialSaveRequested {
            secret: CredentialSecret::new("private-key"),
        });
        assert_eq!(
            commands,
            vec![AppCommand::PersistCredential {
                credential_id: "deepl-primary".into(),
                secret: CredentialSecret::new("private-key"),
            }]
        );
        assert!(state.credential_busy);
        assert!(!state.credential_configured);
        assert!(!format!("{state:?}").contains("private-key"));

        assert_eq!(
            state.reduce(AppEvent::CredentialSaved {
                credential_id: "deepl-primary".into(),
            }),
            vec![AppCommand::ClearCredentialDraft]
        );
        assert!(state.credential_configured);
        assert!(!state.credential_busy);
        assert_eq!(
            state.settings.deepl_credential_id.as_deref(),
            Some("deepl-primary")
        );
    }

    #[test]
    fn credential_failures_preserve_committed_status() {
        use crate::ports::credential::CredentialSecret;

        let settings = Settings {
            deepl_credential_id: Some("deepl-primary".into()),
            ..Settings::default()
        };
        let mut state = AppState::with_credential_status(settings.clone(), true);
        state.reduce(AppEvent::CredentialSaveRequested {
            secret: CredentialSecret::new("replacement"),
        });
        state.reduce(AppEvent::CredentialSaveFailed {
            error: "save failed".into(),
        });
        assert_eq!(state.settings, settings);
        assert!(state.credential_configured);
        assert!(!state.credential_busy);
        assert_eq!(state.credential_error_message, "save failed");

        state.reduce(AppEvent::CredentialRemoveRequested);
        state.reduce(AppEvent::CredentialRemoveFailed {
            error: "remove failed".into(),
        });
        assert_eq!(state.settings, settings);
        assert!(state.credential_configured);
        assert!(!state.credential_busy);
        assert_eq!(state.credential_error_message, "remove failed");
    }

    #[test]
    fn credential_remove_clears_only_the_reference_after_success() {
        let mut state = AppState::with_credential_status(
            Settings {
                deepl_credential_id: Some("deepl-primary".into()),
                ..Settings::default()
            },
            true,
        );
        assert_eq!(
            state.reduce(AppEvent::CredentialRemoveRequested),
            vec![AppCommand::RemoveCredential {
                credential_id: "deepl-primary".into(),
            }]
        );
        assert_eq!(
            state.reduce(AppEvent::CredentialRemoved),
            vec![AppCommand::ClearCredentialDraft]
        );
        assert!(!state.credential_configured);
        assert_eq!(state.settings.deepl_credential_id, None);
    }

    #[test]
    fn credential_access_is_a_transient_command_and_never_enters_state() {
        use crate::ports::credential::CredentialAccessPurpose;

        let mut state = AppState::with_credential_status(
            Settings {
                deepl_credential_id: Some("deepl-primary".into()),
                ..Settings::default()
            },
            true,
        );

        assert_eq!(
            state.reduce(AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Reveal,
                generation: 42,
            }),
            vec![AppCommand::AccessCredential {
                credential_id: "deepl-primary".into(),
                purpose: CredentialAccessPurpose::Reveal,
                generation: 42,
            }]
        );
        assert!(state.credential_busy);
        assert!(!format!("{state:?}").contains("secret"));

        state.reduce(AppEvent::CredentialAccessSucceeded {
            purpose: CredentialAccessPurpose::Reveal,
        });
        assert!(!state.credential_busy);
    }

    #[test]
    fn credential_access_without_a_reference_fails_only_that_operation() {
        use crate::ports::credential::CredentialAccessPurpose;

        let mut state = AppState::default();
        assert!(
            state
                .reduce(AppEvent::CredentialAccessRequested {
                    purpose: CredentialAccessPurpose::Copy,
                    generation: 1,
                })
                .is_empty()
        );
        assert!(!state.credential_busy);
        assert_eq!(
            state.credential_error_message,
            "DeepL API key is not configured"
        );
    }
}
