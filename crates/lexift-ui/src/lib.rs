mod binding;
mod bridge;
mod mapper;
mod placement;

slint::include_modules!();

pub use bridge::{Ui, UiHandle};
