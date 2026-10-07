use crate::{Result, domain::ollama::OllamaModel};
use std::{future::Future, pin::Pin};

pub type OllamaModelsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<OllamaModel>>> + Send + 'a>>;

/// Read-only discovery of user-installed local models; never downloads or loads weights.
pub trait OllamaServicePort: Send + Sync {
    fn models(&self, address: String) -> OllamaModelsFuture<'_>;
}
