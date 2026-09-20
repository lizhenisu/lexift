use crate::{AppWindow, SettingsWindow, TranslationPopup, mapper::UiState};

pub(crate) fn apply(
    main: &AppWindow,
    popup: &TranslationPopup,
    settings: &SettingsWindow,
    state: UiState,
) {
    main.set_status_text(state.status.clone().into());
    main.set_translation_busy(state.busy);
    main.set_target_language(state.target_language.clone().into());
    main.set_translated_text(state.translated.clone().into());
    main.set_error_text(state.error.clone().into());
    popup.set_phase_text(state.status.into());
    popup.set_source_text(state.source.into());
    popup.set_translated_text(state.translated.into());
    popup.set_error_text(state.error.into());
    let target_index = language_index(&state.target_language);
    if settings.get_draft_target_index() != target_index {
        settings.set_draft_target_index(target_index);
    }
    if settings.get_draft_hotkey_label().as_str() != state.hotkey {
        settings.set_draft_hotkey_label(state.hotkey.into());
    }
    if settings.get_draft_provider_id().as_str() != state.provider {
        settings.set_draft_provider_id(state.provider.into());
    }
    if settings.get_target_language_error_text().as_str() != state.target_language_error {
        settings.set_target_language_error_text(state.target_language_error.into());
    }
    if settings.get_hotkey_error_text().as_str() != state.hotkey_error {
        settings.set_hotkey_error_text(state.hotkey_error.into());
    }
    if settings.get_provider_error_text().as_str() != state.provider_error {
        settings.set_provider_error_text(state.provider_error.into());
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
        settings.set_credential_error_text(state.credential_error.into());
    }
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
