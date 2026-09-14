mod binding;
mod bridge;
mod mapper;

slint::include_modules!();

pub use bridge::{Ui, UiHandle};
