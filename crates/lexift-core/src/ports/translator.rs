use std::{future::Future, pin::Pin};

use crate::{
    Result,
    domain::translation::{TranslateRequest, TranslateResult},
};

pub type TranslationFuture<'a> = Pin<Box<dyn Future<Output = Result<TranslateResult>> + Send + 'a>>;
/// Ordered translation body fragments. Reasoning and protocol metadata are excluded.
pub type TranslationProgress = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// Asynchronous translation capability implemented by provider adapters.
pub trait TranslatorPort: Send + Sync {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_>;
    fn translate_with_progress(
        &self,
        request: TranslateRequest,
        _progress: TranslationProgress,
    ) -> TranslationFuture<'_> {
        self.translate(request)
    }
}
