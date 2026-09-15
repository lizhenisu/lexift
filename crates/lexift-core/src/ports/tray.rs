use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenMainWindow,
    OpenSettings,
    Quit,
}

pub type TrayHandler = Arc<dyn Fn(TrayAction) + Send + Sync + 'static>;

/// Publishes native tray actions as application-level intents.
pub trait TrayPort: Send + Sync {
    fn register(&self, handler: TrayHandler) -> Result<()>;
}
