use std::sync::Arc;

use lexift_core::{
    Error,
    domain::translation::TranslateRequest,
    ports::translator::{TranslationFuture, TranslatorPort},
};

use crate::http;
#[cfg(feature = "mock")]
use crate::mock::MockTranslator;

/// Translation providers selected by the application composition root.
pub struct ProviderRegistry {
    default: Option<Arc<dyn TranslatorPort>>,
    configured: bool,
    _http_client: Option<reqwest::Client>,
}

impl ProviderRegistry {
    /// Creates a production registry without implicit development providers.
    pub fn new() -> lexift_core::Result<Self> {
        Ok(Self {
            default: Some(Arc::new(UnconfiguredTranslator)),
            configured: false,
            _http_client: Some(http::build_client()?),
        })
    }

    #[cfg(feature = "mock")]
    pub fn with_mock() -> Self {
        Self {
            default: Some(Arc::new(MockTranslator)),
            configured: true,
            _http_client: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_registry_does_not_install_mock_provider() {
        let registry = ProviderRegistry::new().expect("production registry should initialize");
        assert!(registry.default_translator().is_some());
        assert!(!registry.has_configured_provider());
        assert!(registry._http_client.is_some());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_provider_requires_explicit_construction() {
        let registry = ProviderRegistry::with_mock();
        assert!(registry.default_translator().is_some());
        assert!(registry.has_configured_provider());
        assert!(registry._http_client.is_none());
    }
}
