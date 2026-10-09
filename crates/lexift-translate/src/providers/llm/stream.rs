//! Incremental framing and completion validation for supported model protocols.
use super::*;
use lexift_core::ports::translator::TranslationProgress;

#[derive(Clone, Copy)]
enum Kind {
    Ollama,
    Chat,
    Responses,
    Native,
}

struct Decoder {
    kind: Kind,
    buffer: Vec<u8>,
    data: String,
    text: String,
    stopped: bool,
    done: bool,
    progress: TranslationProgress,
}

fn invalid() -> Error {
    Error::new("Model returned an incomplete translation")
}

impl Decoder {
    fn new(connection: &LlmConnection, progress: TranslationProgress) -> Self {
        let kind = match (connection.protocol, connection.provider) {
            (LlmProtocol::Responses, _) => Kind::Responses,
            (LlmProtocol::ChatCompletions, _) => Kind::Chat,
            (_, LlmProvider::Ollama) => Kind::Ollama,
            (_, LlmProvider::LmStudio) => Kind::Native,
            _ => Kind::Chat,
        };
        Self {
            kind,
            buffer: Vec::new(),
            data: String::new(),
            text: String::new(),
            stopped: false,
            done: false,
            progress,
        }
    }
    fn append(&mut self, text: &str) -> Result<()> {
        if self.stopped && !text.is_empty() {
            return Err(invalid());
        }
        if !text.is_empty() {
            self.text.push_str(text);
            (self.progress)(text.to_owned());
        }
        Ok(())
    }
    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=end).collect();
            self.line(&line[..line.len() - 1])?;
        }
        // Bound incomplete framing without truncating a successfully parsed body.
        if self.buffer.len() > 8 * 1024 * 1024 || self.data.len() > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        Ok(())
    }
    fn line(&mut self, bytes: &[u8]) -> Result<()> {
        let line = std::str::from_utf8(bytes)
            .map_err(|_| invalid())?
            .trim_end_matches('\r');
        if matches!(self.kind, Kind::Ollama) {
            if !line.is_empty() {
                self.event(line)?;
            }
        } else if line.is_empty() {
            if !self.data.is_empty() {
                let data = std::mem::take(&mut self.data);
                self.event(data.trim_end_matches('\n'))?;
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            self.data.push('\n');
        } // SSE event names, comments and IDs are not translation content.
        Ok(())
    }
    fn event(&mut self, data: &str) -> Result<()> {
        if data == "[DONE]" {
            if !matches!(self.kind, Kind::Chat) || !self.stopped {
                return Err(invalid());
            }
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_str(data).map_err(|_| invalid())?;
        if !value["error"].is_null() {
            return Err(invalid());
        }
        if self.done {
            return Err(invalid());
        }
        match self.kind {
            Kind::Ollama => {
                if remote(&value)
                    || value["message"]["tool_calls"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                {
                    return Err(invalid());
                }
                self.append(value["message"]["content"].as_str().unwrap_or(""))?;
                if value["done"] == true {
                    if value["done_reason"] != "stop" {
                        return Err(invalid());
                    }
                    self.stopped = true;
                    self.done = true;
                }
            }
            Kind::Chat => {
                let choices = value["choices"].as_array().ok_or_else(invalid)?;
                for choice in choices {
                    if choice["index"] != 0 {
                        return Err(invalid());
                    }
                    if !choice["delta"]["tool_calls"].is_null()
                        || !choice["delta"]["function_call"].is_null()
                        || !choice["delta"]["refusal"].is_null()
                    {
                        return Err(invalid());
                    }
                    self.append(choice["delta"]["content"].as_str().unwrap_or(""))?;
                    if !choice["finish_reason"].is_null() {
                        if choice["finish_reason"] != "stop" {
                            return Err(invalid());
                        }
                        self.stopped = true;
                    }
                }
            }
            Kind::Responses => match value["type"].as_str().ok_or_else(invalid)? {
                "response.output_text.delta" => {
                    self.append(value["delta"].as_str().ok_or_else(invalid)?)?
                }
                "response.completed" => {
                    let final_body = response_text(value["response"].clone())?;
                    if self.text != final_body {
                        return Err(invalid());
                    }
                    self.stopped = true;
                    self.done = true;
                }
                "response.failed"
                | "response.incomplete"
                | "error"
                | "response.refusal.delta"
                | "response.function_call_arguments.delta" => return Err(invalid()),
                _ => {}
            },
            Kind::Native => match value["type"].as_str().ok_or_else(invalid)? {
                "message.delta" => self.append(value["content"].as_str().ok_or_else(invalid)?)?,
                "chat.end" => {
                    let final_body = final_text(LlmProvider::LmStudio, value["result"].clone())?;
                    if self.text != final_body {
                        return Err(invalid());
                    }
                    self.stopped = true;
                    self.done = true;
                }
                "error"
                | "tool_call.start"
                | "tool_call.arguments"
                | "tool_call.success"
                | "tool_call.failure" => return Err(invalid()),
                _ => {}
            },
        }
        Ok(())
    }
    fn finish(mut self) -> Result<String> {
        if !self.buffer.is_empty() {
            let tail = std::mem::take(&mut self.buffer);
            self.line(&tail)?;
        }
        if !self.data.is_empty() {
            let data = std::mem::take(&mut self.data);
            self.event(data.trim_end_matches('\n'))?;
        }
        if !self.done || self.text.trim().is_empty() {
            return Err(invalid());
        }
        Ok(self.text)
    }
}

pub(super) async fn read(
    mut response: Response,
    connection: &LlmConnection,
    progress: TranslationProgress,
) -> Result<String> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if (connection.protocol != LlmProtocol::Native || connection.provider != LlmProvider::Ollama)
        && !content_type.starts_with("text/event-stream")
    {
        return Err(Error::new("Invalid model service response"));
    }
    let mut decoder = Decoder::new(connection, progress);
    while let Some(bytes) = response.chunk().await.map_err(network_error)? {
        decoder.push(&bytes)?;
    }
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    fn fixture(kind: Kind) -> String {
        match kind {
            Kind::Ollama => concat!("{\"message\":{\"thinking\":\"private\",\"content\":\"你好\\n\"},\"done\":false}\n", "{\"message\":{\"content\":\"世界\"},\"done\":true,\"done_reason\":\"stop\"}\n").into(),
            Kind::Chat => concat!("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好\\n\",\"reasoning_content\":\"private\"},\"finish_reason\":null}]}\r\n\r\n", "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"世界\"},\"finish_reason\":\"stop\"}]}\n\n", "data: [DONE]\n\n").into(),
            Kind::Native => concat!("event: reasoning.delta\ndata: {\"type\":\"reasoning.delta\",\"content\":\"private\"}\n\n", "event: message.delta\ndata: {\"type\":\"message.delta\",\"content\":\"你好\\n世界\"}\n\n", "event: chat.end\ndata: {\"type\":\"chat.end\",\"result\":{\"output\":[{\"type\":\"message\",\"content\":\"你好\\n世界\"}]}}\n\n").into(),
            Kind::Responses => concat!("data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"private\"}\n\n", "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\\n世界\"}\n\n", "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好\\n世界\"}]}]}}\n\n").into(),
        }
    }
    fn connection(kind: Kind) -> LlmConnection {
        let mut c = LlmConnection::new(if matches!(kind, Kind::Ollama) {
            LlmProvider::Ollama
        } else {
            LlmProvider::LmStudio
        });
        c.protocol = match kind {
            Kind::Chat => LlmProtocol::ChatCompletions,
            Kind::Responses => LlmProtocol::Responses,
            _ => LlmProtocol::Native,
        };
        c
    }
    #[test]
    fn all_protocols_preserve_utf8_whitespace_and_exclude_reasoning() {
        for kind in [Kind::Ollama, Kind::Chat, Kind::Native, Kind::Responses] {
            let body = fixture(kind);
            for width in [1, 3, 17, 4096] {
                let fragments = Arc::new(Mutex::new(String::new()));
                let captured = fragments.clone();
                let mut decoder = Decoder::new(
                    &connection(kind),
                    Arc::new(move |text| captured.lock().unwrap().push_str(&text)),
                );
                for chunk in body.as_bytes().chunks(width) {
                    decoder.push(chunk).unwrap();
                }
                assert_eq!(decoder.finish().unwrap(), "你好\n世界");
                assert_eq!(*fragments.lock().unwrap(), "你好\n世界");
            }
            let mut decoder = Decoder::new(&connection(kind), Arc::new(|_| {}));
            let truncated = body
                .rfind("data:")
                .unwrap_or_else(|| body.rfind("{\"message\"").unwrap());
            decoder.push(&body.as_bytes()[..truncated]).unwrap();
            assert!(
                decoder.finish().is_err(),
                "missing terminal event must fail"
            );
        }
    }
    #[test]
    fn errors_truncation_tool_calls_and_invalid_utf8_never_complete() {
        for body in [
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: {\"error\":{\"message\":\"secret should not escape\"}}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{}]}}]}\n\n",
            "data: [DONE]\n\n",
        ] {
            let mut decoder = Decoder::new(&connection(Kind::Chat), Arc::new(|_| {}));
            assert!(decoder.push(body.as_bytes()).is_err());
        }
        let mut decoder = Decoder::new(&connection(Kind::Ollama), Arc::new(|_| {}));
        assert!(decoder.push(&[0xff, b'\n']).is_err());
    }
    #[test]
    fn streaming_http_reads_all_protocols_without_buffering_complete_json() {
        for kind in [Kind::Ollama, Kind::Chat, Kind::Native, Kind::Responses] {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let content_type = if matches!(kind, Kind::Ollama) {
                    "application/x-ndjson"
                } else {
                    "text/event-stream"
                };
                let (address, requests, server) = crate::providers::test_http::server_with_types(
                    vec![(200, content_type.into(), fixture(kind))],
                );
                let response = reqwest::Client::new().get(address).send().await.unwrap();
                assert_eq!(
                    read(response, &connection(kind), Arc::new(|_| {}))
                        .await
                        .unwrap(),
                    "你好\n世界"
                );
                requests.recv().unwrap();
                server.join().unwrap();
            });
        }
    }

    #[test]
    fn model_switch_controls_http_stream_and_complete_entry_remains_non_streaming() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for streaming in [false, true] {
                    let list = r#"{"models":[{"key":"local","type":"llm"}]}"#;
                    let final_body = r#"{"output":[{"type":"message","content":"你好\n世界"}]}"#;
                    let (address, requests, server) =
                        crate::providers::test_http::server_with_types(vec![
                            (200, "application/json".into(), list.into()),
                            (
                                200,
                                if streaming {
                                    "text/event-stream"
                                } else {
                                    "application/json"
                                }
                                .into(),
                                if streaming {
                                    fixture(Kind::Native)
                                } else {
                                    final_body.into()
                                },
                            ),
                        ]);
                    let mut connection = LlmConnection::new(LlmProvider::LmStudio);
                    connection.address = address;
                    let fragments = Arc::new(Mutex::new(String::new()));
                    let captured = fragments.clone();
                    let translator = LlmService::new().unwrap().translator(
                        connection,
                        None,
                        TranslationModel {
                            provider: LlmProvider::LmStudio,
                            model: "local".into(),
                            streaming,
                            parameters: Default::default(),
                        },
                        DEFAULT_TRANSLATION_PROMPT.into(),
                    );
                    let result = translator
                        .translate_with_progress(
                            TranslateRequest {
                                text: "Hello".into(),
                                source_language: None,
                                target_language: lexift_core::domain::language::Language(
                                    "zh-CN".into(),
                                ),
                            },
                            Arc::new(move |text| captured.lock().unwrap().push_str(&text)),
                        )
                        .await
                        .unwrap();
                    assert_eq!(result.text, "你好\n世界");
                    assert_eq!(
                        *fragments.lock().unwrap(),
                        if streaming { "你好\n世界" } else { "" }
                    );
                    requests.recv().unwrap();
                    let request = requests.recv().unwrap();
                    let body: Value =
                        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                    assert_eq!(body["stream"], streaming);
                    assert_eq!(body["store"], false);
                    server.join().unwrap();
                }
            });
    }

    #[test]
    fn stalled_http_stream_times_out_without_returning_partial_success() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let mut bytes = [0u8; 1024];
                let count = socket.read(&mut bytes).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 9999\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n").unwrap();
            socket.flush().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let response = reqwest::Client::builder()
                    .timeout(Duration::from_millis(100))
                    .build()
                    .unwrap()
                    .get(address)
                    .send()
                    .await
                    .unwrap();
                let result = read(response, &connection(Kind::Chat), Arc::new(|_| {})).await;
                assert_eq!(result.unwrap_err().to_string(), "Model request timed out");
            });
        server.join().unwrap();
    }

    #[test]
    #[ignore = "requires existing LM Studio models and LEXIFT_TEST_LMSTUDIO_ADDRESS"]
    fn configured_lm_studio_streaming_protocol_smoke() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let service = LlmService::new().unwrap();
            for protocol in LlmProtocol::ALL {
                let mut connection = LlmConnection::new(LlmProvider::LmStudio);
                connection.address = std::env::var("LEXIFT_TEST_LMSTUDIO_ADDRESS").unwrap(); connection.protocol = protocol;
                let models = service.models(connection.clone(), None).await.unwrap();
                let model = models.iter().find(|m| m.issue.is_none()).unwrap();
                let fragments = Arc::new(Mutex::new(String::new())); let captured = fragments.clone();
                let text = service.generate_with_progress(connection, LlmAuthorization::default(), GenerateRequest { model: model.id.clone(), system: "Translate into Simplified Chinese. Output only the translation.".into(), input: "Hello, world.".into(), parameters: Default::default() }, Arc::new(move |text| captured.lock().unwrap().push_str(&text))).await.unwrap();
                assert!(!text.trim().is_empty()); assert_eq!(text, *fragments.lock().unwrap());
                println!("LM Studio {} streaming body and final result matched", protocol.id());
            }
        });
    }
}
