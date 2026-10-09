use crate::{AppWindow, SettingsWindow, TranslationPopup, mapper::UiState};
use slint::ComponentHandle;

pub(crate) fn apply_main(main: &AppWindow, state: &UiState) {
    main.set_status_text(state.status.clone().into());
    main.set_translation_busy(state.busy);
    main.set_target_language(state.target_language.clone().into());
    main.set_translated_text(state.translated.clone().into());
    main.set_error_text(state.error.clone().into());
}

pub(crate) fn apply_settings(settings: &SettingsWindow, state: &UiState) {
    settings.set_deepseek_environment_key(state.deepseek_environment_key);
    crate::llm_settings::sync(settings, &state.llm);
    crate::llm_settings::secure_completed(
        settings,
        state.credential_busy,
        state.credential_error.is_empty(),
    );
    crate::llm_settings::refresh_localized(settings);
    settings.set_appearance_language_index(state.ui_language.index());
    settings.set_annotation_hotkey_status(crate::annotation::status().into());
    settings.set_appearance_theme_index(crate::theme::index(state.theme));
    let target_index = language_index(&state.target_language);
    if settings.get_draft_target_index() != target_index {
        settings.set_draft_target_index(target_index);
    }
    if settings.get_draft_hotkey_label().as_str() != state.hotkey {
        settings.set_draft_hotkey_label(state.hotkey.clone().into());
    }
    if settings.get_draft_annotation_hotkey_label().as_str() != state.annotation_hotkey {
        settings.set_draft_annotation_hotkey_label(state.annotation_hotkey.clone().into());
    }
    if settings.get_draft_provider_id().as_str() != state.provider {
        settings.set_draft_provider_id(state.provider.clone().into());
        crate::llm_settings::sync_provider(settings, &state.provider);
    }
    if settings.get_launch_at_login() != state.launch_at_login {
        settings.set_launch_at_login(state.launch_at_login);
    }
    settings.set_history_error(state.history_error.clone().into());
    settings.set_history_enabled(state.history_enabled);
    settings.set_history_limit(state.history_limit);
    if settings.get_selection_toolbar() != state.selection_toolbar {
        settings.set_selection_toolbar(state.selection_toolbar);
    }
    if settings.get_target_language_error_text().as_str() != state.target_language_error {
        settings.set_target_language_error_text(state.target_language_error.clone().into());
    }
    if settings.get_hotkey_error_text().as_str() != state.hotkey_error {
        settings.set_hotkey_error_text(state.hotkey_error.clone().into());
    }
    if settings.get_annotation_hotkey_error_text().as_str() != state.annotation_hotkey_error {
        settings.set_annotation_hotkey_error_text(state.annotation_hotkey_error.clone().into());
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

pub(crate) fn apply_popup_content(popup: &TranslationPopup, state: &crate::mapper::PopupUiState) {
    for (index, copy) in state.copy.iter().enumerate() {
        let source = index == 0;
        let completed = if copy.completed == 0 {
            String::new()
        } else {
            copy.completed.to_string()
        };
        let previous = if source {
            popup.get_copy_source_completed()
        } else {
            popup.get_copy_result_completed()
        };
        if source {
            popup.set_copy_source_pending(copy.pending);
            popup.set_copy_source_completed(completed.clone().into());
        } else {
            popup.set_copy_result_pending(copy.pending);
            popup.set_copy_result_completed(completed.clone().into());
        }
        if !completed.is_empty() && previous.as_str() != completed {
            let weak = popup.as_weak();
            let session = state.session_id;
            slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
                if let Some(w) = weak
                    .upgrade()
                    .filter(|w| w.get_session_id() as u64 == session)
                {
                    w.invoke_copy_feedback_expired(source, completed.into());
                }
            });
        }
    }
    popup.set_session_id(state.session_id as i32);
    popup.set_phase_text(state.status.clone().into());
    // Copy feedback updates must not replace an unsubmitted source editor draft.
    if popup.get_mapped_source_text().as_str() != state.source {
        popup.set_source_text(state.source.clone().into());
        popup.set_mapped_source_text(state.source.clone().into());
    }
    popup.set_copy_source_value(state.copy_text[0].clone().into());
    popup.set_copy_result_value(state.copy_text[1].clone().into());
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
