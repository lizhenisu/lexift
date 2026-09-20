use lexift_core::{AppState, TranslationPhase};

pub(crate) struct UiState {
    pub status: String,
    pub busy: bool,
    pub target_language: String,
    pub source: String,
    pub translated: String,
    pub error: String,
    pub hotkey: String,
    pub provider: String,
    pub target_language_error: String,
    pub hotkey_error: String,
    pub provider_error: String,
    pub credential_configured: bool,
    pub credential_busy: bool,
    pub credential_error: String,
}

pub(crate) fn view_state(state: &AppState) -> UiState {
    UiState {
        status: match state.phase {
            TranslationPhase::Idle => "Idle",
            TranslationPhase::NoSelection => "No selection",
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
        target_language: state.desired_settings.target_language.0.clone(),
        source: state.source_text.clone(),
        translated: state.translated_text.clone(),
        error: state.error_message.clone(),
        hotkey: state.desired_settings.hotkey.to_string(),
        provider: state.desired_settings.provider.id().into(),
        target_language_error: state.target_language_settings_error.clone(),
        hotkey_error: state.hotkey_settings_error.clone(),
        provider_error: state.provider_settings_error.clone(),
        credential_configured: state.credential_configured,
        credential_busy: state.credential_busy,
        credential_error: state.credential_error_message.clone(),
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
            ..Settings::default()
        });

        assert_eq!(view_state(&state).target_language, "ja");
    }

    #[test]
    fn maps_translation_phases_and_content() {
        let cases = [
            (TranslationPhase::Idle, "Idle", false),
            (TranslationPhase::NoSelection, "No selection", false),
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

    #[test]
    fn maps_desired_settings_and_field_errors_separately() {
        let mut state = AppState::default();
        state.desired_settings.hotkey = "Ctrl+Shift+Y".parse().unwrap();
        state.hotkey_settings_error = "save failed".into();
        state.provider_settings_error = "runtime failed".into();
        state.credential_configured = true;
        state.credential_busy = true;
        state.credential_error_message = "credential failed".into();
        let mapped = view_state(&state);
        assert_eq!(mapped.hotkey, "Ctrl + Shift + Y");
        assert_eq!(mapped.hotkey_error, "save failed");
        assert_eq!(mapped.provider_error, "runtime failed");
        assert!(mapped.credential_configured);
        assert!(mapped.credential_busy);
        assert_eq!(mapped.credential_error, "credential failed");
        assert!(mapped.error.is_empty());
    }
}
