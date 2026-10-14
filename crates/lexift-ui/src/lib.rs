mod frozen_capture;
pub use frozen_capture::FrozenCapture;
mod pinned_screenshot;
pub use annotation_export_render::SoftwareExportRenderer;
mod about;
mod ocr;
mod screen_interaction;
pub use about::AboutLink;
mod annotation;
mod binding;
mod bridge;
mod i18n;
mod llm_settings;
mod mapper;
pub use llm_settings::{LlmQueryCancellation, LlmQueryCompletion};
mod placement;
#[cfg(test)]
mod settings_theme_tests;
mod theme;
#[cfg(test)]
mod translation_layout_tests;
mod tray_menu;

slint::include_modules!();

pub use bridge::{
    PassiveWindowPreparation, PopupDismissCause, PopupPointerInput, PopupPointerSink,
    PopupResizeEdge, Ui, UiHandle, WindowLifecycleCallbacks, WindowRole,
};

#[cfg(test)]
mod settings_hint_tests;

#[cfg(test)]
mod ocr_tests;

// Crate-private render aliases keep view adapters independent of the session coordinator.
pub(crate) use annotation::export_render as annotation_export_render;
pub(crate) use annotation::fingerprint as annotation_fingerprint;
pub(crate) use annotation::path as annotation_path;
pub(crate) use annotation::render as annotation_render;
pub(crate) use annotation::scene as annotation_scene;
pub(crate) use annotation::sequence as annotation_sequence;
pub(crate) use annotation::signature as annotation_signature;
pub(crate) use annotation::sticker as annotation_sticker;
pub(crate) use annotation::text as annotation_text;

#[cfg(debug_assertions)]
#[doc(hidden)]
pub type NativePinProbe = std::rc::Rc<dyn Fn(&slint::Window, &str, bool) -> bool>;
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn start_native_pin_repro(probe: NativePinProbe) -> std::rc::Rc<std::cell::Cell<bool>> {
    annotation::native_repro::start(probe)
}
