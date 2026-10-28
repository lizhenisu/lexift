//! Read-only home presentation. Confirmed native registrations never borrow Settings drafts.
use lexift_core::{
    AppState,
    domain::{runtime_config::HotkeyConfig, settings::SettingsField},
};
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub label: String,
    pub status: String,
    pub unavailable: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct View {
    pub translation: Shortcut,
    pub annotation: Shortcut,
    pub ocr: Shortcut,
}
fn shortcut(active: Option<HotkeyConfig>, configured: HotkeyConfig, failed: bool) -> Shortcut {
    let unavailable = active.is_none();
    let status = if unavailable {
        crate::i18n::tr("Shortcut unavailable. Check Settings.")
    } else if failed {
        crate::i18n::tr("Shortcut change failed. The previous shortcut is still active.")
    } else {
        String::new()
    };
    Shortcut {
        label: active.unwrap_or(configured).to_string(),
        status,
        unavailable,
    }
}
pub(crate) fn view(state: &AppState) -> View {
    let failed = |field| state.settings_error_field == Some(field);
    let mut ocr = shortcut(
        state.registered_hotkeys.ocr,
        state.runtime_config.ocr_hotkey,
        failed(SettingsField::OcrHotkey),
    );
    if !state.settings.ocr_enabled {
        ocr.unavailable = true;
        ocr.status = crate::i18n::tr("OCR is disabled. Enable it in Settings.");
    }
    View {
        translation: shortcut(
            state.registered_hotkeys.translation,
            state.runtime_config.hotkey,
            failed(SettingsField::Hotkey),
        ),
        annotation: shortcut(
            state.registered_hotkeys.annotation,
            state.runtime_config.annotation_hotkey,
            failed(SettingsField::AnnotationHotkey),
        ),
        ocr,
    }
}
pub(crate) fn apply(main: &crate::AppWindow, state: &AppState) {
    let view = view(state);
    main.set_translation_shortcut(view.translation.label.into());
    main.set_translation_shortcut_status(view.translation.status.into());
    main.set_translation_shortcut_unavailable(view.translation.unavailable);
    main.set_annotation_shortcut(view.annotation.label.into());
    main.set_annotation_shortcut_status(view.annotation.status.into());
    main.set_annotation_shortcut_unavailable(view.annotation.unavailable);
    main.set_ocr_shortcut(view.ocr.label.into());
    main.set_ocr_shortcut_status(view.ocr.status.into());
    main.set_ocr_shortcut_unavailable(view.ocr.unavailable);
}

#[cfg(test)]
#[path = "usage_guide_tests.rs"]
mod tests;
