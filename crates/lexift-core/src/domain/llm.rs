//! Service connections are reusable; translation preferences belong to the translation use case.
use crate::{Error, Result};

pub const DEFAULT_TRANSLATION_PROMPT: &str = "You are a professional translator. Translate the input text to {target_language}.\n\nRules:\n\n- Output ONLY the translation, no explanation, no quotes.\n\n- Preserve original formatting, line breaks, and punctuation.\n\n- For technical terms, keep the English original in parentheses.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LlmProvider {
    Ollama,
    LmStudio,
    DeepSeek,
}
impl LlmProvider {
    pub const ALL: [Self; 3] = [Self::Ollama, Self::LmStudio, Self::DeepSeek];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
            Self::DeepSeek => "deepseek",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.id() == value)
            .ok_or_else(|| Error::new("Unknown model provider"))
    }
    pub const fn default_address(self) -> &'static str {
        match self {
            Self::Ollama => "http://127.0.0.1:11434",
            Self::LmStudio => "http://127.0.0.1:1234",
            Self::DeepSeek => "https://api.deepseek.com",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConnection {
    pub provider: LlmProvider,
    pub address: String,
    pub credential_id: Option<String>,
    pub enabled: bool,
    pub protocol: LlmProtocol,
    pub protocols: Vec<ProtocolSettings>,
}
impl LlmConnection {
    pub fn new(provider: LlmProvider) -> Self {
        Self {
            provider,
            address: provider.default_address().into(),
            credential_id: None,
            enabled: true,
            protocol: LlmProtocol::Native,
            protocols: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        service_address(&self.address)?;
        let mut protocols = std::collections::BTreeSet::new();
        for profile in &self.protocols {
            if !protocols.insert(profile.protocol.id()) {
                return Err(Error::new("Duplicate protocol profile"));
            }
            api_base_address(&profile.base_url)?;
            let mut names = std::collections::BTreeSet::new();
            for header in &profile.headers {
                validate_header_name(&header.name)?;
                if !names.insert(header.name.to_ascii_lowercase())
                    || header.credential_id.is_empty()
                {
                    return Err(Error::new("Duplicate or invalid request header"));
                }
                if profile.authentication == Authentication::Automatic
                    && ["authorization", "x-api-key"]
                        .iter()
                        .any(|name| header.name.eq_ignore_ascii_case(name))
                {
                    return Err(Error::new(
                        "Custom authorization and automatic API key are mutually exclusive",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn profile(&self) -> ProtocolSettings {
        self.protocols
            .iter()
            .find(|p| p.protocol == self.protocol)
            .cloned()
            .unwrap_or_else(|| {
                let base_url = if self.protocol == LlmProtocol::Native
                    || self.provider == LlmProvider::DeepSeek
                {
                    self.address.clone()
                } else {
                    format!("{}/v1", self.address.trim_end_matches('/'))
                };
                ProtocolSettings {
                    protocol: self.protocol,
                    base_url,
                    authentication: Authentication::Automatic,
                    headers: Vec::new(),
                }
            })
    }
    /// Compare effective requests, including a lazily materialized default protocol profile.
    pub fn same_request_configuration(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.enabled == other.enabled
            && self.address == other.address
            && self.protocol == other.protocol
            && self.credential_id == other.credential_id
            && self.profile() == other.profile()
    }
    pub fn endpoint_key(&self) -> String {
        format!("{}:{}", self.protocol.id(), self.profile().base_url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmProtocol {
    #[default]
    Native,
    ChatCompletions,
    Responses,
}
impl LlmProtocol {
    pub const ALL: [Self; 3] = [Self::Native, Self::ChatCompletions, Self::Responses];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::ChatCompletions => "chat-completions",
            Self::Responses => "responses",
        }
    }
    pub fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.id() == id)
            .ok_or_else(|| Error::new("Unknown model protocol"))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Authentication {
    #[default]
    Automatic,
    Headers,
    None,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderReference {
    pub name: String,
    pub credential_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSettings {
    pub protocol: LlmProtocol,
    pub base_url: String,
    pub authentication: Authentication,
    pub headers: Vec<HeaderReference>,
}
/// HTTP API base, including an optional deployment path; never accepts embedded credentials.
pub fn api_base_address(input: &str) -> Result<String> {
    let input = input.trim_end_matches('/');
    let (_, tail) = input
        .split_once("://")
        .ok_or_else(|| Error::new("Invalid API base URL"))?;
    let origin_end = input.len() - tail.len() + tail.find('/').unwrap_or(tail.len());
    service_address(&input[..origin_end])?;
    if input.contains(['?', '#', '\\'])
        || input.chars().any(|c| c.is_control() || c.is_whitespace())
        || input[origin_end..].contains('@')
    {
        return Err(Error::new("Invalid API base URL"));
    }
    Ok(input.into())
}
pub fn validate_header_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || [
            "host",
            "content-length",
            "connection",
            "transfer-encoding",
            "upgrade",
            "proxy-authorization",
        ]
        .contains(&name.to_ascii_lowercase().as_str())
    {
        return Err(Error::new("Invalid or reserved request header"));
    }
    Ok(())
}

/// Validate an explicit HTTP(S) service origin without resolving or restricting its host.
pub fn service_address(input: &str) -> Result<String> {
    let invalid = || Error::new("Use an HTTP or HTTPS service address without a path");
    if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    let (scheme, authority) = input.split_once("://").ok_or_else(invalid)?;
    if scheme != "http" && scheme != "https" {
        return Err(invalid());
    }
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@', '\\', '%']) {
        return Err(invalid());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or_else(invalid)?;
        host.parse::<std::net::Ipv6Addr>().map_err(|_| invalid())?;
        (
            format!("[{host}]"),
            if suffix.is_empty() {
                None
            } else {
                Some(suffix.strip_prefix(':').ok_or_else(invalid)?)
            },
        )
    } else {
        if authority.matches(':').count() > 1 {
            return Err(invalid());
        }
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(h, p)| (h, Some(p)));
        if host.is_empty()
            || !host.is_ascii()
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(invalid());
        }
        (host.to_ascii_lowercase(), port)
    };
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u16>().ok().is_none_or(|p| p == 0))
    {
        return Err(invalid());
    }
    Ok(format!(
        "{scheme}://{host}{}",
        port.map(|p| format!(":{p}")).unwrap_or_default()
    ))
}

/// Integer hundredths avoid float equality and persistence drift in settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationParameters {
    pub reasoning: String,
    pub temperature: Option<u16>,
    pub top_p: Option<u16>,
}
impl Default for GenerationParameters {
    fn default() -> Self {
        Self {
            reasoning: "off".into(),
            temperature: None,
            top_p: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationModel {
    pub streaming: bool,
    pub provider: LlmProvider,
    pub model: String,
    pub parameters: GenerationParameters,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmSettings {
    pub connections: Vec<LlmConnection>,
    pub models: Vec<TranslationModel>,
    pub prompt: String,
    pub catalog: Vec<CatalogModel>,
    pub deepl_enabled: bool,
}
impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            connections: LlmProvider::ALL
                .into_iter()
                .map(LlmConnection::new)
                .collect(),
            models: Vec::new(),
            prompt: DEFAULT_TRANSLATION_PROMPT.into(),
            catalog: Vec::new(),
            deepl_enabled: true,
        }
    }
}
impl LlmSettings {
    pub fn provider_enabled(&self, provider: super::runtime_config::ProviderConfig) -> bool {
        if provider == super::runtime_config::ProviderConfig::DeepL {
            return self.deepl_enabled;
        }
        LlmProvider::ALL
            .into_iter()
            .find(|p| p.id() == provider.id())
            .is_some_and(|p| self.connection(p).enabled)
    }
    /// Enablement is independent of connection and model configuration.
    pub fn set_provider_enabled(
        &mut self,
        provider: super::runtime_config::ProviderConfig,
        enabled: bool,
    ) {
        if provider == super::runtime_config::ProviderConfig::DeepL {
            self.deepl_enabled = enabled;
            return;
        }
        if let Some(p) = LlmProvider::ALL
            .into_iter()
            .find(|p| p.id() == provider.id())
        {
            let mut c = self.connection(p);
            c.enabled = enabled;
            self.connections.retain(|c| c.provider != p);
            self.connections.push(c);
        }
    }

    pub fn model_enabled(&self, provider: LlmProvider, id: &str) -> bool {
        let key = self.connection(provider).endpoint_key();
        self.catalog.iter().any(|m| {
            m.provider == provider
                && m.model.id == id
                && m.enabled
                && m.available
                && m.endpoint == key
                && m.model.issue.is_none()
        })
    }
    /// Retain missing entries and explicit choices; discovery never opts new models in.
    pub fn merge_models(&mut self, connection: &LlmConnection, models: Vec<LlmModel>) {
        for entry in self
            .catalog
            .iter_mut()
            .filter(|m| m.provider == connection.provider)
        {
            entry.available = false;
        }
        for model in models {
            if let Some(entry) = self
                .catalog
                .iter_mut()
                .find(|m| m.provider == connection.provider && m.model.id == model.id)
            {
                entry.model = model;
                entry.available = true;
                entry.endpoint = connection.endpoint_key();
            } else {
                self.catalog.push(CatalogModel {
                    provider: connection.provider,
                    model,
                    enabled: false,
                    available: true,
                    endpoint: connection.endpoint_key(),
                });
            }
        }
    }
    pub fn connection(&self, provider: LlmProvider) -> LlmConnection {
        self.connections
            .iter()
            .find(|c| c.provider == provider)
            .cloned()
            .unwrap_or_else(|| LlmConnection::new(provider))
    }
    pub fn selected(&self, provider: LlmProvider) -> Option<&TranslationModel> {
        self.models.iter().rev().find(|m| m.provider == provider)
    }
    pub fn validate(&self) -> Result<()> {
        if !self.prompt.contains("{target_language}") || self.prompt.len() > 32768 {
            return Err(Error::new(
                "Translation prompt must contain {target_language}",
            ));
        }
        for connection in &self.connections {
            connection.validate()?;
        }
        for model in &self.models {
            let p = &model.parameters;
            if model.model.len() > 256
                || model.model.chars().any(char::is_control)
                || p.temperature.is_some_and(|v| {
                    v > if model.provider == LlmProvider::LmStudio {
                        100
                    } else {
                        200
                    }
                })
                || p.top_p.is_some_and(|v| v == 0 || v > 100)
            {
                return Err(Error::new("Invalid model parameters"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub provider: LlmProvider,
    pub model: LlmModel,
    pub enabled: bool,
    pub available: bool,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmModel {
    pub id: String,
    pub aliases: Vec<String>,
    pub reasoning: Vec<String>,
    pub issue: Option<String>,
}
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub model: String,
    pub system: String,
    pub input: String,
    pub parameters: GenerationParameters,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(id: &str) -> LlmModel {
        LlmModel {
            id: id.into(),
            aliases: Vec::new(),
            reasoning: Vec::new(),
            issue: None,
        }
    }
    #[test]
    fn discovery_preserves_opt_in_missing_entries_and_endpoint_confirmation() {
        let mut config = LlmSettings::default();
        let mut connection = config.connection(LlmProvider::LmStudio);
        config.merge_models(&connection, vec![model("first"), model("second")]);
        assert!(config.catalog.iter().all(|m| !m.enabled));
        config.catalog[0].enabled = true;
        assert!(config.model_enabled(connection.provider, "first"));
        config.merge_models(&connection, vec![model("second")]);
        assert!(!config.model_enabled(connection.provider, "first"));
        assert!(config.catalog[0].enabled);
        config.merge_models(&connection, vec![model("first")]);
        assert!(config.model_enabled(connection.provider, "first"));
        connection.address = "http://192.168.1.2:1234".into();
        config
            .connections
            .retain(|c| c.provider != connection.provider);
        config.connections.push(connection.clone());
        assert!(!config.model_enabled(connection.provider, "first"));
        config.merge_models(&connection, vec![model("first")]);
        assert!(config.model_enabled(connection.provider, "first"));
    }
    #[test]
    fn deployment_paths_and_header_names_are_validated() {
        assert_eq!(
            api_base_address("https://host.example/gateway/v1/").unwrap(),
            "https://host.example/gateway/v1"
        );
        for value in [
            "https://user@host/v1",
            "http://host/v1?secret=x",
            "http://host/v1#part",
            "http://host/v1\n",
        ] {
            assert!(api_base_address(value).is_err());
        }
        for name in ["Host", "Content-Length", "Connection", "Bad\r\nName", ""] {
            assert!(validate_header_name(name).is_err());
        }
        assert!(validate_header_name("X-Private-Token").is_ok());
        let original = LlmConnection::new(LlmProvider::LmStudio);
        let mut materialized = original.clone();
        materialized.protocols.push(original.profile());
        assert!(original.same_request_configuration(&materialized));
        let mut connection = LlmConnection::new(LlmProvider::Ollama);
        let mut profile = connection.profile();
        profile.headers = vec![
            HeaderReference {
                name: "X-Test".into(),
                credential_id: "one".into(),
            },
            HeaderReference {
                name: "x-test".into(),
                credential_id: "two".into(),
            },
        ];
        connection.protocols.push(profile);
        assert!(connection.validate().is_err());
    }
    #[test]
    fn explicit_origins_support_network_services() {
        for s in [
            "http://localhost:1234",
            "http://192.168.1.2:11434/",
            "https://models.example.com",
            "http://[2001:db8::1]:1234",
        ] {
            assert!(service_address(s).is_ok(), "{s}");
        }
        for s in [
            "file://host",
            "http://user@host",
            "http://host/api",
            "http://host?key=x",
            "http://host:0",
            "http://host:65536",
            "http://::1:1234",
            "http://host\\other",
        ] {
            assert!(service_address(s).is_err(), "{s}");
        }
    }
    #[test]
    fn prompt_and_sampling_are_validated() {
        let mut s = LlmSettings::default();
        assert!(s.validate().is_ok());
        s.prompt = "missing placeholder".into();
        assert!(s.validate().is_err());
    }
}
