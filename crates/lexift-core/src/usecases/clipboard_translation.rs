use crate::{
    Result,
    domain::{
        language::Language,
        translation::{TranslateRequest, TranslateResult},
    },
    ports::{clipboard::ClipboardPort, translator::TranslatorPort},
};

pub fn execute(
    clipboard: &impl ClipboardPort,
    translator: &impl TranslatorPort,
    target_language: Language,
) -> Result<Option<TranslateResult>> {
    let Some(text) = clipboard.read_text()? else {
        return Ok(None);
    };
    translator
        .translate(TranslateRequest {
            text,
            target_language,
        })
        .map(Some)
}
