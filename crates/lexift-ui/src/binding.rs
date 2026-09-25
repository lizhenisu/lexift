use crate::{AppWindow, SettingsWindow, TranslationPopup, mapper::UiState};

pub(crate) fn apply_main(main: &AppWindow, state: &UiState) {
    main.set_status_text(state.status.clone().into());
    main.set_translation_busy(state.busy);
    main.set_target_language(state.target_language.clone().into());
    main.set_translated_text(state.translated.clone().into());
    main.set_error_text(state.error.clone().into());
}

pub(crate) fn apply_settings(settings: &SettingsWindow, state: &UiState) {
    let target_index = language_index(&state.target_language);
    if settings.get_draft_target_index() != target_index {
        settings.set_draft_target_index(target_index);
    }
    if settings.get_draft_hotkey_label().as_str() != state.hotkey {
        settings.set_draft_hotkey_label(state.hotkey.clone().into());
    }
    if settings.get_draft_provider_id().as_str() != state.provider {
        settings.set_draft_provider_id(state.provider.clone().into());
    }
    if settings.get_launch_at_login() != state.launch_at_login {
        settings.set_launch_at_login(state.launch_at_login);
    }
    if settings.get_selection_toolbar() != state.selection_toolbar {
        settings.set_selection_toolbar(state.selection_toolbar);
    }
    if settings.get_target_language_error_text().as_str() != state.target_language_error {
        settings.set_target_language_error_text(state.target_language_error.clone().into());
    }
    if settings.get_hotkey_error_text().as_str() != state.hotkey_error {
        settings.set_hotkey_error_text(state.hotkey_error.clone().into());
    }
    if settings.get_provider_error_text().as_str() != state.provider_error {
        settings.set_provider_error_text(state.provider_error.clone().into());
    }
    if settings.get_launch_at_login_error_text().as_str() != state.launch_at_login_error {
        settings.set_launch_at_login_error_text(state.launch_at_login_error.clone().into());
    }
    if settings.get_credential_configured() != state.credential_configured {
        settings.set_credential_configured(state.credential_configured);
    }
    if settings.get_credential_busy() != state.credential_busy {
        settings.set_credential_busy(state.credential_busy);
    }
    if settings.get_credential_request_pending() != state.credential_busy {
        settings.set_credential_request_pending(state.credential_busy);
    }
    if settings.get_credential_error_text().as_str() != state.credential_error {
        settings.set_credential_error_text(state.credential_error.clone().into());
    }
}

pub(crate) fn apply_popup(popup: &TranslationPopup, state: &crate::mapper::PopupUiState) {
    apply_popup_content(popup, state);
    popup.set_popup_width(420.0);
    popup.set_popup_height(state.height);
    popup.set_source_card_height(state.source_height);
    popup.set_resize_min_height(336.0);
}

pub(crate) fn apply_popup_content(popup: &TranslationPopup, state: &crate::mapper::PopupUiState) {
    popup.set_session_id(state.session_id as i32);
    popup.set_phase_text(state.status.clone().into());
    popup.set_source_text(state.source.clone().into());
    popup.set_translated_text(state.translated.clone().into());
    popup.set_error_text(state.error.clone().into());
    popup.set_source_index(state.source_index);
    popup.set_source_label(state.source_label.clone().into());
    popup.set_source_language(state.source_language.clone().into());
    popup.set_target_index(state.target_index);
    popup.set_target_label(state.target_label.clone().into());
    popup.set_detected_language(state.detected_language.clone().into());
    popup.set_busy(state.busy);
    popup.set_pinned(state.pinned);
    popup.set_speaking_source(state.speaking_source);
    popup.set_speaking_translation(state.speaking_translation);
    popup.set_feedback_text(state.feedback.clone().into());
    popup.set_feedback_error(state.feedback_error);
}

fn language_index(language: &str) -> i32 {
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
