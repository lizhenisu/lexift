use crate::{
    Result,
    domain::llm::{GenerateRequest, LlmConnection, LlmModel},
};
use std::{future::Future, pin::Pin};
pub type ModelsFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<LlmModel>>> + Send + 'a>>;
pub type GenerateFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
/// Stateless model discovery and text generation; owns no translation policy or model weights.
pub trait LlmServicePort: Send + Sync {
    fn check_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
    ) -> ModelsFuture<'_> {
        self.models_authenticated(connection, auth)
    }

    fn models_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
    ) -> ModelsFuture<'_>;
    fn generate_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        request: GenerateRequest,
    ) -> GenerateFuture<'_>;
    fn generate_with_progress(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        request: GenerateRequest,
        _progress: crate::ports::translator::TranslationProgress,
    ) -> GenerateFuture<'_> {
        self.generate_authenticated(connection, auth, request)
    }
    fn models(&self, connection: LlmConnection, credential: Option<String>) -> ModelsFuture<'_> {
        self.models_authenticated(
            connection,
            LlmAuthorization {
                key: credential,
                headers: Vec::new(),
            },
        )
    }
    fn generate(
        &self,
        connection: LlmConnection,
        credential: Option<String>,
        request: GenerateRequest,
    ) -> GenerateFuture<'_> {
        self.generate_authenticated(
            connection,
            LlmAuthorization {
                key: credential,
                headers: Vec::new(),
            },
            request,
        )
    }
}

/// Request-only secrets, resolved by App; never persisted in ordinary settings or Debug output.
#[derive(Clone, Default)]
pub struct LlmAuthorization {
    pub key: Option<String>,
    pub headers: Vec<(String, String)>,
}
impl std::fmt::Debug for LlmAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LlmAuthorization([REDACTED])")
    }
}

#[derive(Debug, Clone)]
pub enum LlmQueryMode {
    Check,
    Models,
    Test(GenerateRequest),
    Draft {
        operation: Box<LlmQueryMode>,
        headers: Vec<LlmHeaderUpdate>,
    },
}

/// Secure values remain in the window draft until an atomic connection Apply operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmHeaderUpdate {
    pub protocol: crate::domain::llm::LlmProtocol,
    pub name: String,
    pub secret: Option<crate::ports::credential::CredentialSecret>,
}
