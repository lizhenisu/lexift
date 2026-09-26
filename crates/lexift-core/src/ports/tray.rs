use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenMainWindow,
    OpenSettings,
    Quit,
}

/// Localized labels supplied by the presentation layer, owned by the native adapter.
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

    fn set_menu_labels(&self, _labels: TrayMenuLabels) {}

    /// Updates the preference used the next time the native context menu opens.
    fn set_theme(&self, _theme: crate::domain::settings::ThemePreference) {}
}
