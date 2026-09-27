mod annotation;
mod annotation_render;
mod binding;
mod bridge;
mod i18n;
mod mapper;
mod placement;
mod theme;
mod tray_menu;

slint::include_modules!();

pub use bridge::{
    PassiveWindowPreparation, PopupPointerInput, PopupPointerSink, PopupResizeEdge, Ui, UiHandle,
    WindowLifecycleCallbacks,
};
