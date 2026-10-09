//! Stateless model protocol adapters and the separate translation policy adapter.
mod stream;
use lexift_core::{
    Error, Result,
    domain::{
        llm::*,
        translation::{TranslateRequest, TranslateResult},
    },
    ports::{
        llm::*,
        translator::{TranslationFuture, TranslatorPort},
    },
};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct LlmService {
    local: Client,
    cloud: Client,
}
impl LlmService {
    pub fn new() -> Result<Self> {
        let build = |local| {
            let mut builder = Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never());
            if local {
                builder = builder.no_proxy();
            }
            builder
                .build()
                .map_err(|_| Error::new("Model client initialization failed"))
        };
        Ok(Self {
            local: build(true)?,
            cloud: build(false)?,
        })
    }
    async fn request_response(
        &self,
        connection: &LlmConnection,
        auth: &LlmAuthorization,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Response> {
        connection.validate()?;
        if connection.provider == LlmProvider::DeepSeek
            && connection.profile().authentication == Authentication::Automatic
            && auth.key.as_deref().is_none_or(|s| s.trim().is_empty())
        {
            return Err(Error::new("DeepSeek API key is not configured"));
        }
        let client = if connection.provider == LlmProvider::DeepSeek {
            &self.cloud
        } else {
            &self.local
        };
        let url = format!(
            "{}{path}",
            api_base_address(&connection.profile().base_url)?
        );
        let mut request = if let Some(body) = body {
            client.post(url).json(&body)
        } else {
            client.get(url)
        };
        let profile = connection.profile();
        if profile.authentication == Authentication::Automatic
            && let Some(secret) = &auth.key
        {
            request = request.bearer_auth(secret);
        }
        for (name, value) in &auth.headers {
            validate_header_name(name)?;
            if value.contains(['\r', '\n']) {
                return Err(Error::new("Invalid request header value"));
            }
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::new("Invalid request header"))?;
            if (name == reqwest::header::AUTHORIZATION || name == "x-api-key")
                && profile.authentication != Authentication::Headers
            {
                return Err(Error::new("Select custom-header authentication first"));
            }
            let mut value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| Error::new("Invalid request header value"))?;
            value.set_sensitive(true);
            request = request.header(name, value);
        }
        let response = request
            .timeout(timeout)
            .send()
            .await
            .map_err(network_error)?;
        check_status(&response)?;
        Ok(response)
    }
    async fn request(
        &self,
        connection: &LlmConnection,
        auth: &LlmAuthorization,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        decode(
            self.request_response(connection, auth, path, body, timeout)
                .await?,
        )
        .await
    }
    pub fn translator(
        &self,
        connection: LlmConnection,
        credential: Option<String>,
        model: TranslationModel,
        prompt: String,
    ) -> Arc<dyn TranslatorPort> {
        self.translator_authenticated(
            connection,
            LlmAuthorization {
                key: credential,
                headers: Vec::new(),
            },
            model,
            prompt,
        )
    }
    pub fn translator_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        model: TranslationModel,
        prompt: String,
    ) -> Arc<dyn TranslatorPort> {
        Arc::new(LlmTranslator {
            service: self.clone(),
            connection,
            auth,
            model,
            prompt,
        })
    }
}
fn network_error(e: reqwest::Error) -> Error {
    Error::new(if e.is_timeout() {
        "Model request timed out"
    } else if e.is_connect() {
        "Cannot connect to model service"
    } else {
        "Invalid model service response"
    })
}
fn check_status(response: &Response) -> Result<()> {
    if !response.status().is_success() {
        return Err(Error::new(match response.status().as_u16() {
            401 | 403 => "Model service authentication failed",
            404 => "Model or API not found; check service version",
            429 => "Model service rate limit exceeded",
            300..=399 => "Model service redirects are not allowed",
            _ => "Model service rejected the request",
        }));
    }
    Ok(())
}
async fn decode(response: Response) -> Result<Value> {
    check_status(&response)?;
    response.json().await.map_err(network_error)
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
fn reasoning(provider: LlmProvider, value: &Value) -> Vec<String> {
    match provider {
        LlmProvider::LmStudio => strings(&value["capabilities"]["reasoning"]["allowed_options"]),
        LlmProvider::DeepSeek => {
            let mut values = strings(&value["effort"]["supported_levels"]);
            values.insert(0, "off".into());
            values
        }
        LlmProvider::Ollama => value["thinking"]["values"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| match value {
                        Value::Bool(false) => Some("off".into()),
                        Value::Bool(true) => Some("on".into()),
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}
impl LlmServicePort for LlmService {
    fn check_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
    ) -> ModelsFuture<'_> {
        Box::pin(async move {
            let path = if connection.protocol != LlmProtocol::Native {
                "/models"
            } else {
                match connection.provider {
                    LlmProvider::Ollama => "/api/tags",
                    LlmProvider::LmStudio => "/api/v1/models",
                    LlmProvider::DeepSeek => "/models",
                }
            };
            self.request(&connection, &auth, path, None, Duration::from_secs(10))
                .await?;
            Ok(Vec::new())
        })
    }

    fn models(&self, connection: LlmConnection, credential: Option<String>) -> ModelsFuture<'_> {
        self.models_authenticated(
            connection,
            LlmAuthorization {
                key: credential,
                headers: Vec::new(),
            },
        )
    }
    fn models_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
    ) -> ModelsFuture<'_> {
        Box::pin(async move {
            let work = async {
                let path = if connection.protocol != LlmProtocol::Native {
                    "/models"
                } else {
                    match connection.provider {
                        LlmProvider::Ollama => "/api/tags",
                        LlmProvider::LmStudio => "/api/v1/models",
                        LlmProvider::DeepSeek => "/models",
                    }
                };
                let value = self
                    .request(&connection, &auth, path, None, Duration::from_secs(10))
                    .await?;
                let list = value[if connection.provider == LlmProvider::DeepSeek
                    || connection.protocol != LlmProtocol::Native
                {
                    "data"
                } else {
                    "models"
                }]
                .as_array()
                .ok_or_else(|| Error::new("Invalid model list"))?;
                if list.len() > 256 {
                    return Err(Error::new("Model service returned too many models"));
                }
                let mut models = Vec::new();
                for entry in list {
                    let id = entry[if connection.protocol != LlmProtocol::Native {
                        "id"
                    } else {
                        match connection.provider {
                            LlmProvider::Ollama => "name",
                            LlmProvider::LmStudio => "key",
                            LlmProvider::DeepSeek => "id",
                        }
                    }]
                    .as_str()
                    .ok_or_else(|| Error::new("Invalid model identifier"))?
                    .to_owned();
                    let details = if connection.protocol == LlmProtocol::Native
                        && connection.provider == LlmProvider::Ollama
                        && !remote(entry)
                    {
                        self.request(
                            &connection,
                            &auth,
                            "/api/show",
                            Some(json!({"model": id})),
                            Duration::from_secs(10),
                        )
                        .await?
                    } else {
                        entry.clone()
                    };
                    let issue = if connection.protocol != LlmProtocol::Native {
                        if entry["type"] == "embedding"
                            || entry["capabilities"]["embedding"] == true
                        {
                            Some("Embedding-only models cannot translate text".into())
                        } else if connection.provider == LlmProvider::DeepSeek
                            || entry["type"] == "llm"
                            || entry["capabilities"]["text_generation"] == true
                        {
                            None
                        } else {
                            Some("Model capability is unknown; verify using the native model catalog".into())
                        }
                    } else {
                        match connection.provider {
                            LlmProvider::Ollama if remote(entry) || remote(&details) => {
                                Some("Cloud Ollama models are not supported".into())
                            }
                            LlmProvider::Ollama
                                if !strings(&details["capabilities"])
                                    .iter()
                                    .any(|v| v == "completion") =>
                            {
                                Some("Model does not report text generation capability".into())
                            }
                            LlmProvider::LmStudio if entry["type"] == "embedding" => {
                                Some("Embedding-only models cannot translate text".into())
                            }
                            LlmProvider::LmStudio if entry["type"] != "llm" => {
                                Some("Model does not report text generation capability".into())
                            }
                            _ => None,
                        }
                    };
                    models.push(LlmModel {
                        id,
                        aliases: entry["loaded_instances"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v["id"].as_str().map(str::to_owned))
                            .collect(),
                        reasoning: if connection.protocol == LlmProtocol::Native
                            || (connection.protocol == LlmProtocol::ChatCompletions
                                && connection.provider == LlmProvider::DeepSeek)
                        {
                            reasoning(connection.provider, &details)
                        } else {
                            Vec::new()
                        },
                        issue,
                    });
                }
                if connection.protocol != LlmProtocol::Native
                    && connection.provider != LlmProvider::DeepSeek
                {
                    let mut native = connection.clone();
                    native.protocol = LlmProtocol::Native;
                    let native_base = native.profile().base_url;
                    let compatible_base = connection.profile().base_url;
                    let same_origin = reqwest::Url::parse(&native_base)
                        .ok()
                        .zip(reqwest::Url::parse(&compatible_base).ok())
                        .is_some_and(|(a, b)| a.origin() == b.origin());
                    let native_auth = if native.profile().authentication
                        == connection.profile().authentication
                        && native.profile().headers == connection.profile().headers
                    {
                        auth.clone()
                    } else {
                        LlmAuthorization {
                            key: auth.key.clone(),
                            headers: Vec::new(),
                        }
                    };
                    if same_origin
                        && let Ok(known) = self.models_authenticated(native, native_auth).await
                    {
                        for model in &mut models {
                            if let Some(details) = known
                                .iter()
                                .find(|m| m.id == model.id || m.aliases.contains(&model.id))
                            {
                                model.issue = details.issue.clone();
                                model.reasoning = Vec::new();
                            }
                        }
                    }
                }
                models.sort_by(|a, b| a.id.cmp(&b.id));
                models.dedup_by(|a, b| a.id == b.id);
                Ok(models)
            };
            tokio::time::timeout(Duration::from_secs(10), work)
                .await
                .map_err(|_| Error::new("Model discovery timed out"))?
        })
    }
    fn generate(
        &self,
        connection: LlmConnection,
        credential: Option<String>,
        request: GenerateRequest,
    ) -> GenerateFuture<'_> {
        self.generate_authenticated(
            connection,
            LlmAuthorization {
                key: credential,
                headers: Vec::new(),
            },
            request,
        )
    }
    fn generate_authenticated(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        request: GenerateRequest,
    ) -> GenerateFuture<'_> {
        self.generate_inner(connection, auth, request, None)
    }
    fn generate_with_progress(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        request: GenerateRequest,
        progress: lexift_core::ports::translator::TranslationProgress,
    ) -> GenerateFuture<'_> {
        self.generate_inner(connection, auth, request, Some(progress))
    }
}

impl LlmService {
    fn generate_inner(
        &self,
        connection: LlmConnection,
        auth: LlmAuthorization,
        request: GenerateRequest,
        progress: Option<lexift_core::ports::translator::TranslationProgress>,
    ) -> GenerateFuture<'_> {
        Box::pin(async move {
            let work = async {
                if !connection.enabled {
                    return Err(Error::new("Provider is disabled"));
                }
                if request.model.is_empty() {
                    return Err(Error::new("Select a model in translation settings first"));
                }
                let models = self
                    .models_authenticated(connection.clone(), auth.clone())
                    .await?;
                let model = models
                    .iter()
                    .find(|m| m.id == request.model)
                    .ok_or_else(|| Error::new("Selected model is unavailable"))?;
                if let Some(issue) = &model.issue {
                    return Err(Error::new(issue));
                }
                let p = &request.parameters;
                if !model.reasoning.is_empty() && !model.reasoning.contains(&p.reasoning) {
                    return Err(Error::new(
                        "Model does not support the selected thinking setting",
                    ));
                }
                if model.reasoning.is_empty() && p.reasoning != "off" {
                    return Err(Error::new("Model thinking capability is unknown"));
                }
                let (path, mut payload) = if connection.protocol == LlmProtocol::Native
                    || (connection.protocol == LlmProtocol::ChatCompletions
                        && connection.provider == LlmProvider::DeepSeek)
                {
                    payload(connection.provider, &request)?
                } else {
                    compatible_payload(connection.protocol, &request)?
                };
                if model.reasoning.is_empty() {
                    payload.as_object_mut().unwrap().remove("think");
                    payload.as_object_mut().unwrap().remove("reasoning");
                }
                if let Some(progress) = progress {
                    payload["stream"] = json!(true);
                    let response = self
                        .request_response(
                            &connection,
                            &auth,
                            path,
                            Some(payload),
                            Duration::from_secs(120),
                        )
                        .await?;
                    return stream::read(response, &connection, progress).await;
                }
                let result = self
                    .request(
                        &connection,
                        &auth,
                        path,
                        Some(payload),
                        Duration::from_secs(120),
                    )
                    .await?;
                if connection.protocol == LlmProtocol::Responses {
                    response_text(result)
                } else if connection.protocol == LlmProtocol::ChatCompletions {
                    final_text(LlmProvider::DeepSeek, result)
                } else {
                    final_text(connection.provider, result)
                }
            };
            tokio::time::timeout(Duration::from_secs(120), work)
                .await
                .map_err(|_| Error::new("Model request timed out"))?
        })
    }
}
fn compatible_payload(protocol: LlmProtocol, r: &GenerateRequest) -> Result<(&'static str, Value)> {
    let mut body = json!({"model":r.model,"stream":false});
    let path = if protocol == LlmProtocol::Responses {
        body["store"] = json!(false);
        body["instructions"] = json!(r.system);
        body["input"] = json!(r.input);
        "/responses"
    } else {
        body["messages"] =
            json!([{"role":"system","content":r.system},{"role":"user","content":r.input}]);
        "/chat/completions"
    };
    if r.parameters.reasoning != "off" {
        return Err(Error::new(
            "Thinking settings require verified protocol capabilities",
        ));
    }
    if let Some(v) = r.parameters.temperature {
        body["temperature"] = json!(f64::from(v) / 100.);
    }
    if let Some(v) = r.parameters.top_p {
        body["top_p"] = json!(f64::from(v) / 100.);
    }
    Ok((path, body))
}
fn response_text(value: Value) -> Result<String> {
    let invalid = || Error::new("Model returned an incomplete translation");
    if value["status"] != "completed" || !value["error"].is_null() {
        return Err(invalid());
    }
    let mut text = String::new();
    for item in value["output"].as_array().ok_or_else(invalid)? {
        match item["type"].as_str() {
            Some("reasoning") => {}
            Some("message") if item["role"] == "assistant" && item["status"] == "completed" => {
                for part in item["content"].as_array().ok_or_else(invalid)? {
                    if part["type"] != "output_text" {
                        return Err(invalid());
                    }
                    text.push_str(part["text"].as_str().ok_or_else(invalid)?);
                }
            }
            _ => return Err(invalid()),
        }
    }
    if text.trim().is_empty() {
        return Err(Error::new("Model returned an empty translation"));
    }
    Ok(text)
}
fn remote(v: &Value) -> bool {
    ["remote_model", "remote_host"]
        .iter()
        .any(|k| v[k].as_str().is_some_and(|s| !s.is_empty()))
}
fn payload(provider: LlmProvider, r: &GenerateRequest) -> Result<(&'static str, Value)> {
    let p = &r.parameters;
    if p.temperature.is_some_and(|v| {
        v > if provider == LlmProvider::LmStudio {
            100
        } else {
            200
        }
    }) || p.top_p.is_some_and(|v| v == 0 || v > 100)
    {
        return Err(Error::new("Invalid sampling parameters"));
    }
    let thinking = p.reasoning != "off";
    if provider == LlmProvider::DeepSeek
        && ((thinking && p.temperature.is_some())
            || (!thinking && p.top_p.is_some())
            || (thinking && p.top_p.is_some_and(|v| v < 95)))
    {
        return Err(Error::new(
            "Sampling parameters are incompatible with the thinking mode",
        ));
    }
    let messages = json!([{"role":"system","content":r.system},{"role":"user","content":r.input}]);
    let mut body = json!({"model": r.model, "stream": false});
    let path = match provider {
        LlmProvider::Ollama => {
            body["messages"] = messages;
            body["think"] = match p.reasoning.as_str() {
                "off" => json!(false),
                "on" => json!(true),
                other => json!(other),
            };
            body["options"] = json!({});
            "/api/chat"
        }
        LlmProvider::LmStudio => {
            body["input"] = json!(r.input);
            body["system_prompt"] = json!(r.system);
            body["store"] = json!(false);
            body["reasoning"] = json!(p.reasoning);
            "/api/v1/chat"
        }
        LlmProvider::DeepSeek => {
            body["messages"] = messages;
            body["reasoning_effort"] = json!(if thinking {
                p.reasoning.as_str()
            } else {
                "none"
            });
            "/chat/completions"
        }
    };
    let sampling = if provider == LlmProvider::Ollama {
        &mut body["options"]
    } else {
        &mut body
    };
    if let Some(v) = p.temperature {
        sampling["temperature"] = json!(f64::from(v) / 100.);
    }
    if let Some(v) = p.top_p {
        sampling["top_p"] = json!(f64::from(v) / 100.);
    }
    Ok((path, body))
}
fn final_text(provider: LlmProvider, result: Value) -> Result<String> {
    let incomplete = || Error::new("Model returned an incomplete translation");
    let text = match provider {
        LlmProvider::Ollama => {
            if remote(&result) || result["done"] != true || result["done_reason"] != "stop" {
                return Err(incomplete());
            }
            if result["message"]["tool_calls"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
            {
                return Err(incomplete());
            }
            result["message"]["content"].as_str().map(str::to_owned)
        }
        LlmProvider::DeepSeek => {
            let choice = &result["choices"][0];
            if choice["finish_reason"] != "stop"
                || choice["message"]["tool_calls"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
            {
                return Err(incomplete());
            }
            choice["message"]["content"].as_str().map(str::to_owned)
        }
        LlmProvider::LmStudio => {
            let items = result["output"].as_array().ok_or_else(incomplete)?;
            if items
                .iter()
                .any(|i| i["type"] != "message" && i["type"] != "reasoning")
                || result["stats"]["stop_reason"]
                    .as_str()
                    .is_some_and(|v| !matches!(v, "eosFound" | "stopStringFound" | "stop"))
            {
                return Err(incomplete());
            }
            Some(
                items
                    .iter()
                    .filter(|i| i["type"] == "message")
                    .filter_map(|i| i["content"].as_str())
                    .collect::<Vec<_>>()
                    .join(""),
            )
        }
    }
    .ok_or_else(incomplete)?;
    if text.trim().is_empty() {
        return Err(Error::new("Model returned an empty translation"));
    }
    Ok(text)
}

struct LlmTranslator {
    service: LlmService,
    connection: LlmConnection,
    auth: LlmAuthorization,
    model: TranslationModel,
    prompt: String,
}
impl TranslatorPort for LlmTranslator {
    fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
        self.translate_inner(request, None)
    }
    fn translate_with_progress(
        &self,
        request: TranslateRequest,
        progress: lexift_core::ports::translator::TranslationProgress,
    ) -> TranslationFuture<'_> {
        self.translate_inner(request, self.model.streaming.then_some(progress))
    }
}
impl LlmTranslator {
    fn translate_inner(
        &self,
        request: TranslateRequest,
        progress: Option<lexift_core::ports::translator::TranslationProgress>,
    ) -> TranslationFuture<'_> {
        Box::pin(async move {
            if !self.prompt.contains("{target_language}") {
                return Err(Error::new(
                    "Translation prompt must contain {target_language}",
                ));
            }
            let language = target_language(&request.target_language.0);
            let text = self
                .service
                .generate_inner(
                    self.connection.clone(),
                    self.auth.clone(),
                    GenerateRequest {
                        model: self.model.model.clone(),
                        system: self.prompt.replace("{target_language}", language),
                        input: request.text,
                        parameters: self.model.parameters.clone(),
                    },
                    progress,
                )
                .await?;
            Ok(TranslateResult {
                text,
                detected_source_language: None,
            })
        })
    }
}
fn target_language(code: &str) -> &str {
    match code {
        "zh-CN" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        "en" | "en-US" | "en-GB" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" | "pt-PT" => "European Portuguese",
        "pt-BR" => "Brazilian Portuguese",
        "ru" => "Russian",
        "ar" => "Arabic",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_http::{ok, server};

    #[test]
    #[ignore = "requires an existing user-managed Ollama service and installed text model"]
    fn installed_ollama_smoke() {
        live_smoke(LlmProvider::Ollama, None);
    }

    #[test]
    #[ignore = "uses LEXIFT_DEEPSEEK_API_KEY for one fixed-text paid API request"]
    fn deepseek_fixed_text_smoke() {
        let key = std::env::var("LEXIFT_DEEPSEEK_API_KEY")
            .expect("DeepSeek test credential must be provided through the process environment");
        live_smoke(LlmProvider::DeepSeek, Some(key));
    }

    fn live_smoke(provider: LlmProvider, credential: Option<String>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let service = LlmService::new().unwrap();
                let connection = LlmConnection::new(provider);
                let models = service
                    .models(connection.clone(), credential.clone())
                    .await
                    .unwrap();
                let model = models
                    .iter()
                    .find(|m| {
                        m.issue.is_none()
                            && (m.reasoning.is_empty() || m.reasoning.iter().any(|s| s == "off"))
                    })
                    .expect("A compatible installed text model is required");
                let translator = service.translator(
                    connection,
                    credential,
                    TranslationModel {
                        streaming: true,
                        provider,
                        model: model.id.clone(),
                        parameters: Default::default(),
                    },
                    DEFAULT_TRANSLATION_PROMPT.into(),
                );
                let result = translator
                    .translate(TranslateRequest {
                        text: "Hello, world.".into(),
                        source_language: None,
                        target_language: lexift_core::domain::language::Language("zh-CN".into()),
                    })
                    .await
                    .unwrap();
                assert!(!result.text.trim().is_empty());
                assert!(!result.text.contains("<think>"));
                println!(
                    "{} discovery and fixed-text translation passed",
                    provider.id()
                );
            });
    }

    #[test]
    #[ignore = "requires the user-provided LM Studio service; may run its already-installed model"]
    fn configured_lm_studio_protocol_smoke() {
        let address = std::env::var("LEXIFT_TEST_LMSTUDIO_ADDRESS")
            .expect("Supply a temporary LM Studio origin");
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let service=LlmService::new().unwrap();
            for protocol in LlmProtocol::ALL {
                let mut connection=LlmConnection::new(LlmProvider::LmStudio);connection.address=address.clone();connection.protocol=protocol;
                let models=service.models(connection.clone(),None).await.unwrap();
                let model=models.iter().find(|m|m.issue.is_none()).expect("Installed text model required");
                assert!(models.iter().any(|m|m.issue.is_some()),"embedding models must remain excluded");
                let text=service.generate(connection,None,GenerateRequest{model:model.id.clone(),system:"Translate the input to Simplified Chinese. Output only the translation.".into(),input:"Hello, world.".into(),parameters:Default::default()}).await.unwrap();
                assert!(!text.is_empty());println!("LM Studio {} discovery and fixed-text generation passed",protocol.id());
            }
        });
    }

    #[test]
    fn native_lm_studio_discovers_capabilities_and_generates_statelessly() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let list = r#"{"models":[{"key":"local","type":"llm","capabilities":{"reasoning":{"allowed_options":["off","low","high"]}}},{"key":"embed","type":"embedding"}]}"#;
            let (address, requests, task) = server(vec![ok(list),ok(list),ok(r#"{"output":[{"type":"reasoning","content":"private"},{"type":"message","content":"你好"}]}"#)]);
            let connection = LlmConnection { provider:LlmProvider::LmStudio, address, credential_id: None, ..LlmConnection::new(LlmProvider::LmStudio) };
            let service = LlmService::new().unwrap();
            let models = service.models(connection.clone(),Some("test-token".into())).await.unwrap();
            assert_eq!(models.iter().find(|m|m.id=="local").unwrap().reasoning, ["off","low","high"]);
            assert!(models.iter().find(|m|m.id=="embed").unwrap().issue.is_some());
            let text = service.generate(connection,Some("test-token".into()),GenerateRequest { model:"local".into(), system:"system prompt".into(),input:"Hello".into(),parameters:GenerationParameters {reasoning:"low".into(),temperature:Some(25),top_p:Some(90)} }).await.unwrap();
            assert_eq!(text,"你好");
            let discovery=requests.recv().unwrap(); assert!(discovery.starts_with("GET /api/v1/models ")); assert!(discovery.contains("Bearer test-token"));
            requests.recv().unwrap();
            let generated=requests.recv().unwrap(); let body: Value=serde_json::from_str(generated.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert!(generated.starts_with("POST /api/v1/chat ")); assert_eq!(body["store"],false);assert_eq!(body["temperature"],0.25);assert_eq!(body["reasoning"],"low");assert!(body.get("previous_response_id").is_none()); task.join().unwrap();
        });
    }

    #[test]
    fn ollama_translation_uses_custom_prompt_and_boolean_thinking_metadata() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (address,requests,task) = server(vec![ok(r#"{"models":[{"name":"model"}]}"#),ok(r#"{"capabilities":["completion"],"thinking":{"values":[false,true]}}"#),ok(r#"{"done":true,"done_reason":"stop","message":{"content":"你好","thinking":"private"}}"#)]);
            let translator = LlmService::new().unwrap().translator(LlmConnection {provider:LlmProvider::Ollama,address,credential_id:None,..LlmConnection::new(LlmProvider::Ollama)},None,TranslationModel { streaming: true,provider:LlmProvider::Ollama,model:"model".into(),parameters:Default::default()},"Custom translation to {target_language}".into());
            let result=translator.translate(TranslateRequest {text:"Hello\nworld".into(),source_language:None,target_language:lexift_core::domain::language::Language("zh-CN".into())}).await.unwrap();assert_eq!(result.text,"你好");
            requests.recv().unwrap();requests.recv().unwrap();let request=requests.recv().unwrap();let body:Value=serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body["think"],false);assert_eq!(body["messages"][0]["content"],"Custom translation to Simplified Chinese");assert_eq!(body["messages"][1]["content"],"Hello\nworld");assert_eq!(body["options"],json!({}));task.join().unwrap();
        });
    }

    #[test]
    fn discovery_rejects_redirect_auth_failure_and_malformed_responses() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for (status, body) in [
                    (302, "{}"),
                    (401, "{}"),
                    (500, "secret server details"),
                    (200, "not json"),
                ] {
                    let (address, _requests, task) = server(vec![(status, body.into())]);
                    let error = LlmService::new()
                        .unwrap()
                        .models(
                            LlmConnection {
                                provider: LlmProvider::LmStudio,
                                address,
                                credential_id: None,
                                ..LlmConnection::new(LlmProvider::LmStudio)
                            },
                            None,
                        )
                        .await
                        .unwrap_err()
                        .to_string();
                    assert!(!error.contains("secret server details"));
                    task.join().unwrap();
                }
            });
    }

    #[test]
    fn responses_extracts_only_complete_assistant_text() {
        let response = json!({"status":"completed","output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"translated"}]}]});
        assert_eq!(response_text(response.clone()).unwrap(), "translated");
        let mut truncated = response.clone();
        truncated["status"] = json!("incomplete");
        assert!(response_text(truncated).is_err());
        let mut tool = response;
        tool["output"][0]["type"] = json!("function_call");
        assert!(response_text(tool).is_err());
    }
    #[test]
    fn compatible_protocols_use_exact_base_paths_headers_and_stateless_payloads() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            for protocol in [LlmProtocol::ChatCompletions,LlmProtocol::Responses] {
                let result=if protocol==LlmProtocol::Responses {r#"{"status":"completed","output":[{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"translated"}]}]}"#} else {r#"{"choices":[{"finish_reason":"stop","message":{"content":"translated"}}]}"#};
                let list=r#"{"data":[{"id":"real-service-id","type":"llm"}]}"#;
                let (address,requests,task)=server(vec![ok(list),ok(result)]);
                let mut connection=LlmConnection::new(LlmProvider::DeepSeek);connection.address=address.clone();connection.protocol=protocol;
                connection.protocols.push(ProtocolSettings{protocol,base_url:format!("{address}/gateway/v1"),authentication:Authentication::Headers,headers:vec![HeaderReference{name:"Authorization".into(),credential_id:"test-reference".into()}]});
                let auth=LlmAuthorization{key:None,headers:vec![("Authorization".into(),"Bearer test-only".into())]};
                let translated=LlmService::new().unwrap().generate_authenticated(connection,auth,GenerateRequest{model:"real-service-id".into(),system:"custom system".into(),input:"original text".into(),parameters:Default::default()}).await.unwrap();
                assert_eq!(translated,"translated");
                let discovery=requests.recv().unwrap();assert!(discovery.starts_with("GET /gateway/v1/models "));assert!(discovery.to_lowercase().contains("authorization: bearer test-only"));
                let generated=requests.recv().unwrap();
                if protocol==LlmProtocol::Responses {assert!(generated.starts_with("POST /gateway/v1/responses "));assert!(generated.contains("\"store\":false"));assert!(generated.contains("\"instructions\":\"custom system\""));}
                else {assert!(generated.starts_with("POST /gateway/v1/chat/completions "));assert!(generated.contains("original text"));}
                assert!(!generated.contains("temperature"));assert!(!generated.contains("top_p"));task.join().unwrap();
            }
        });
    }
    #[test]
    fn thinking_controls_preserve_exact_supported_levels() {
        assert_eq!(
            reasoning(
                LlmProvider::Ollama,
                &json!({"thinking":{"values":[false,true,"low"]}})
            ),
            ["off", "on", "low"]
        );
        assert!(reasoning(LlmProvider::Ollama, &json!({"capabilities":["thinking"]})).is_empty());
        assert_eq!(
            reasoning(
                LlmProvider::DeepSeek,
                &json!({"effort":{"supported_levels":["low","high","max"]}})
            ),
            ["off", "low", "high", "max"]
        );
    }

    #[test]
    fn deepseek_sampling_mode_restrictions_are_not_silently_ignored() {
        let mut request = GenerateRequest {
            model: "selected".into(),
            system: "prompt".into(),
            input: "text".into(),
            parameters: Default::default(),
        };
        request.parameters.top_p = Some(80);
        assert!(payload(LlmProvider::DeepSeek, &request).is_err());
        request.parameters.reasoning = "high".into();
        assert!(payload(LlmProvider::DeepSeek, &request).is_err());
        request.parameters.top_p = Some(95);
        let (_, body) = payload(LlmProvider::DeepSeek, &request).unwrap();
        assert_eq!(body["top_p"], 0.95);
        assert_eq!(body["reasoning_effort"], "high");
        request.parameters.temperature = Some(50);
        assert!(payload(LlmProvider::DeepSeek, &request).is_err());
    }
    #[test]
    fn protocols_omit_default_sampling_and_keep_input_separate() {
        for provider in LlmProvider::ALL {
            let r = GenerateRequest {
                model: "model".into(),
                system: "translate".into(),
                input: "untrusted text".into(),
                parameters: Default::default(),
            };
            let (_, body) = payload(provider, &r).unwrap();
            assert_eq!(body["stream"], false);
            assert!(body.get("temperature").is_none());
            assert!(body.get("top_p").is_none());
            if provider == LlmProvider::LmStudio {
                assert_eq!(body["store"], false);
                assert_eq!(body["input"], r.input);
            } else {
                assert_eq!(body["messages"][1]["content"], r.input);
            }
        }
    }
    #[test]
    fn only_final_content_is_returned_and_truncation_fails() {
        assert_eq!(final_text(LlmProvider::DeepSeek,json!({"choices":[{"finish_reason":"stop","message":{"content":"译文","reasoning_content":"private"}}]})).unwrap(),"译文");
        assert!(
            final_text(
                LlmProvider::DeepSeek,
                json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
            )
            .is_err()
        );
        assert_eq!(final_text(LlmProvider::LmStudio,json!({"output":[{"type":"reasoning","content":"private"},{"type":"message","content":"译文"}]})).unwrap(),"译文");
    }
}
