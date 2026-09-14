use std::{future::Future, pin::Pin};

use crate::{
    Result,
    domain::translation::{TranslateRequest, TranslateResult},
};

pub type TranslationFuture<'a> = Pin<Box<dyn Future<Output = Result<TranslateResult>> + Send + 'a>>;

/// Asynchronous translation capability implemented by provider adapters.
pub trait TranslatorPort: Send + Sync {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_>;
}
