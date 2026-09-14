use std::sync::Arc;

use lexift_core::ports::translator::TranslatorPort;

#[cfg(feature = "mock")]
use crate::mock::MockTranslator;

/// Translation providers selected by the application composition root.
pub struct ProviderRegistry {
    default: Option<Arc<dyn TranslatorPort>>,
}

impl ProviderRegistry {
    /// Creates a production registry without implicit development providers.
    pub fn new() -> Self {
        Self { default: None }
    }

    #[cfg(feature = "mock")]
    pub fn with_mock() -> Self {
        Self {
            default: Some(Arc::new(MockTranslator)),
        }
    }

    pub fn default_translator(&self) -> Option<Arc<dyn TranslatorPort>> {
        self.default.as_ref().map(Arc::clone)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_registry_does_not_install_mock_provider() {
        assert!(ProviderRegistry::new().default_translator().is_none());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_provider_requires_explicit_construction() {
        assert!(ProviderRegistry::with_mock().default_translator().is_some());
    }
}
