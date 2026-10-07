use crate::{Error, Result};

/// Legacy Ollama configuration shape retained for schema migration and adapter callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaConfig {
    pub address: String,
    pub model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            address: "http://127.0.0.1:11434".into(),
            model: String::new(),
        }
    }
}

impl OllamaConfig {
    pub fn validate(&self) -> Result<()> {
        local_service_address(&self.address)?;
        if self.model.len() > 256 || self.model.chars().any(char::is_control) {
            return Err(Error::new("Invalid Ollama model name"));
        }
        Ok(())
    }
}

/// Compatibility alias for the common HTTP(S) service-origin validator.
pub fn local_service_address(input: &str) -> Result<String> {
    super::llm::service_address(input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaModelIssue {
    EmbeddingOnly,
    Remote,
    UnknownCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    pub name: String,
    pub issue: Option<OllamaModelIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaConnectionState {
    Idle,
    Checking,
    Connected,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_configuration_is_rejected_before_persistence() {
        use crate::{
            AppCommand, AppEvent, AppState,
            domain::settings::{SettingsChange, SettingsFeedback, SettingsField},
        };
        let mut state = AppState::default();
        let original = state.desired_settings.clone();
        let commands = state.reduce(AppEvent::SettingsChangeRequested {
            change: SettingsChange::Ollama(OllamaConfig {
                address: "http://remote:0".into(),
                model: "test".into(),
            }),
        });
        assert_eq!(state.desired_settings, original);
        assert!(!state.settings_saving);
        assert_eq!(
            commands,
            [AppCommand::ShowSettingsFeedback {
                feedback: SettingsFeedback::SettingsSaveFailed(SettingsField::Ollama)
            }]
        );
    }
    #[test]
    fn explicit_service_origins_are_accepted() {
        for address in [
            "http://localhost:11434",
            "http://127.0.0.2:80/",
            "http://[::1]:65535",
        ] {
            assert!(local_service_address(address).is_ok(), "{address}");
        }
        for address in [
            "http://127.0.0.1:0",
            "http://localhost:65536",
            "http://user@localhost:80",
            "http://localhost:80/?x",
            "http://localhost:80/#x",
            "http://localhost:80/api",
            "http://localhost:80\\remote",
            "http://::1:80",
            "http://[127.0.0.1]:80",
            " http://localhost:80",
        ] {
            assert!(local_service_address(address).is_err(), "{address}");
        }
    }
}
