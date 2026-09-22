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
    endpoint: String,
}

impl DeepLApiTranslator {
    pub(crate) fn new(client: Client, auth_key: String) -> Self {
        let auth_key = auth_key.trim().to_owned();
        let endpoint = endpoint_for_auth_key(&auth_key).into();
        Self {
            client,
            auth_key,
            endpoint,
        }
    }

    #[cfg(test)]
    fn new_with_endpoint(client: Client, auth_key: String, endpoint: String) -> Self {
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
            let source_lang = request
                .source_language
                .as_ref()
                .map(map_source_language)
                .transpose()?;
            let payload = DeepLTranslateRequest {
                text: vec![request.text],
                source_lang,
                target_lang,
            };
            let response = self
                .client
                .post(&self.endpoint)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    source_lang: Option<&'static str>,
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
            detected_source_language: translation
                .detected_source_language
                .as_deref()
                .map(normalize_detected_language),
        })
    }
}

#[derive(Deserialize)]
struct DeepLTranslation {
    text: String,
    #[serde(rename = "detected_source_language")]
    detected_source_language: Option<String>,
}

fn normalize_detected_language(code: &str) -> Language {
    let code = match code {
        "EN" => "en-US",
        "ZH" => "zh-CN",
        "JA" => "ja",
        "KO" => "ko",
        "DE" => "de",
        "FR" => "fr",
        "ES" => "es",
        "IT" => "it",
        "PT" => "pt-PT",
        other => return Language(other.to_ascii_lowercase()),
    };
    Language(code.into())
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
        "zh-CN" => "ZH-HANS",
        "zh-TW" => "ZH-HANT",
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

fn map_source_language(language: &Language) -> Result<&'static str> {
    let code = match language.0.as_str() {
        "en-US" => "EN",
        "zh-CN" => "ZH",
        "ja" => "JA",
        "ko" => "KO",
        "de" => "DE",
        "fr" => "FR",
        "es" => "ES",
        "it" => "IT",
        "pt-PT" => "PT",
        unsupported => {
            return Err(Error::new(format!(
                "Unsupported source language: {unsupported}"
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
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use lexift_core::{AppCommand, AppEvent, AppState};

    use super::*;

    struct CapturedRequest {
        head: String,
        body: String,
    }

    #[derive(Clone)]
    struct TestResponse {
        status: u16,
        body: String,
        delay: Duration,
    }

    fn spawn_test_server(
        request_count: usize,
        responder: impl Fn(&CapturedRequest) -> TestResponse + Send + Sync + 'static,
    ) -> (
        String,
        mpsc::Receiver<CapturedRequest>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        let address = listener.local_addr().expect("test server address");
        let endpoint = format!("http://{address}/v2/translate");
        let responder = Arc::new(responder);
        let (request_sender, request_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut handlers = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (stream, _) = listener.accept().expect("test server should accept");
                let responder = Arc::clone(&responder);
                let request_sender = request_sender.clone();
                handlers.push(thread::spawn(move || {
                    let (mut stream, request) = read_request(stream);
                    let response = responder(&request);
                    request_sender
                        .send(request)
                        .expect("captured request should be received");
                    thread::sleep(response.delay);
                    write_response(&mut stream, response);
                }));
            }
            for handler in handlers {
                handler.join().expect("test server handler should finish");
            }
        });
        (endpoint, request_receiver, server)
    }

    fn read_request(mut stream: TcpStream) -> (TcpStream, CapturedRequest) {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let (header_end, content_length) = loop {
            let read = stream
                .read(&mut buffer)
                .expect("request should be readable");
            assert!(read > 0, "request ended before its headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("valid content length"))
                    })
                    .expect("request should include content length");
                break (header_end + 4, content_length);
            }
        };
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("body should be readable");
            assert!(read > 0, "request ended before its body");
            bytes.extend_from_slice(&buffer[..read]);
        }

        let head = String::from_utf8(bytes[..header_end - 4].to_vec())
            .expect("request headers should be UTF-8");
        let body = String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
            .expect("request body should be UTF-8");
        (stream, CapturedRequest { head, body })
    }

    fn write_response(stream: &mut TcpStream, response: TestResponse) {
        let head = format!(
            "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            response.body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(response.body.as_bytes());
    }

    fn json_response(status: u16, body: impl Into<String>) -> TestResponse {
        TestResponse {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn translate_through(
        endpoint: String,
        request_timeout: Duration,
        request: TranslateRequest,
    ) -> Result<TranslateResult> {
        let translator = DeepLApiTranslator::new_with_endpoint(
            http::build_test_client(Duration::from_millis(100), request_timeout)
                .expect("test HTTP client should initialize"),
            "test-key".into(),
            endpoint,
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize")
            .block_on(translator.translate(request))
    }

    fn request(text: &str) -> TranslateRequest {
        TranslateRequest {
            text: text.into(),
            source_language: None,
            target_language: Language("zh-CN".into()),
        }
    }

    #[test]
    fn maps_core_languages_to_deepl_codes() {
        for (language, expected) in [
            ("zh-CN", "ZH-HANS"),
            ("zh-TW", "ZH-HANT"),
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
    fn maps_popup_source_languages_to_deepl_codes() {
        for (language, expected) in [
            ("en-US", "EN"),
            ("zh-CN", "ZH"),
            ("ja", "JA"),
            ("ko", "KO"),
            ("de", "DE"),
            ("fr", "FR"),
            ("es", "ES"),
            ("it", "IT"),
            ("pt-PT", "PT"),
        ] {
            assert_eq!(
                map_source_language(&Language(language.into())).expect("supported language"),
                expected
            );
        }
    }

    #[test]
    fn rejects_unknown_source_languages() {
        let error = map_source_language(&Language("zh-TW".into()))
            .expect_err("unsupported source variant should be rejected");
        assert_eq!(error.to_string(), "Unsupported source language: zh-TW");
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

        let result = response.into_result().expect("translation result");
        assert_eq!(result.text, "你好，世界");
        assert_eq!(
            result.detected_source_language,
            Some(Language("en-US".into()))
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
    fn sends_and_parses_a_complete_http_translation() {
        let (endpoint, requests, server) = spawn_test_server(1, |_| {
            json_response(
                200,
                r#"{"translations":[{"detected_source_language":"EN","text":"你好，世界"}]}"#,
            )
        });

        let result = translate_through(endpoint, Duration::from_secs(2), request("Hello world"))
            .expect("local translation should succeed");
        assert_eq!(result.text, "你好，世界");
        assert_eq!(
            result.detected_source_language,
            Some(Language("en-US".into()))
        );

        let captured = requests.recv().expect("request should be captured");
        assert!(captured.head.starts_with("POST /v2/translate HTTP/1.1"));
        let lower_head = captured.head.to_ascii_lowercase();
        assert!(lower_head.contains("content-type: application/json"));
        assert!(lower_head.contains("authorization: deepl-auth-key test-key"));
        let body: serde_json::Value =
            serde_json::from_str(&captured.body).expect("request body should be JSON");
        assert_eq!(body["text"], serde_json::json!(["Hello world"]));
        assert!(body.get("source_lang").is_none());
        assert_eq!(body["target_lang"], "ZH-HANS");
        server.join().expect("test server should finish");
    }

    #[test]
    fn serializes_an_explicit_source_language() {
        let (endpoint, requests, server) = spawn_test_server(1, |_| {
            json_response(200, r#"{"translations":[{"text":"Hello"}]}"#)
        });
        let mut request = request("Hallo");
        request.source_language = Some(Language("de".into()));

        translate_through(endpoint, Duration::from_secs(2), request)
            .expect("explicit source translation should succeed");
        let captured = requests.recv().expect("request should be captured");
        let body: serde_json::Value =
            serde_json::from_str(&captured.body).expect("request body should be JSON");
        assert_eq!(body["source_lang"], "DE");
        server.join().expect("test server should finish");
    }

    #[test]
    fn maps_http_error_responses_through_the_adapter() {
        for (status, expected) in [
            (403, "DeepL authentication failed"),
            (429, "DeepL rate limit exceeded"),
            (456, "DeepL quota exceeded"),
            (500, "DeepL service unavailable"),
            (503, "DeepL service unavailable"),
        ] {
            let (endpoint, _requests, server) =
                spawn_test_server(1, move |_| json_response(status, r#"{"ignored":true}"#));
            let error = translate_through(endpoint, Duration::from_secs(2), request("Hello world"))
                .expect_err("non-success response should fail");
            assert_eq!(error.to_string(), expected);
            server.join().expect("test server should finish");
        }
    }

    #[test]
    fn rejects_malformed_json_through_the_http_adapter() {
        let (endpoint, _requests, server) =
            spawn_test_server(1, |_| json_response(200, "not-json"));
        let error = translate_through(endpoint, Duration::from_secs(2), request("Hello world"))
            .expect_err("malformed JSON should fail");
        assert_eq!(
            error.to_string(),
            "Translation service returned invalid JSON"
        );
        server.join().expect("test server should finish");
    }

    #[test]
    fn rejects_an_empty_response_through_the_http_adapter() {
        let (endpoint, _requests, server) =
            spawn_test_server(1, |_| json_response(200, r#"{"translations":[]}"#));
        let error = translate_through(endpoint, Duration::from_secs(2), request("Hello world"))
            .expect_err("empty translations should fail");
        assert_eq!(error.to_string(), "DeepL returned no translations");
        server.join().expect("test server should finish");
    }

    #[test]
    fn maps_request_timeout_through_the_http_adapter() {
        let (endpoint, _requests, server) = spawn_test_server(1, |_| TestResponse {
            status: 200,
            body: r#"{"translations":[{"text":"late"}]}"#.into(),
            delay: Duration::from_millis(150),
        });
        let error = translate_through(endpoint, Duration::from_millis(40), request("Hello world"))
            .expect_err("delayed response should time out");
        assert_eq!(error.to_string(), "Translation request timed out");
        server.join().expect("test server should finish");
    }

    #[test]
    fn maps_connection_failure_through_the_http_adapter() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("temporary listener should bind");
        let endpoint = format!(
            "http://{}/v2/translate",
            listener.local_addr().expect("temporary address")
        );
        drop(listener);

        let error = translate_through(endpoint, Duration::from_secs(1), request("Hello world"))
            .expect_err("closed endpoint should reject the connection");
        assert_eq!(
            error.to_string(),
            "Could not connect to the translation service"
        );
    }

    #[test]
    fn stale_real_http_result_does_not_replace_the_newer_result() {
        let (endpoint, _requests, server) = spawn_test_server(2, |captured| {
            let body: serde_json::Value =
                serde_json::from_str(&captured.body).expect("request body should be JSON");
            let source = body["text"][0].as_str().expect("source text");
            TestResponse {
                status: 200,
                body: format!(r#"{{"translations":[{{"text":"{source}"}}]}}"#),
                delay: if source == "old request" {
                    Duration::from_millis(150)
                } else {
                    Duration::from_millis(10)
                },
            }
        });
        let translator = Arc::new(DeepLApiTranslator::new_with_endpoint(
            http::build_test_client(Duration::from_secs(2), Duration::from_secs(2))
                .expect("test HTTP client should initialize"),
            "test-key".into(),
            endpoint,
        ));
        let mut state = AppState::default();
        let (old_task, old_request) =
            translation_command(state.reduce(AppEvent::InputTranslationRequested {
                text: "old request".into(),
            }));
        let (new_task, new_request) =
            translation_command(state.reduce(AppEvent::InputTranslationRequested {
                text: "new request".into(),
            }));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime should initialize");
        let (new_result, old_result) = runtime.block_on(async {
            let old_translator = Arc::clone(&translator);
            let old = tokio::spawn(async move { old_translator.translate(old_request).await });
            let new = tokio::spawn(async move { translator.translate(new_request).await });
            (
                new.await.expect("new task should finish"),
                old.await.expect("old task should finish"),
            )
        });
        state.reduce(AppEvent::TranslationFinished {
            task_id: new_task,
            result: new_result.expect("new translation should succeed"),
        });
        state.reduce(AppEvent::TranslationFinished {
            task_id: old_task,
            result: old_result.expect("old translation should succeed"),
        });

        assert_eq!(state.translated_text, "new request");
        server.join().expect("test server should finish");
    }

    fn translation_command(
        commands: Vec<AppCommand>,
    ) -> (lexift_core::TranslationTaskId, TranslateRequest) {
        commands
            .into_iter()
            .find_map(|command| match command {
                AppCommand::Translate { task_id, request } => Some((task_id, request)),
                _ => None,
            })
            .expect("input translation should emit a translate command")
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
                source_language: None,
                target_language: Language("zh-CN".into()),
            }))
            .expect("DeepL should translate with a valid credential");

        assert!(!result.text.trim().is_empty());
    }

    #[test]
    #[ignore = "accesses the DeepL API"]
    fn rejects_an_invalid_deepl_credential() {
        let translator = DeepLApiTranslator::new(
            http::build_client().expect("shared HTTP client should initialize"),
            "invalid-test-key:fx".into(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("manual test runtime should initialize");
        let error = runtime
            .block_on(translator.translate(request("Hello world")))
            .expect_err("DeepL should reject an invalid credential");

        assert_eq!(error.to_string(), "DeepL authentication failed");
    }
}
