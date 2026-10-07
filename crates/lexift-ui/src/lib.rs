mod annotation;
mod annotation_fingerprint;
mod annotation_path;
mod annotation_render;
mod annotation_scene;
mod annotation_sequence;
mod annotation_signature;
mod annotation_sticker;
mod annotation_text;
mod binding;
mod bridge;
mod i18n;
mod llm_settings;
mod mapper;
pub use llm_settings::{LlmQueryCancellation, LlmQueryCompletion};
mod placement;
mod theme;
#[cfg(test)]
mod translation_layout_tests;
mod tray_menu;

slint::include_modules!();

pub use bridge::{
    PassiveWindowPreparation, PopupDismissCause, PopupPointerInput, PopupPointerSink,
    PopupResizeEdge, Ui, UiHandle, WindowLifecycleCallbacks,
};
