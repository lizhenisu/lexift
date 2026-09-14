use std::sync::Arc;

use lexift_core::{
    Error,
    domain::translation::TranslateRequest,
    ports::translator::{TranslationFuture, TranslatorPort},
};

#[cfg(feature = "mock")]
use crate::mock::MockTranslator;

/// Translation providers selected by the application composition root.
pub struct ProviderRegistry {
    default: Option<Arc<dyn TranslatorPort>>,
    configured: bool,
}

impl ProviderRegistry {
    /// Creates a production registry without implicit development providers.
    pub fn new() -> Self {
        Self {
            default: Some(Arc::new(UnconfiguredTranslator)),
            configured: false,
        }
    }

    #[cfg(feature = "mock")]
    pub fn with_mock() -> Self {
        Self {
            default: Some(Arc::new(MockTranslator)),
            configured: true,
        }
    }

    pub fn default_translator(&self) -> Option<Arc<dyn TranslatorPort>> {
        self.default.as_ref().map(Arc::clone)
    }

    pub fn has_configured_provider(&self) -> bool {
        self.configured
    }
}

struct UnconfiguredTranslator;

impl TranslatorPort for UnconfiguredTranslator {
    fn translate(&self, _request: TranslateRequest) -> TranslationFuture<'_> {
        Box::pin(async {
            Err(Error::new(
                "no production translation provider is configured",
            ))
        })
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
        let registry = ProviderRegistry::new();
        assert!(registry.default_translator().is_some());
        assert!(!registry.has_configured_provider());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_provider_requires_explicit_construction() {
        let registry = ProviderRegistry::with_mock();
        assert!(registry.default_translator().is_some());
        assert!(registry.has_configured_provider());
    }
}
