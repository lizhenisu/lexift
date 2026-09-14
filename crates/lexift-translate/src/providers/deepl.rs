//! Official DeepL API translation adapter.

use lexift_core::{
    Error, Result,
    domain::{
        language::Language,
        translation::{TranslateRequest, TranslateResult},
    },
    ports::translator::{TranslationFuture, TranslatorPort},
};
use reqwest::{Client, StatusCode, header::AUTHORIZATION};
use serde::{Deserialize, Serialize};

use crate::http;

const FREE_ENDPOINT: &str = "https://api-free.deepl.com/v2/translate";
const PRO_ENDPOINT: &str = "https://api.deepl.com/v2/translate";

/// Translates text through DeepL's supported official API endpoints.
pub(crate) struct DeepLApiTranslator {
    client: Client,
    auth_key: String,
    endpoint: &'static str,
}

impl DeepLApiTranslator {
    pub(crate) fn new(client: Client, auth_key: String) -> Self {
        let auth_key = auth_key.trim().to_owned();
        let endpoint = endpoint_for_auth_key(&auth_key);
        Self {
            client,
            auth_key,
            endpoint,
        }
    }
}

impl TranslatorPort for DeepLApiTranslator {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
        Box::pin(async move {
            let target_lang = map_target_language(&request.target_language)?;
            let payload = DeepLTranslateRequest {
                text: vec![request.text],
                target_lang,
            };
            let response = self
                .client
                .post(self.endpoint)
                .header(AUTHORIZATION, format!("DeepL-Auth-Key {}", self.auth_key))
                .json(&payload)
                .send()
                .await
                .map_err(http::map_error)?;

            let status = response.status();
            if !status.is_success() {
                return Err(map_status(status));
            }

            let response = response
                .json::<DeepLResponse>()
                .await
                .map_err(http::map_error)?;
            response.into_result()
        })
    }
}

#[derive(Serialize)]
struct DeepLTranslateRequest {
    text: Vec<String>,
    target_lang: &'static str,
}

#[derive(Deserialize)]
struct DeepLResponse {
    translations: Vec<DeepLTranslation>,
}

impl DeepLResponse {
    fn into_result(self) -> Result<TranslateResult> {
        let translation = self
            .translations
            .into_iter()
            .next()
            .ok_or_else(|| Error::new("DeepL returned no translations"))?;
        Ok(TranslateResult {
            text: translation.text,
        })
    }
}

#[derive(Deserialize)]
struct DeepLTranslation {
    text: String,
    #[serde(rename = "detected_source_language")]
    _detected_source_language: Option<String>,
}

fn endpoint_for_auth_key(auth_key: &str) -> &'static str {
    if auth_key.ends_with(":fx") {
        FREE_ENDPOINT
    } else {
        PRO_ENDPOINT
    }
}

fn map_target_language(language: &Language) -> Result<&'static str> {
    let code = match language.0.as_str() {
        "zh-CN" | "zh-TW" => "ZH",
        "ja" => "JA",
        "ko" => "KO",
        "de" => "DE",
        "fr" => "FR",
        "es" => "ES",
        "it" => "IT",
        "en" => "EN",
        "en-US" => "EN-US",
        "en-GB" => "EN-GB",
        "pt-PT" => "PT-PT",
        "pt-BR" => "PT-BR",
        unsupported => {
            return Err(Error::new(format!(
                "Unsupported target language: {unsupported}"
            )));
        }
    };
    Ok(code)
}

fn map_status(status: StatusCode) -> Error {
    let message = match status.as_u16() {
        400 => "Invalid DeepL translation request",
        403 => "DeepL authentication failed",
        413 => "Translation request too large",
        429 => "DeepL rate limit exceeded",
        456 => "DeepL quota exceeded",
        500..=599 => "DeepL service unavailable",
        _ => "DeepL request failed",
    };
    Error::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_core_languages_to_deepl_codes() {
        for (language, expected) in [
            ("zh-CN", "ZH"),
            ("zh-TW", "ZH"),
            ("ja", "JA"),
            ("de", "DE"),
            ("en-US", "EN-US"),
        ] {
            assert_eq!(
                map_target_language(&Language(language.into())).expect("supported language"),
                expected
            );
        }
    }

    #[test]
    fn rejects_unknown_target_languages() {
        let error = map_target_language(&Language("xx-INVALID".into()))
            .expect_err("unknown language should be rejected");
        assert_eq!(error.to_string(), "Unsupported target language: xx-INVALID");
    }

    #[test]
    fn selects_free_or_pro_endpoint_from_key_suffix() {
        assert_eq!(endpoint_for_auth_key("some-key:fx"), FREE_ENDPOINT);
        assert_eq!(endpoint_for_auth_key("some-key"), PRO_ENDPOINT);
    }

    #[test]
    fn parses_a_translation_response() {
        let response: DeepLResponse = serde_json::from_str(
            r#"{"translations":[{"detected_source_language":"EN","text":"你好，世界"}]}"#,
        )
        .expect("valid DeepL response");

        assert_eq!(
            response.into_result().expect("translation result").text,
            "你好，世界"
        );
    }

    #[test]
    fn rejects_an_empty_translation_response_without_panicking() {
        let response: DeepLResponse =
            serde_json::from_str(r#"{"translations":[]}"#).expect("valid empty DeepL response");

        assert_eq!(
            response
                .into_result()
                .expect_err("empty response should fail")
                .to_string(),
            "DeepL returned no translations"
        );
    }

    #[test]
    fn maps_deepl_http_statuses() {
        for (status, expected) in [
            (400, "Invalid DeepL translation request"),
            (403, "DeepL authentication failed"),
            (413, "Translation request too large"),
            (429, "DeepL rate limit exceeded"),
            (456, "DeepL quota exceeded"),
            (503, "DeepL service unavailable"),
            (418, "DeepL request failed"),
        ] {
            assert_eq!(
                map_status(StatusCode::from_u16(status).expect("valid status")).to_string(),
                expected
            );
        }
    }

    #[test]
    #[ignore = "requires LEXIFT_DEEPL_AUTH_KEY and accesses the DeepL API"]
    fn translates_with_configured_deepl_api() {
        let auth_key = std::env::var("LEXIFT_DEEPL_AUTH_KEY")
            .expect("LEXIFT_DEEPL_AUTH_KEY must be set for the manual API test");
        let translator = DeepLApiTranslator::new(
            http::build_client().expect("shared HTTP client should initialize"),
            auth_key,
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("manual test runtime should initialize");
        let result = runtime
            .block_on(translator.translate(TranslateRequest {
                text: "Hello world".into(),
                target_language: Language("zh-CN".into()),
            }))
            .expect("DeepL should translate with a valid credential");

        assert!(!result.text.trim().is_empty());
    }
}
