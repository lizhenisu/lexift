use std::sync::Arc;

use lexift_core::ports::selection::SelectionPort;

#[cfg(feature = "mock")]
use crate::mock::MockSelectionPort;

/// Platform adapters selected by the application composition root.
pub struct PlatformCapabilities {
    selection: Option<Arc<dyn SelectionPort>>,
}

impl PlatformCapabilities {
    /// Creates a production capability set without implicit development adapters.
    pub fn new() -> Self {
        Self { selection: None }
    }

    #[cfg(feature = "mock")]
    pub fn mock() -> Self {
        Self {
            selection: Some(Arc::new(MockSelectionPort)),
        }
    }

    pub fn selection(&self) -> Option<Arc<dyn SelectionPort>> {
        self.selection.as_ref().map(Arc::clone)
    }
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_capabilities_do_not_install_mock_selection() {
        assert!(PlatformCapabilities::new().selection().is_none());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_selection_requires_explicit_construction() {
        assert!(PlatformCapabilities::mock().selection().is_some());
    }
}
