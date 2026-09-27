use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenMainWindow,
    OpenSettings,
    Quit,
}

/// Where and how the notification-area menu was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayMenuRequest {
    pub anchor: crate::domain::geometry::Point,
    pub keyboard: bool,
}
pub type TrayMenuHandler = Arc<dyn Fn(TrayMenuRequest) + Send + Sync + 'static>;

/// Localized menu labels supplied by the presentation layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuLabels {
    pub open: String,
    pub settings: String,
    pub quit: String,
}
impl Default for TrayMenuLabels {
    fn default() -> Self {
        Self {
            open: "Open Lexift".into(),
            settings: "Settings".into(),
            quit: "Quit".into(),
        }
    }
}

pub type TrayHandler = Arc<dyn Fn(TrayAction) + Send + Sync + 'static>;

/// Publishes native tray actions as application-level intents.
pub trait TrayPort: Send + Sync {
    fn register(&self, handler: TrayHandler) -> Result<()>;

    fn set_menu_handler(&self, _handler: TrayMenuHandler) {}

    /// Returns keyboard focus to the notification area after explicit cancellation.
    /// Do not call when another window has already gained focus.
    fn menu_cancelled(&self) {}
}
