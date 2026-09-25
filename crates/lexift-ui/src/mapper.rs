use lexift_core::{AppState, TranslationPhase, state::PopupSessionState};

pub(crate) struct UiState {
    pub status: String,
    pub busy: bool,
    pub target_language: String,
    pub translated: String,
    pub error: String,
    pub hotkey: String,
    pub provider: String,
    pub launch_at_login: bool,
    pub selection_toolbar: bool,
    pub target_language_error: String,
    pub hotkey_error: String,
    pub provider_error: String,
    pub launch_at_login_error: String,
    pub credential_configured: bool,
    pub credential_busy: bool,
    pub credential_error: String,
}

#[derive(Clone)]
pub(crate) struct PopupUiState {
    pub session_id: u64,
    pub status: String,
    pub busy: bool,
    pub source: String,
    pub translated: String,
    pub error: String,
    pub source_index: i32,
    pub source_label: String,
    pub source_language: String,
    pub target_index: i32,
    pub target_label: String,
    pub detected_language: String,
    pub pinned: bool,
    pub speaking_source: bool,
    pub speaking_translation: bool,
    pub feedback: String,
    pub feedback_error: bool,
    pub height: f32,
    pub source_height: f32,
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
        translated: state.translated_text.clone(),
        error: state.error_message.clone(),
        hotkey: state.desired_settings.hotkey.to_string(),
        provider: state.desired_settings.provider.id().into(),
        launch_at_login: state.desired_settings.launch_at_login,
        selection_toolbar: state.desired_settings.selection_toolbar,
        target_language_error: state.target_language_settings_error.clone(),
        hotkey_error: state.hotkey_settings_error.clone(),
        provider_error: state.provider_settings_error.clone(),
        launch_at_login_error: state.launch_at_login_settings_error.clone(),
        credential_configured: state.credential_configured,
        credential_busy: state.credential_busy,
        credential_error: state.credential_error_message.clone(),
    }
}

pub(crate) fn popup_states(state: &AppState) -> Vec<PopupUiState> {
    state.popup_sessions.iter().map(popup_state).collect()
}

fn popup_state(session: &PopupSessionState) -> PopupUiState {
    PopupUiState {
        session_id: session.id.value(),
        status: phase_label(session.phase).into(),
        busy: matches!(
            session.phase,
            TranslationPhase::Capturing | TranslationPhase::Translating
        ),
        source: session.source_text.clone(),
        translated: session.translated_text.clone(),
        error: session.error_message.clone(),
        source_index: source_language_index(session.source_language.as_ref()),
        source_label: session
            .source_language
            .as_ref()
            .map(|language| source_language_label(&language.0))
            .unwrap_or("Auto detect")
            .into(),
        source_language: session
            .source_language
            .as_ref()
            .map(|language| language.0.clone())
            .unwrap_or_default(),
        target_index: language_index(&session.target_language.0),
        target_label: language_label(&session.target_language.0).into(),
        detected_language: session
            .detected_source_language
            .as_ref()
            .map(|language| language.0.clone())
            .unwrap_or_default(),
        pinned: session.pinned,
        speaking_source: session.speaking_source,
        speaking_translation: session.speaking_translation,
        feedback: session.feedback_message.clone(),
        feedback_error: session.feedback_error,
        height: popup_height(
            &session.source_text,
            &session.translated_text,
            &session.error_message,
        ),
        source_height: source_height(&session.source_text),
    }
}

pub(crate) fn source_language_index(
    language: Option<&lexift_core::domain::language::Language>,
) -> i32 {
    match language.map(|language| language.0.as_str()) {
        None => 0,
        Some("en-US") => 1,
        Some("zh-CN") => 2,
        Some("ja") => 3,
        Some("ko") => 4,
        Some("de") => 5,
        Some("fr") => 6,
        Some("es") => 7,
        Some("it") => 8,
        Some("pt-PT") => 9,
        Some(_) => 0,
    }
}

fn source_language_label(language: &str) -> &'static str {
    match language {
        "en-US" => "English",
        "zh-CN" => "中文",
        "ja" => "日本語",
        "ko" => "한국어",
        "de" => "Deutsch",
        "fr" => "Français",
        "es" => "Español",
        "it" => "Italiano",
        "pt-PT" => "Português",
        _ => "Auto detect",
    }
}

fn phase_label(phase: TranslationPhase) -> &'static str {
    match phase {
        TranslationPhase::Idle => "Idle",
        TranslationPhase::NoSelection => "No selection",
        TranslationPhase::Capturing => "Capturing selection…",
        TranslationPhase::Translating => "Translating…",
        TranslationPhase::Success => "Translation complete",
        TranslationPhase::Error => "Translation failed",
    }
}

pub(crate) fn language_index(language: &str) -> i32 {
    match language {
        "zh-CN" => 0,
        "zh-TW" => 1,
        "en-US" => 2,
        "en-GB" => 3,
        "ja" => 4,
        "ko" => 5,
        "de" => 6,
        "fr" => 7,
        "es" => 8,
        "it" => 9,
        "pt-PT" => 10,
        "pt-BR" => 11,
        _ => 0,
    }
}

pub(crate) fn language_label(language: &str) -> &'static str {
    match language {
        "zh-CN" => "简体中文",
        "zh-TW" => "繁體中文",
        "en-US" => "English (US)",
        "en-GB" => "English (UK)",
        "ja" => "日本語",
        "ko" => "한국어",
        "de" => "Deutsch",
        "fr" => "Français",
        "es" => "Español",
        "it" => "Italiano",
        "pt-PT" => "Português (Portugal)",
        "pt-BR" => "Português (Brasil)",
        _ => "Detected",
    }
}

fn popup_height(source: &str, translated: &str, error: &str) -> f32 {
    popup_metrics_for_width(source, translated, error, 420.0).0
}

fn source_height(source: &str) -> f32 {
    popup_metrics_for_width(source, "", "", 420.0).1
}

pub(crate) fn popup_metrics_for_width(
    source: &str,
    translated: &str,
    error: &str,
    popup_width: f32,
) -> (f32, f32) {
    let chars_per_line = ((popup_width.max(340.0) / 420.0 * 44.0).round() as usize).clamp(28, 96);
    let source_lines = (source.chars().count().div_ceil(chars_per_line)).clamp(1, 5);
    let result_chars = if error.is_empty() {
        translated.chars().count()
    } else {
        error.chars().count()
    };
    let result_lines = result_chars.div_ceil(chars_per_line).clamp(1, 10);
    let popup_height = (288 + source_lines * 18 + result_lines * 20).clamp(336, 556) as f32;
    let source_height = (86 + source_lines.clamp(1, 4) * 14).min(140) as f32;
    (popup_height, source_height)
}

#[cfg(test)]
mod tests {
    use lexift_core::domain::{language::Language, settings::Settings};

    use super::*;

    #[test]
    fn narrower_manual_popup_uses_more_wrapped_lines_for_auto_growth() {
        let source = "a".repeat(180);
        let translated = "b".repeat(300);
        let wide = popup_metrics_for_width(&source, &translated, "", 640.0);
        let narrow = popup_metrics_for_width(&source, &translated, "", 340.0);

        assert!(narrow.0 >= wide.0);
        assert!(narrow.1 >= wide.1);
    }

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
