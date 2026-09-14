use crate::{AppWindow, TranslationPopup, mapper::UiState};

pub(crate) fn apply(main: &AppWindow, popup: &TranslationPopup, state: UiState) {
    main.set_status_text(state.status.clone().into());
    main.set_translation_busy(state.busy);
    main.set_target_language(state.target_language.into());
    main.set_translated_text(state.translated.clone().into());
    main.set_error_text(state.error.clone().into());
    popup.set_phase_text(state.status.into());
    popup.set_source_text(state.source.into());
    popup.set_translated_text(state.translated.into());
    popup.set_error_text(state.error.into());
}
