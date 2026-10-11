//! Independent OCR selection host and Provider settings adapters.
pub(crate) mod provider;
mod selection;
pub(crate) use selection::{active, close, init, request, screen_busy, set_handler, show, sync};
