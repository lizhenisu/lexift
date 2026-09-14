use std::sync::Arc;

use lexift_core::ports::translator::TranslatorPort;

use crate::mock::MockTranslator;

pub struct ProviderRegistry {
    default: Arc<dyn TranslatorPort>,
}

impl ProviderRegistry {
    /// Registers the deterministic translator used by the M1 vertical slice.
    pub fn new() -> Self {
        Self {
            default: Arc::new(MockTranslator),
        }
    }

    pub fn default_translator(&self) -> Arc<dyn TranslatorPort> {
        Arc::clone(&self.default)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
