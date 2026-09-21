use std::time::Duration;

use lexift_core::{
    domain::translation::{TranslateRequest, TranslateResult},
    ports::translator::{TranslationFuture, TranslatorPort},
};

pub(crate) struct MockTranslator;

impl TranslatorPort for MockTranslator {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if request.text == "Hello world" {
                Ok(TranslateResult {
                    text: "你好，世界".into(),
                    detected_source_language: Some(lexift_core::domain::language::Language(
                        "en-US".into(),
                    )),
                })
            } else {
                Err(lexift_core::Error::new(
                    "mock translator received unexpected text",
                ))
            }
        })
    }
}
