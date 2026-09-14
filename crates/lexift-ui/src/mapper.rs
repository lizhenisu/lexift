use lexift_core::{AppState, TranslationPhase};

pub(crate) struct UiState {
    pub status: String,
    pub busy: bool,
    pub target_language: String,
    pub source: String,
    pub translated: String,
    pub error: String,
}

pub(crate) fn view_state(state: &AppState) -> UiState {
    UiState {
        status: match state.phase {
            TranslationPhase::Idle => "Idle",
            TranslationPhase::Capturing => "Capturing selection…",
            TranslationPhase::Translating => "Translating…",
            TranslationPhase::Success => "Translation complete",
            TranslationPhase::Error => "Translation failed",
        }
        .into(),
        busy: matches!(
            state.phase,
            TranslationPhase::Capturing | TranslationPhase::Translating
        ),
        target_language: state.settings.target_language.0.clone(),
        source: state.source_text.clone(),
        translated: state.translated_text.clone(),
        error: state.error_message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use lexift_core::domain::{language::Language, settings::Settings};

    use super::*;

    #[test]
    fn maps_target_language_from_core_settings() {
        let state = AppState::new(Settings {
            target_language: Language("ja".into()),
        });

        assert_eq!(view_state(&state).target_language, "ja");
    }

    #[test]
    fn maps_translation_phases_and_content() {
        let cases = [
            (TranslationPhase::Idle, "Idle", false),
            (TranslationPhase::Capturing, "Capturing selection…", true),
            (TranslationPhase::Translating, "Translating…", true),
            (TranslationPhase::Success, "Translation complete", false),
            (TranslationPhase::Error, "Translation failed", false),
        ];

        for (phase, expected_status, expected_busy) in cases {
            let mut state = AppState::default();
            state.phase = phase;
            state.translated_text = "translated".into();
            state.error_message = "error".into();

            let mapped = view_state(&state);
            assert_eq!(mapped.status, expected_status);
            assert_eq!(mapped.busy, expected_busy);
            assert_eq!(mapped.translated, "translated");
            assert_eq!(mapped.error, "error");
        }
    }
}
