mod annotation;
mod binding;
mod bridge;
mod mapper;
mod placement;
mod theme;

slint::include_modules!();

pub use bridge::{
    PassiveWindowPreparation, PopupPointerInput, PopupPointerSink, PopupResizeEdge, Ui, UiHandle,
    WindowLifecycleCallbacks,
};
