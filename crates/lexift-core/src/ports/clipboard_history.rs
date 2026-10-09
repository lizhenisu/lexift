//! Operating-system capabilities for history capture, encrypted storage and guarded paste.
use crate::{
    Result,
    domain::{
        clipboard_history::{HistoryEntry, PasteTarget},
        selection::SelectionTarget,
    },
};
use std::sync::{Arc, atomic::AtomicBool};
pub type HistoryHandler = Arc<dyn Fn(Result<Option<HistoryEntry>>) + Send + Sync>;
pub trait ClipboardHistoryPort: Send + Sync {
    fn set_recording(&self, enabled: bool, handler: HistoryHandler) -> Result<()>;
    fn stop(&self);
}
pub trait ClipboardHistoryStore: Send + Sync {
    fn load(&self) -> Result<Vec<HistoryEntry>>;
    fn save(&self, entries: &[HistoryEntry]) -> Result<()>;
}
pub trait ClipboardPastePort: Send + Sync {
    fn capture_target(&self, target: SelectionTarget) -> Result<PasteTarget>;
    fn paste(&self, target: &PasteTarget, text: &str, cancelled: &AtomicBool) -> Result<()>;
}

pub type HistoryCapabilities = (
    Arc<dyn ClipboardHistoryPort>,
    Arc<dyn ClipboardHistoryStore>,
    Arc<dyn ClipboardPastePort>,
);
