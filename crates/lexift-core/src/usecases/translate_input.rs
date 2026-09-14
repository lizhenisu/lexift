use crate::{
    Result,
    domain::translation::{TranslateRequest, TranslateResult},
    ports::translator::TranslatorPort,
};

pub async fn execute(
    translator: &(impl TranslatorPort + ?Sized),
    request: TranslateRequest,
) -> Result<TranslateResult> {
    translator.translate(request).await
}
