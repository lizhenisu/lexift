use std::sync::Arc;

use lexift_core::ports::selection::SelectionPort;

use crate::mock::MockSelectionPort;

pub struct PlatformCapabilities {
    selection: Arc<dyn SelectionPort>,
}

impl PlatformCapabilities {
    /// Provides deterministic adapters for the M1 vertical slice.
    pub fn new() -> Self {
        Self {
            selection: Arc::new(MockSelectionPort),
        }
    }

    pub fn selection(&self) -> Arc<dyn SelectionPort> {
        Arc::clone(&self.selection)
    }
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self::new()
    }
}
