use super::*;
use crate::providers::test_http::{ok, server};
use lexift_core::domain::language::Language;
use std::net::TcpListener;

fn request() -> TranslateRequest {
    TranslateRequest {
        text: "Hello\nworld".into(),
        source_language: Some(Language("en-US".into())),
        target_language: Language("zh-CN".into()),
    }
}

#[test]
fn discovery_never_loads_models_and_reports_incompatible_entries() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {

    let (address, requests, task) = server(vec![
        ok(r#"{"models":[{"name":"local"},{"name":"embed"},{"name":"cloud","remote_model":"upstream"},{"name":"unknown"}]}"#),
        ok(r#"{"capabilities":["completion"]}"#),
        ok(r#"{"capabilities":["embedding"]}"#),
        ok(r#"{}"#),
    ]);
    let models = OllamaService::new().unwrap().models(address).await.unwrap();
    assert_eq!(models.iter().filter(|m| m.issue.is_none()).map(|m| m.name.as_str()).collect::<Vec<_>>(), ["local"]);
    assert_eq!(models[0].issue, Some(OllamaModelIssue::Remote));
    assert_eq!(models[1].issue, Some(OllamaModelIssue::EmbeddingOnly));
    assert_eq!(models[3].issue, Some(OllamaModelIssue::UnknownCapabilities));
    task.join().unwrap();
    let captured: Vec<_> = requests.try_iter().collect();
    assert!(captured[0].starts_with("GET /api/tags "));
    for request in &captured[1..] { assert!(request.starts_with("POST /api/show ")); }
    assert!(captured.iter().all(|r| !r.contains("Hello") && !r.contains("/api/chat") && !r.contains("/api/pull")));
    });
}

#[test]
fn chat_uses_only_final_content_and_independent_messages() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {

    let (address, requests, task) = server(vec![ok(r#"{"capabilities":["completion","thinking"]}"#), ok(r#"{"message":{"content":"你好\n世界","thinking":"private reasoning"},"done":true,"done_reason":"stop"}"#)]);
    let translator = OllamaService::new().unwrap().translator(OllamaConfig { address, model: "local:small".into() });
    assert_eq!(translator.translate(request()).await.unwrap().text, "你好\n世界");
    task.join().unwrap();
    let captured: Vec<_> = requests.try_iter().collect();
    let payload: serde_json::Value = serde_json::from_str(captured[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["model"], "local:small");
    assert_eq!(payload["stream"], false);
    assert_eq!(payload["messages"].as_array().unwrap().len(), 2);
    assert_eq!(payload["messages"][1]["content"], "Hello\nworld");
    let prompt = payload["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("zh-CN") && prompt.contains("en-US") && prompt.contains("formatting"));
    for forbidden in ["keep_alive", "options", "template", "raw", "think"] { assert!(payload.get(forbidden).is_none()); }
    });
}

#[test]
fn cloud_or_embedding_model_never_receives_user_text() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for details in [
                r#"{"capabilities":["completion"],"remote_host":"https://ollama.com"}"#,
                r#"{"capabilities":["embedding"]}"#,
                r#"{}"#,
            ] {
                let (address, requests, task) = server(vec![ok(details)]);
                let translator = OllamaService::new().unwrap().translator(OllamaConfig {
                    address,
                    model: "test".into(),
                });
                assert!(translator.translate(request()).await.is_err());
                task.join().unwrap();
                assert_eq!(requests.try_iter().count(), 1);
            }
        });
}

#[test]
fn incomplete_empty_and_tool_results_are_failures() {
    for body in [
        r#"{"message":{"content":"partial"},"done":true,"done_reason":"length"}"#,
        r#"{"message":{"content":"partial"},"done":false,"done_reason":"stop"}"#,
        r#"{"message":{"content":" "},"done":true,"done_reason":"stop"}"#,
        r#"{"message":{"content":"translation","tool_calls":[{}]},"done":true,"done_reason":"stop"}"#,
        r#"{"message":{"content":"translation"},"done":true,"done_reason":"stop","remote_model":"remote"}"#,
    ] {
        assert!(
            serde_json::from_str::<ChatResponse>(body)
                .unwrap()
                .into_result()
                .is_err()
        );
    }
}

#[test]
fn redirects_errors_and_malformed_responses_are_safe_and_not_retried() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for (status, body, message) in [
                (302, "secret", "Ollama redirects are not allowed"),
                (404, "secret", "Ollama model or API was not found"),
                (500, "secret", "Ollama service rejected the request"),
                (200, "secret", "Invalid Ollama response"),
            ] {
                let (address, requests, task) = server(vec![(status, body.into())]);
                let result = OllamaService::new()
                    .unwrap()
                    .models(address)
                    .await
                    .unwrap_err();
                assert_eq!(result.to_string(), message);
                task.join().unwrap();
                assert_eq!(requests.try_iter().count(), 1);
            }
        });
}

#[test]
fn unconfigured_offline_and_nonlocal_are_distinct() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let service = OllamaService::new().unwrap();
            assert_eq!(
                service
                    .translator(OllamaConfig::default())
                    .translate(request())
                    .await
                    .unwrap_err()
                    .to_string(),
                "Select an Ollama model in settings first"
            );
            assert!(
                service
                    .models("http://example.org:11434".into())
                    .await
                    .is_err()
            );
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            drop(listener);
            assert_eq!(
                service.models(address).await.unwrap_err().to_string(),
                "Cannot connect to Ollama; start the local service"
            );
        });
}

#[test]
fn system_proxy_environment_is_bypassed() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "providers::ollama::tests::proxy_environment_is_ignored_child",
        ])
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
#[ignore = "isolated process with proxy environment set by parent test"]
fn proxy_environment_is_ignored_child() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (address, _requests, task) = server(vec![ok(r#"{"models":[]}"#)]);
            assert!(
                OllamaService::new()
                    .unwrap()
                    .models(address)
                    .await
                    .unwrap()
                    .is_empty()
            );
            task.join().unwrap();
        });
}

#[test]
#[ignore = "explicit opt-in; uses an existing local model without installing or downloading"]
fn installed_local_model_smoke() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let model = std::env::var("LEXIFT_TEST_OLLAMA_MODEL")
                .expect("choose an existing model explicitly");
            let service = OllamaService::new().unwrap();
            let models = service
                .models(OllamaConfig::default().address)
                .await
                .unwrap();
            assert!(models.iter().any(|m| m.name == model && m.issue.is_none()));
            let translator = service.translator(OllamaConfig {
                model,
                ..Default::default()
            });
            for request in [
                request(),
                TranslateRequest {
                    text: "第一行：你好。\n第二行：谢谢。".into(),
                    source_language: Some(Language("zh-CN".into())),
                    target_language: Language("en-US".into()),
                },
            ] {
                let translated = translator.translate(request).await.unwrap();
                assert!(!translated.text.trim().is_empty());
                assert!(
                    translated.text.contains('\n'),
                    "multiline formatting must be preserved"
                );
            }
        });
}
