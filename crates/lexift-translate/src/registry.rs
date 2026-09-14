use std::sync::Arc;

use lexift_core::{
    Error,
    domain::translation::TranslateRequest,
    ports::translator::{TranslationFuture, TranslatorPort},
};

use crate::http;
#[cfg(feature = "mock")]
use crate::mock::MockTranslator;
use crate::providers::deepl::DeepLApiTranslator;

/// Translation providers selected by the application composition root.
pub struct ProviderRegistry {
    default: Arc<dyn TranslatorPort>,
    configured: bool,
}

impl ProviderRegistry {
    /// Creates a production registry without implicit development providers.
    pub fn new() -> Self {
        Self {
            default: Arc::new(UnconfiguredTranslator),
            configured: false,
        }
    }

    /// Creates a production registry backed by the official DeepL API.
    pub fn with_deepl_api(auth_key: String) -> lexift_core::Result<Self> {
        let client = http::build_client()?;
        Ok(Self {
            default: Arc::new(DeepLApiTranslator::new(client, auth_key)),
            configured: true,
        })
    }

    #[cfg(feature = "mock")]
    pub fn with_mock() -> Self {
        Self {
            default: Arc::new(MockTranslator),
            configured: true,
        }
    }

    pub fn default_translator(&self) -> Arc<dyn TranslatorPort> {
        Arc::clone(&self.default)
    }

    pub fn has_configured_provider(&self) -> bool {
        self.configured
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

struct UnconfiguredTranslator;

impl TranslatorPort for UnconfiguredTranslator {
    fn translate(&self, _request: TranslateRequest) -> TranslationFuture<'_> {
        Box::pin(async { Err(Error::new("Translation provider is not configured")) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_registry_does_not_install_mock_provider() {
        let registry = ProviderRegistry::new();
        assert!(!registry.has_configured_provider());
    }

    #[test]
    fn unconfigured_registry_defers_failure_until_translation() {
        let registry = ProviderRegistry::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize");
        let error = runtime
            .block_on(registry.default_translator().translate(TranslateRequest {
                text: "Hello world".into(),
                target_language: lexift_core::domain::language::Language("zh-CN".into()),
            }))
            .expect_err("translation should fail when no provider is configured");

        assert_eq!(error.to_string(), "Translation provider is not configured");
    }

    #[test]
    fn deepl_registry_is_configured() {
        let registry = ProviderRegistry::with_deepl_api("some-key:fx".into())
            .expect("shared HTTP client should initialize");
        assert!(registry.has_configured_provider());
    }

    #[cfg(feature = "mock")]
    #[test]
    fn mock_provider_requires_explicit_construction() {
        let registry = ProviderRegistry::with_mock();
        assert!(registry.has_configured_provider());
    }
}
