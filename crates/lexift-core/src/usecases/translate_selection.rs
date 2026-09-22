use crate::{
    Result,
    domain::{language::Language, translation::TranslateResult},
    ports::{selection::SelectionPort, translator::TranslatorPort},
};

pub async fn execute(
    selection: &(impl SelectionPort + ?Sized),
    translator: &(impl TranslatorPort + ?Sized),
    target_language: Language,
) -> Result<Option<TranslateResult>> {
    let Some(selection) = selection.selected_text()? else {
        return Ok(None);
    };
    let request = crate::domain::translation::TranslateRequest {
        text: selection.text,
        source_language: None,
        target_language,
    };
    translator.translate(request).await.map(Some)
}
