use std::sync::{Arc, RwLock};

#[cfg(not(feature = "m1-demo"))]
use lexift_core::{
    Error,
    ports::credential::{CredentialError, CredentialErrorKind, CredentialStore},
};
use lexift_core::{
    Result,
    domain::{ollama::OllamaConfig, runtime_config::ProviderConfig, translation::TranslateRequest},
    ports::translator::{TranslationFuture, TranslatorPort},
};

#[derive(Clone)]
enum TranslatorBackend {
    #[cfg(not(feature = "m1-demo"))]
    CredentialBacked {
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
        factory: lexift_translate::DeepLTranslatorFactory,
        llm: lexift_translate::providers::llm::LlmService,
    },
    #[cfg(any(feature = "m1-demo", test))]
    Fixed(Arc<dyn TranslatorPort>),
}

pub(crate) struct TranslatorRuntime {
    config: RwLock<(
        ProviderConfig,
        OllamaConfig,
        lexift_core::domain::llm::LlmSettings,
    )>,
    backend: TranslatorBackend,
}

impl TranslatorRuntime {
    /// Freeze configuration before the task is scheduled, without reading credentials on the UI thread.
    pub(crate) fn snapshot(&self) -> Arc<dyn TranslatorPort> {
        Arc::new(Self {
            config: RwLock::new(
                self.config
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            ),
            backend: self.backend.clone(),
        })
    }
    #[cfg(not(feature = "m1-demo"))]
    pub(crate) fn credential_backed(
        provider: ProviderConfig,
        ollama_config: OllamaConfig,
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
    ) -> Result<Self> {
        Ok(Self {
            config: RwLock::new((provider, ollama_config, Default::default())),
            backend: TranslatorBackend::CredentialBacked {
                store,
                credential_reference,
                factory: lexift_translate::DeepLTranslatorFactory::new()?,
                llm: lexift_translate::providers::llm::LlmService::new()?,
            },
        })
    }

    #[cfg(any(feature = "m1-demo", test))]
    pub(crate) fn fixed(provider: ProviderConfig, translator: Arc<dyn TranslatorPort>) -> Self {
        Self {
            config: RwLock::new((provider, OllamaConfig::default(), Default::default())),
            backend: TranslatorBackend::Fixed(translator),
        }
    }

    pub(crate) fn validate(&self, provider: ProviderConfig) -> Result<()> {
        match (&self.backend, provider) {
            #[cfg(not(feature = "m1-demo"))]
            (
                TranslatorBackend::CredentialBacked { .. },
                ProviderConfig::Ollama | ProviderConfig::LmStudio | ProviderConfig::DeepSeek,
            ) => Ok(()),
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
        self.config.write().unwrap_or_else(|p| p.into_inner()).0 = provider;
    }

    pub(crate) fn configure_llm(
        &self,
        config: lexift_core::domain::llm::LlmSettings,
    ) -> Result<()> {
        config.validate()?;
        self.config.write().unwrap_or_else(|p| p.into_inner()).2 = config;
        Ok(())
    }

    pub(crate) fn configure_ollama(&self, config: OllamaConfig) -> Result<()> {
        config.validate()?;
        self.config.write().unwrap_or_else(|p| p.into_inner()).1 = config;
        Ok(())
    }
}

impl TranslatorPort for TranslatorRuntime {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
        let (provider, _ollama_config, llm_config) = self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if (provider == ProviderConfig::DeepL && !llm_config.deepl_enabled)
            || lexift_core::domain::llm::LlmProvider::parse(provider.id())
                .ok()
                .is_some_and(|p| !llm_config.connection(p).enabled)
        {
            return Box::pin(async { Err(lexift_core::Error::new("Provider is disabled")) });
        }
        let translator = match (&self.backend, provider) {
            #[cfg(not(feature = "m1-demo"))]
            (
                TranslatorBackend::CredentialBacked { llm, store, .. },
                ProviderConfig::Ollama | ProviderConfig::LmStudio | ProviderConfig::DeepSeek,
            ) => {
                use lexift_core::domain::llm::LlmProvider;
                let kind = match provider {
                    ProviderConfig::Ollama => LlmProvider::Ollama,
                    ProviderConfig::LmStudio => LlmProvider::LmStudio,
                    _ => LlmProvider::DeepSeek,
                };
                if let Some(model) = llm_config.selected(kind) {
                    let connection = llm_config.connection(kind);
                    if !llm_config.model_enabled(kind, &model.model) {
                        return Box::pin(async {
                            Err(Error::new(
                                "Selected model is not enabled or needs confirmation in Provider settings",
                            ))
                        });
                    }
                    let secret =
                        crate::llm_credentials::resolve_authorization(store.as_ref(), &connection);
                    secret.map(|secret| {
                        llm.translator_authenticated(
                            connection,
                            secret,
                            model.clone(),
                            llm_config.prompt.clone(),
                        )
                    })
                } else {
                    Err(Error::new("Select a model in translation settings first"))
                }
            }
            #[cfg(any(feature = "m1-demo", test))]
            (TranslatorBackend::Fixed(translator), _) => Ok(Arc::clone(translator)),
            #[cfg(not(feature = "m1-demo"))]
            (
                TranslatorBackend::CredentialBacked {
                    store,
                    credential_reference,
                    factory,
                    ..
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

    #[test]
    fn ollama_needs_no_key_and_translation_keeps_its_original_configuration() {
        let runtime = TranslatorRuntime::credential_backed(
            ProviderConfig::Ollama,
            OllamaConfig::default(),
            Arc::new(FakeCredentialStore::default()),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();
        runtime.validate(ProviderConfig::Ollama).unwrap();
        let request = TranslateRequest {
            text: "test".into(),
            source_language: None,
            target_language: lexift_core::domain::language::Language("en-US".into()),
        };
        let frozen = runtime.translate(request.clone());
        let scheduled = runtime.snapshot();
        runtime
            .configure_ollama(OllamaConfig {
                model: "now configured".into(),
                ..Default::default()
            })
            .unwrap();
        runtime.commit(ProviderConfig::DeepL);
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            executor.block_on(frozen).unwrap_err().to_string(),
            "Select a model in translation settings first"
        );
        assert_eq!(
            executor
                .block_on(scheduled.translate(request.clone()))
                .unwrap_err()
                .to_string(),
            "Select a model in translation settings first"
        );
        assert_eq!(
            executor
                .block_on(runtime.translate(request))
                .unwrap_err()
                .to_string(),
            "Translation provider is not configured"
        );
        assert_eq!(runtime.config.read().unwrap().1.model, "now configured");
        assert!(
            runtime
                .configure_ollama(OllamaConfig {
                    address: "http://remote:0".into(),
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(runtime.config.read().unwrap().1.model, "now configured");
    }

    #[test]
    fn disabling_provider_blocks_new_requests_and_preserves_existing_snapshots() {
        let runtime = TranslatorRuntime::credential_backed(
            ProviderConfig::Ollama,
            OllamaConfig::default(),
            Arc::new(FakeCredentialStore::default()),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();
        let frozen = runtime.snapshot();
        let mut config = lexift_core::domain::llm::LlmSettings::default();
        config.connections[0].enabled = false;
        runtime.configure_llm(config).unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let request = TranslateRequest {
            text: "test".into(),
            source_language: None,
            target_language: lexift_core::domain::language::Language("en-US".into()),
        };
        assert_eq!(
            executor
                .block_on(runtime.translate(request.clone()))
                .unwrap_err()
                .to_string(),
            "Provider is disabled"
        );
        assert_eq!(
            executor
                .block_on(frozen.translate(request))
                .unwrap_err()
                .to_string(),
            "Select a model in translation settings first"
        );
    }

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
        let runtime = TranslatorRuntime::credential_backed(
            ProviderConfig::DeepL,
            OllamaConfig::default(),
            store.clone(),
            reference,
        )
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
