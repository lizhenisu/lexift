//! Clipboard history capabilities, with an HWND confined to its notification thread.
mod monitor;
mod paste;
mod store;
mod transient;
use lexift_core::{Error, Result};
use std::sync::Arc;
pub(crate) use transient::Transient;
pub(crate) fn capabilities() -> Result<lexift_core::ports::clipboard_history::HistoryCapabilities> {
    let directory = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| Error::new("Clipboard history storage directory is unavailable"))?;
    Ok((
        Arc::new(monitor::Monitor::default()),
        Arc::new(store::Store {
            path: std::path::PathBuf::from(directory)
                .join("Lexift")
                .join("clipboard-history.dat"),
        }),
        Arc::new(paste::Paste),
    ))
}
