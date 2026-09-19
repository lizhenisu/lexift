use std::{error, fmt};

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialSecret(String);

impl CredentialSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Why the UI requested temporary access to a stored credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialAccessPurpose {
    Reveal,
    Edit,
    Copy,
}

impl fmt::Debug for CredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialSecret([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErrorKind {
    Missing,
    PermissionDenied,
    PlatformFailure,
    InvalidFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError {
    kind: CredentialErrorKind,
    message: String,
}

impl CredentialError {
    pub fn new(kind: CredentialErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> CredentialErrorKind {
        self.kind
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for CredentialError {}

pub type CredentialResult<T> = std::result::Result<T, CredentialError>;

pub trait CredentialStore: Send + Sync {
    fn get(&self, id: &str) -> CredentialResult<Option<String>>;
    fn set(&self, id: &str, secret: &str) -> CredentialResult<()>;
    fn delete(&self, id: &str) -> CredentialResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_secret_debug_output_is_always_redacted() {
        let secret = CredentialSecret::new("never-log-this-key");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "CredentialSecret([REDACTED])");
        assert!(!debug.contains(secret.expose()));
    }
}
