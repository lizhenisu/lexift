use std::net::IpAddr;

use crate::{Error, Result};

/// User-owned local service configuration. Model weights are never managed by Lexift.
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

/// Strict authority parsing prevents URL normalization from accepting disguised remote hosts.
/// Returns a canonical HTTP origin; localhost is pinned to loopback rather than resolved by DNS.
pub fn local_service_address(input: &str) -> Result<String> {
    let invalid = || Error::new("Use an HTTP loopback address with a valid port and no path");
    if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    let authority = input.strip_prefix("http://").ok_or_else(invalid)?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.contains(['/', '?', '#', '@', '\\', '%']) {
        return Err(invalid());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, rest) = rest.split_once(']').ok_or_else(invalid)?;
        host.parse::<std::net::Ipv6Addr>().map_err(|_| invalid())?;
        (host, rest.strip_prefix(':').ok_or_else(invalid)?)
    } else {
        if authority.matches(':').count() != 1 {
            return Err(invalid());
        }
        authority.rsplit_once(':').ok_or_else(invalid)?
    };
    if port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let port: u16 = port.parse().map_err(|_| invalid())?;
    if port == 0 {
        return Err(invalid());
    }
    let host = if host.eq_ignore_ascii_case("localhost") {
        "127.0.0.1".to_owned()
    } else {
        match host.parse::<IpAddr>().map_err(|_| invalid())? {
            IpAddr::V4(ip) if ip.is_loopback() => ip.to_string(),
            IpAddr::V6(ip) if ip.is_loopback() => format!("[{ip}]"),
            _ => return Err(invalid()),
        }
    };
    Ok(format!("http://{host}:{port}"))
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
                address: "http://remote:80".into(),
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
    fn only_explicit_loopback_origins_are_accepted() {
        for address in [
            "http://localhost:11434",
            "http://127.0.0.2:80/",
            "http://[::1]:65535",
        ] {
            assert!(local_service_address(address).is_ok(), "{address}");
        }
        for address in [
            "https://localhost:11434",
            "http://example.org:11434",
            "http://192.168.1.2:11434",
            "http://127.0.0.1:0",
            "http://localhost:65536",
            "http://localhost",
            "http://user@localhost:80",
            "http://localhost:80/?x",
            "http://localhost:80/#x",
            "http://localhost:80/api",
            "http://127.1:80",
            "http://localhost:80\\remote",
            "http://::1:80",
            "http://[127.0.0.1]:80",
            " http://localhost:80",
        ] {
            assert!(local_service_address(address).is_err(), "{address}");
        }
    }
}
