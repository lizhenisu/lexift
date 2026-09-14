use crate::{
    Result,
    domain::translation::{TranslateRequest, TranslateResult},
    ports::translator::TranslatorPort,
};

pub fn execute(
    translator: &impl TranslatorPort,
    request: TranslateRequest,
) -> Result<TranslateResult> {
    translator.translate(request)
}
