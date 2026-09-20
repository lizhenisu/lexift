use std::sync::{Arc, RwLock};

#[cfg(not(feature = "m1-demo"))]
use lexift_core::{
    Error,
    ports::credential::{CredentialError, CredentialErrorKind, CredentialStore},
};
use lexift_core::{
    Result,
    domain::{runtime_config::ProviderConfig, translation::TranslateRequest},
    ports::translator::{TranslationFuture, TranslatorPort},
};

enum TranslatorBackend {
    #[cfg(not(feature = "m1-demo"))]
    CredentialBacked {
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
        factory: lexift_translate::DeepLTranslatorFactory,
    },
    #[cfg(any(feature = "m1-demo", test))]
    Fixed(Arc<dyn TranslatorPort>),
}

pub(crate) struct TranslatorRuntime {
    provider: RwLock<ProviderConfig>,
    backend: TranslatorBackend,
}

impl TranslatorRuntime {
    #[cfg(not(feature = "m1-demo"))]
    pub(crate) fn credential_backed(
        provider: ProviderConfig,
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
    ) -> Result<Self> {
        Ok(Self {
            provider: RwLock::new(provider),
            backend: TranslatorBackend::CredentialBacked {
                store,
                credential_reference,
                factory: lexift_translate::DeepLTranslatorFactory::new()?,
            },
        })
    }

    #[cfg(any(feature = "m1-demo", test))]
    pub(crate) fn fixed(provider: ProviderConfig, translator: Arc<dyn TranslatorPort>) -> Self {
        Self {
            provider: RwLock::new(provider),
            backend: TranslatorBackend::Fixed(translator),
        }
    }

    pub(crate) fn validate(&self, provider: ProviderConfig) -> Result<()> {
        match (&self.backend, provider) {
            #[cfg(any(feature = "m1-demo", test))]
            (TranslatorBackend::Fixed(_), _) => Ok(()),
            #[cfg(not(feature = "m1-demo"))]
            (
                TranslatorBackend::CredentialBacked {
                    store,
                    credential_reference,
                    ..
                },
                ProviderConfig::DeepL,
            ) => {
                let credential_id = credential_reference
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                    .ok_or_else(|| Error::new("DeepL API key is not configured"))?;
                match store.get(&credential_id).map_err(credential_lookup_error)? {
                    Some(_) => Ok(()),
                    None => Err(Error::new("DeepL API key is missing")),
                }
            }
        }
    }

    pub(crate) fn commit(&self, provider: ProviderConfig) {
        *self
            .provider
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }
}

impl TranslatorPort for TranslatorRuntime {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
        let provider = *self
            .provider
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let translator = match (&self.backend, provider) {
            #[cfg(any(feature = "m1-demo", test))]
            (TranslatorBackend::Fixed(translator), _) => Ok(Arc::clone(translator)),
            #[cfg(not(feature = "m1-demo"))]
            (
                TranslatorBackend::CredentialBacked {
                    store,
                    credential_reference,
                    factory,
                },
                ProviderConfig::DeepL,
            ) => credential_reference
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .ok_or_else(|| Error::new("Translation provider is not configured"))
                .and_then(|credential_id| {
                    store
                        .get(&credential_id)
                        .map_err(credential_lookup_error)
                        .and_then(|secret| {
                            secret.ok_or_else(|| Error::new("Translation credential is missing"))
                        })
                })
                .map(|secret| factory.translator(secret)),
        };
        Box::pin(async move {
            match translator {
                Ok(translator) => translator.translate(request).await,
                Err(error) => Err(error),
            }
        })
    }
}

#[cfg(not(feature = "m1-demo"))]
fn credential_lookup_error(error: CredentialError) -> Error {
    let message = match error.kind() {
        CredentialErrorKind::Missing => "Translation credential is missing",
        CredentialErrorKind::PermissionDenied => "Credential access was denied",
        CredentialErrorKind::PlatformFailure => "Credential storage is unavailable",
        CredentialErrorKind::InvalidFormat => "Stored translation credential is invalid",
    };
    Error::new(message)
}

#[cfg(all(test, not(feature = "m1-demo")))]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use lexift_core::ports::credential::{CredentialResult, CredentialStore};

    use super::*;

    #[derive(Default)]
    struct FakeCredentialStore(Mutex<HashMap<String, String>>);

    impl CredentialStore for FakeCredentialStore {
        fn get(&self, id: &str) -> CredentialResult<Option<String>> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }

        fn set(&self, id: &str, secret: &str) -> CredentialResult<()> {
            self.0.lock().unwrap().insert(id.into(), secret.into());
            Ok(())
        }

        fn delete(&self, id: &str) -> CredentialResult<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }

    #[test]
    fn provider_reload_requires_the_referenced_credential() {
        let store = Arc::new(FakeCredentialStore::default());
        let reference = Arc::new(RwLock::new(Some("deepl-primary".into())));
        let runtime =
            TranslatorRuntime::credential_backed(ProviderConfig::DeepL, store.clone(), reference)
                .unwrap();

        assert_eq!(
            runtime
                .validate(ProviderConfig::DeepL)
                .unwrap_err()
                .to_string(),
            "DeepL API key is missing"
        );
        store.set("deepl-primary", "test-key").unwrap();
        runtime.validate(ProviderConfig::DeepL).unwrap();
        runtime.commit(ProviderConfig::DeepL);
    }
}
