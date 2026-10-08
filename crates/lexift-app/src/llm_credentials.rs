//! Resolve service credentials identically for discovery and generation, on their workers.
use lexift_core::{
    Error, Result,
    domain::llm::{LlmConnection, LlmProvider},
    ports::credential::CredentialStore,
};

pub(crate) fn resolve(
    store: &dyn CredentialStore,
    connection: &LlmConnection,
) -> Result<Option<String>> {
    resolve_with_environment(store, connection, || {
        std::env::var("LEXIFT_DEEPSEEK_API_KEY").ok()
    })
}
fn resolve_with_environment(
    store: &dyn CredentialStore,
    connection: &LlmConnection,
    environment: impl FnOnce() -> Option<String>,
) -> Result<Option<String>> {
    if let Some(id) = connection.credential_id.as_deref()
        && let Some(secret) = store
            .get(id)
            .map_err(|_| Error::new("Credential storage is unavailable"))?
    {
        return Ok(Some(secret));
    }
    Ok(if connection.provider == LlmProvider::DeepSeek {
        environment().filter(|s| !s.trim().is_empty())
    } else {
        None
    })
}

pub(crate) fn resolve_authorization(
    store: &dyn CredentialStore,
    connection: &LlmConnection,
) -> Result<lexift_core::ports::llm::LlmAuthorization> {
    use lexift_core::domain::llm::Authentication;
    let profile = connection.profile();
    let key = if profile.authentication == Authentication::Automatic {
        resolve(store, connection)?
    } else {
        None
    };
    let mut headers = Vec::new();
    for header in profile.headers {
        let value = store
            .get(&header.credential_id)
            .map_err(|_| Error::new("Credential storage is unavailable"))?
            .ok_or_else(|| Error::new("Request header credential is missing"))?;
        headers.push((header.name, value));
    }
    Ok(lexift_core::ports::llm::LlmAuthorization { key, headers })
}

pub(crate) fn query_authorization(
    store: &dyn CredentialStore,
    connection: &LlmConnection,
    mode: lexift_core::ports::llm::LlmQueryMode,
) -> Result<(
    lexift_core::ports::llm::LlmAuthorization,
    lexift_core::ports::llm::LlmQueryMode,
)> {
    use lexift_core::ports::llm::LlmQueryMode;
    let (mode, updates) = match mode {
        LlmQueryMode::Draft { operation, headers } => (*operation, headers),
        other => (other, Vec::new()),
    };
    let mut effective = connection.clone();
    for profile in &mut effective.protocols {
        profile.headers.retain(|h| {
            !updates
                .iter()
                .any(|u| u.protocol == profile.protocol && u.name.eq_ignore_ascii_case(&h.name))
        });
    }
    let mut auth = resolve_authorization(store, &effective)?;
    for update in updates
        .into_iter()
        .filter(|u| u.protocol == connection.protocol)
    {
        auth.headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case(&update.name));
        if let Some(secret) = update.secret {
            auth.headers.push((update.name, secret.into_inner()));
        }
    }
    Ok((auth, mode))
}

pub(crate) fn discovery_callback(
    runtime: tokio::runtime::Handle,
    store: std::sync::Arc<dyn CredentialStore>,
    service: std::sync::Arc<dyn lexift_core::ports::llm::LlmServicePort>,
) -> impl Fn(
    LlmConnection,
    lexift_core::ports::llm::LlmQueryMode,
    lexift_ui::LlmQueryCompletion,
) -> lexift_ui::LlmQueryCancellation {
    move |connection, mode, complete| {
        let service = service.clone();
        let store = store.clone();
        let task = runtime.spawn(async move {
            use lexift_core::ports::llm::LlmQueryMode;
            let captured = connection.clone();
            let auth = tokio::task::spawn_blocking(move || {
                query_authorization(store.as_ref(), &captured, mode)
            })
            .await;
            let result = match auth {
                Ok(Ok((auth, LlmQueryMode::Models))) => {
                    service.models_authenticated(connection, auth).await
                }
                Ok(Ok((auth, LlmQueryMode::Check))) => {
                    service.check_authenticated(connection, auth).await
                }
                Ok(Ok((auth, LlmQueryMode::Test(request)))) => service
                    .generate_authenticated(connection, auth, request)
                    .await
                    .map(|_| Vec::new()),
                Ok(Ok((_, LlmQueryMode::Draft { .. }))) => {
                    Err(Error::new("Invalid nested query draft"))
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(Error::new("Credential worker failed")),
            };
            complete(result);
        });
        let abort = task.abort_handle();
        Box::new(move || abort.abort())
    }
}

/// Commit settings and all request-header credentials together, restoring prior secrets on failure.
pub(crate) fn save_connection(
    store: &dyn CredentialStore,
    settings_store: &dyn lexift_core::ports::settings::SettingsStore,
    mut settings: lexift_core::domain::settings::Settings,
    mut connection: LlmConnection,
    headers: Vec<lexift_core::ports::llm::LlmHeaderUpdate>,
) -> Result<lexift_core::domain::llm::LlmSettings> {
    use lexift_core::domain::llm::*;
    let original = settings.llm.connection(connection.provider);
    connection.credential_id = original.credential_id;
    // Ordinary drafts cannot replace secure references.
    for profile in &mut connection.protocols {
        profile.headers = original
            .protocols
            .iter()
            .find(|p| p.protocol == profile.protocol)
            .map(|p| p.headers.clone())
            .unwrap_or_default();
    }
    let mut changes = Vec::new();
    for update in headers {
        validate_header_name(&update.name)?;
        if update
            .secret
            .as_ref()
            .is_some_and(|s| s.expose().contains(['\r', '\n']))
        {
            return Err(Error::new("Invalid request header value"));
        }
        let active = connection.protocol;
        connection.protocol = update.protocol;
        let mut profile = connection.profile();
        connection.protocol = active;
        let id = profile
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(&update.name))
            .map(|h| h.credential_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "llm-{}-{}-header-{}",
                    connection.provider.id(),
                    update.protocol.id(),
                    update.name.to_ascii_lowercase()
                )
            });
        profile
            .headers
            .retain(|h| !h.name.eq_ignore_ascii_case(&update.name));
        if update.secret.is_some() {
            profile.headers.push(HeaderReference {
                name: update.name,
                credential_id: id.clone(),
            });
        }
        connection
            .protocols
            .retain(|p| p.protocol != update.protocol);
        connection.protocols.push(profile);
        changes.push((id, update.secret));
    }
    connection.enabled = settings.llm.connection(connection.provider).enabled;
    connection.validate()?;
    settings
        .llm
        .connections
        .retain(|c| c.provider != connection.provider);
    settings.llm.connections.push(connection);
    settings.llm.validate()?;
    let mut previous = Vec::new();
    let result = (|| {
        for (id, secret) in changes {
            let old = store
                .get(&id)
                .map_err(|_| Error::new("Credential storage is unavailable"))?;
            previous.push((id.clone(), old));
            if let Some(secret) = secret {
                store.set(&id, secret.expose())
            } else {
                store.delete(&id)
            }
            .map_err(|_| Error::new("Credential storage is unavailable"))?;
        }
        settings_store.save(&settings)?;
        Ok(settings.llm)
    })();
    if result.is_err() {
        for (id, secret) in previous.into_iter().rev() {
            if let Some(secret) = secret {
                let _ = store.set(&id, &secret);
            } else {
                let _ = store.delete(&id);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::ports::credential::CredentialResult;
    struct Store(Option<String>);
    impl CredentialStore for Store {
        fn get(&self, _: &str) -> CredentialResult<Option<String>> {
            Ok(self.0.clone())
        }
        fn set(&self, _: &str, _: &str) -> CredentialResult<()> {
            panic!("resolution must not write credentials")
        }
        fn delete(&self, _: &str) -> CredentialResult<()> {
            panic!("resolution must not delete credentials")
        }
    }
    #[test]
    fn connection_apply_commits_all_headers_and_restores_secrets_on_failure() {
        use lexift_core::{
            domain::{llm::*, settings::Settings},
            ports::{credential::*, llm::*, settings::SettingsStore},
        };
        use std::{collections::BTreeMap, sync::Mutex};
        #[derive(Default)]
        struct Memory(Mutex<BTreeMap<String, String>>);
        impl CredentialStore for Memory {
            fn get(&self, id: &str) -> CredentialResult<Option<String>> {
                Ok(self.0.lock().unwrap().get(id).cloned())
            }
            fn set(&self, id: &str, value: &str) -> CredentialResult<()> {
                self.0.lock().unwrap().insert(id.into(), value.into());
                Ok(())
            }
            fn delete(&self, id: &str) -> CredentialResult<()> {
                self.0.lock().unwrap().remove(id);
                Ok(())
            }
        }
        struct Config(bool);
        impl SettingsStore for Config {
            fn load(&self) -> Result<Settings> {
                Ok(Settings::default())
            }
            fn save(&self, _: &Settings) -> Result<()> {
                if self.0 {
                    Err(Error::new("fixture config write failed"))
                } else {
                    Ok(())
                }
            }
        }
        for failure in [false, true] {
            let store = Memory::default();
            let mut settings = Settings::default();
            let mut connection = settings.llm.connection(LlmProvider::LmStudio);
            let mut profile = connection.profile();
            profile.authentication = Authentication::Headers;
            profile.headers.push(HeaderReference {
                name: "Authorization".into(),
                credential_id: "existing-header-reference".into(),
            });
            connection.protocols.push(profile);
            store
                .set("existing-header-reference", "Bearer old-test-value")
                .unwrap();
            settings
                .llm
                .connections
                .retain(|c| c.provider != connection.provider);
            settings.llm.connections.push(connection.clone());
            let updates = vec![
                LlmHeaderUpdate {
                    protocol: LlmProtocol::Native,
                    name: "Authorization".into(),
                    secret: Some(CredentialSecret::new("Bearer new-test-value")),
                },
                LlmHeaderUpdate {
                    protocol: LlmProtocol::Native,
                    name: "X-Private".into(),
                    secret: Some(CredentialSecret::new("new-header-test-value")),
                },
            ];
            assert!(!format!("{updates:?}").contains("new-test-value"));
            settings.llm.set_provider_enabled(
                lexift_core::domain::runtime_config::ProviderConfig::LmStudio,
                false,
            );
            connection.enabled = true; // stale page draft predates the immediate toggle
            let result = save_connection(&store, &Config(failure), settings, connection, updates);
            if failure {
                assert!(result.is_err());
                assert_eq!(store.0.lock().unwrap().len(), 1);
                assert_eq!(
                    store.get("existing-header-reference").unwrap().as_deref(),
                    Some("Bearer old-test-value")
                );
            } else {
                let config = result.unwrap();
                assert!(!config.connection(LlmProvider::LmStudio).enabled);
                assert_eq!(
                    config
                        .connection(LlmProvider::LmStudio)
                        .profile()
                        .headers
                        .len(),
                    2
                );
                assert_eq!(
                    store.get("existing-header-reference").unwrap().as_deref(),
                    Some("Bearer new-test-value")
                );
                assert!(!format!("{config:?}").contains("new-test-value"));
            }
        }
    }
    #[test]
    fn saved_key_wins_and_missing_key_uses_environment_only_for_deepseek() {
        let mut connection = LlmConnection::new(LlmProvider::DeepSeek);
        connection.credential_id = Some("reference".into());
        assert_eq!(
            resolve_with_environment(
                &Store(Some("saved-test-key".into())),
                &connection,
                || panic!("environment must not override saved credential")
            )
            .unwrap()
            .as_deref(),
            Some("saved-test-key")
        );
        assert_eq!(
            resolve_with_environment(&Store(None), &connection, || Some(
                "environment-test-key".into()
            ))
            .unwrap()
            .as_deref(),
            Some("environment-test-key")
        );
        connection.provider = LlmProvider::Ollama;
        assert!(
            resolve_with_environment(&Store(None), &connection, || panic!(
                "DeepSeek key must never reach another provider"
            ))
            .unwrap()
            .is_none()
        );
    }
}
