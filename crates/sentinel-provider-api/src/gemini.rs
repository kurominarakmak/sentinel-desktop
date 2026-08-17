//! Google Gemini adapter using the official Interactions API.
//!
//! Wire reference: <https://ai.google.dev/api/interactions-api>

use crate::{
    bounded_redacted_diagnostic, openai_chat::validate_api_base_url, CancellationHandle,
    CredentialError, CredentialReference, MessageRole, ModelLimits, ProviderAdapter,
    ProviderCapabilities, ProviderCompletion, ProviderConfig, ProviderError, ProviderEvent,
    ProviderEventKind, ProviderEventSink, ProviderFuture, ProviderId, ProviderMessage,
    ProviderRequest, ProviderRun, ProviderTransport, RateLimitMetadata, ResponseFormat,
    SecretString, TokenUsage, ToolCall,
};
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-flash";
pub const SUPPORTED_GEMINI_MODELS: &[&str] = &[
    "gemini-3.6-flash",
    "gemini-3.5-flash",
    "gemini-3.1-pro-preview",
    "gemini-3.1-pro-preview-customtools",
    "gemini-3.1-flash-lite",
    "gemini-3-flash-preview",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
    "gemini-pro-latest",
    "gemini-flash-latest",
    "gemini-flash-lite-latest",
];
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub struct GeminiAdapter {
    config: ProviderConfig,
    client: Client,
}

impl GeminiAdapter {
    pub fn new(model_id: &str) -> Result<Self, ProviderError> {
        Self::with_base_url(model_id, DEFAULT_GEMINI_BASE_URL)
    }

    pub fn with_base_url(model_id: &str, base_url: &str) -> Result<Self, ProviderError> {
        validate_api_base_url(base_url)?;
        if !SUPPORTED_GEMINI_MODELS.contains(&model_id) && !is_loopback(base_url) {
            return Err(ProviderError::InvalidConfiguration(
                "unsupported Gemini model ID".into(),
            ));
        }
        let id = ProviderId::new("gemini")?;
        let config = ProviderConfig {
            id: id.clone(),
            display_name: "Gemini".into(),
            model_id: model_id.into(),
            base_url: Some(base_url.trim_end_matches('/').into()),
            enabled: true,
            transport: ProviderTransport::Api,
            capabilities: ProviderCapabilities {
                streaming: true,
                cancellation: true,
                tool_calling: true,
                structured_output: true,
                model_selection: true,
                rate_limits: true,
                implementation: true,
                read_only_review: true,
                repair: true,
            },
            limits: ModelLimits {
                context_tokens: None,
                max_output_tokens: None,
            },
            credential: Some(CredentialReference::for_provider(&id)),
        };
        config.validate()?;
        Self::from_config(config)
    }

    pub fn from_config(config: ProviderConfig) -> Result<Self, ProviderError> {
        if config.id.as_str() != "gemini"
            || config.transport != ProviderTransport::Api
            || (!SUPPORTED_GEMINI_MODELS.contains(&config.model_id.as_str())
                && !config.base_url.as_deref().is_some_and(is_loopback))
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Gemini provider configuration".into(),
            ));
        }
        config.validate()?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Unavailable("HTTP client unavailable".into()))?;
        Ok(Self { config, client })
    }
}

impl ProviderAdapter for GeminiAdapter {
    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    fn start<'a>(
        &'a self,
        request: ProviderRequest,
        credential: SecretString,
        events: Arc<dyn ProviderEventSink>,
    ) -> ProviderFuture<'a, ProviderRun> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move {
            request.validate(&config)?;
            validate_tool_results(&request.messages)?;
            let cancellation = CancellationHandle::default();
            let worker_cancellation = cancellation.clone();
            let completion = tokio::spawn(async move {
                execute(
                    client,
                    config,
                    request,
                    credential,
                    events,
                    worker_cancellation,
                )
                .await
            });
            Ok(ProviderRun::new(cancellation, completion))
        })
    }
}

fn validate_tool_results(messages: &[ProviderMessage]) -> Result<(), ProviderError> {
    if messages.iter().any(|message| {
        message.role == MessageRole::Tool
            && (message
                .tool_call_id
                .as_deref()
                .unwrap_or_default()
                .is_empty()
                || message.name.as_deref().unwrap_or_default().is_empty())
    }) {
        return Err(ProviderError::InvalidRequest(
            "Gemini tool results require function name and call ID".into(),
        ));
    }
    Ok(())
}

async fn execute(
    client: Client,
    config: ProviderConfig,
    request: ProviderRequest,
    credential: SecretString,
    events: Arc<dyn ProviderEventSink>,
    cancellation: CancellationHandle,
) -> Result<ProviderCompletion, ProviderError> {
    events.emit(ProviderEvent {
        request_id: request.request_id.clone(),
        sequence: 1,
        event: ProviderEventKind::Started,
    });
    let endpoint = format!(
        "{}/interactions",
        config
            .base_url
            .as_deref()
            .ok_or_else(|| ProviderError::InvalidConfiguration("base URL is missing".into()))?
            .trim_end_matches('/')
    );
    let key = std::str::from_utf8(credential.expose())
        .map_err(|_| ProviderError::Credential(CredentialError::InvalidSecret))?;
    let send = client
        .post(endpoint)
        .header("x-goog-api-key", key)
        .header("content-type", "application/json")
        .json(&request_body(&request))
        .send();
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        result = tokio::time::timeout(Duration::from_millis(request.timeout_ms), send) => {
            result.map_err(|_| ProviderError::Timeout)?
                .map_err(|error| ProviderError::Transport(bounded_redacted_diagnostic(&error.to_string(), &[credential.expose()])))?
        }
    };
    let rate_limits = rate_limits(response.headers());
    if !response.status().is_success() {
        return Err(http_error(response, &credential, rate_limits).await);
    }
    if response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
    {
        parse_stream(response, config, request, rate_limits, events, cancellation).await
    } else {
        parse_complete(response, config, request, rate_limits, events).await
    }
}

fn request_body(request: &ProviderRequest) -> Value {
    let system_instruction = request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut input = Vec::new();
    for message in request
        .messages
        .iter()
        .filter(|message| message.role != MessageRole::System)
    {
        match message.role {
            MessageRole::User => input.push(
                json!({"type":"user_input","content":[{"type":"text","text":message.content}]}),
            ),
            MessageRole::Assistant => {
                if !message.content.is_empty() {
                    input.push(
                        json!({"type":"model_output","content":[{"type":"text","text":message.content}]}),
                    );
                }
                input.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type":"function_call",
                        "name":call.name,
                        "call_id":call.id,
                        "arguments":call.arguments,
                    })
                }));
            }
            MessageRole::Tool => input.push(json!({
                "type":"function_result",
                "name":message.name,
                "call_id":message.tool_call_id,
                "result":[{"type":"text","text":message.content}]
            })),
            MessageRole::System => unreachable!("system messages are filtered"),
        }
    }
    let mut body = Map::from_iter([
        ("model".into(), Value::String(request.model_id.clone())),
        ("input".into(), Value::Array(input)),
        ("stream".into(), Value::Bool(true)),
        ("store".into(), Value::Bool(false)),
    ]);
    if !system_instruction.is_empty() {
        body.insert(
            "system_instruction".into(),
            Value::String(system_instruction),
        );
    }
    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type":"function",
                            "name":tool.name,
                            "description":tool.description,
                            "parameters":tool.parameters,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if request.response_format == ResponseFormat::JsonObject {
        body.insert(
            "response_format".into(),
            json!({"type":"text","mime_type":"application/json","schema":{"type":"object"}}),
        );
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        body.insert(
            "generation_config".into(),
            json!({"max_output_tokens":max_output_tokens}),
        );
    }
    Value::Object(body)
}

#[derive(Default)]
struct Accumulator {
    text: String,
    reasoning: String,
    tools: BTreeMap<String, ToolCall>,
    usage: TokenUsage,
    finish_reason: Option<String>,
    sequence: u64,
}

async fn parse_stream(
    response: Response,
    config: ProviderConfig,
    request: ProviderRequest,
    rate_limits: Option<RateLimitMetadata>,
    events: Arc<dyn ProviderEventSink>,
    cancellation: CancellationHandle,
) -> Result<ProviderCompletion, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut total = 0usize;
    let mut accumulator = Accumulator {
        sequence: 1,
        ..Default::default()
    };
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| ProviderError::Transport(error.to_string()))?;
        total = total.saturating_add(chunk.len());
        if total > MAX_RESPONSE_BYTES {
            return Err(ProviderError::MalformedResponse);
        }
        buffer.extend_from_slice(&chunk);
        while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = buffer.drain(..=position).collect::<Vec<_>>();
            while line
                .last()
                .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
            {
                line.pop();
            }
            process_sse_line(&line, &request, &events, &mut accumulator)?;
        }
    }
    if !buffer.is_empty() {
        process_sse_line(&buffer, &request, &events, &mut accumulator)?;
    }
    complete(config, request, rate_limits, events, accumulator)
}

fn process_sse_line(
    line: &[u8],
    request: &ProviderRequest,
    events: &Arc<dyn ProviderEventSink>,
    accumulator: &mut Accumulator,
) -> Result<(), ProviderError> {
    if line.is_empty() || line.starts_with(b":") {
        return Ok(());
    }
    let Some(data) = line.strip_prefix(b"data:") else {
        return Ok(());
    };
    let data = data.strip_prefix(b" ").unwrap_or(data);
    let value: Value =
        serde_json::from_slice(data).map_err(|_| ProviderError::MalformedResponse)?;
    process_event(&value, request, events, accumulator)
}

fn process_event(
    value: &Value,
    request: &ProviderRequest,
    events: &Arc<dyn ProviderEventSink>,
    accumulator: &mut Accumulator,
) -> Result<(), ProviderError> {
    match value.get("event_type").and_then(Value::as_str) {
        Some("step.delta") => {
            let delta = value.get("delta").ok_or(ProviderError::MalformedResponse)?;
            let kind = delta.get("type").and_then(Value::as_str);
            let text = delta
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !text.is_empty() {
                accumulator.sequence += 1;
                if kind == Some("thought") {
                    accumulator.reasoning.push_str(&text);
                    events.emit(ProviderEvent {
                        request_id: request.request_id.clone(),
                        sequence: accumulator.sequence,
                        event: ProviderEventKind::ReasoningDelta { text },
                    });
                } else if kind == Some("text") {
                    accumulator.text.push_str(&text);
                    events.emit(ProviderEvent {
                        request_id: request.request_id.clone(),
                        sequence: accumulator.sequence,
                        event: ProviderEventKind::TextDelta { text },
                    });
                }
            }
            if let Some(usage) = value
                .get("metadata")
                .and_then(|value| value.get("total_usage"))
            {
                accumulator.usage = parse_usage(Some(usage));
            }
        }
        Some("step.start") => {
            if let Some(step) = value.get("step") {
                if step.get("type").and_then(Value::as_str) == Some("function_call") {
                    let call = parse_function_call(step)?;
                    accumulator.sequence += 1;
                    events.emit(ProviderEvent {
                        request_id: request.request_id.clone(),
                        sequence: accumulator.sequence,
                        event: ProviderEventKind::ToolCall { call: call.clone() },
                    });
                    accumulator.tools.insert(call.id.clone(), call);
                }
            }
        }
        Some("interaction.completed") => {
            accumulator.finish_reason = Some("completed".into());
            if let Some(usage) = value
                .get("interaction")
                .and_then(|value| value.get("usage"))
            {
                accumulator.usage = parse_usage(Some(usage));
            }
        }
        Some("interaction.status_update") => {
            if let Some(status) = value.get("status").and_then(Value::as_str) {
                accumulator.finish_reason = Some(status.into());
            }
        }
        Some("error") => return Err(ProviderError::MalformedResponse),
        _ => {}
    }
    Ok(())
}

async fn parse_complete(
    response: Response,
    config: ProviderConfig,
    request: ProviderRequest,
    rate_limits: Option<RateLimitMetadata>,
    events: Arc<dyn ProviderEventSink>,
) -> Result<ProviderCompletion, ProviderError> {
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ProviderError::Transport(error.to_string()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ProviderError::MalformedResponse);
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::MalformedResponse)?;
    let mut accumulator = Accumulator {
        sequence: 1,
        finish_reason: value
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        usage: parse_usage(value.get("usage")),
        ..Default::default()
    };
    let steps = value
        .get("steps")
        .and_then(Value::as_array)
        .ok_or(ProviderError::MalformedResponse)?;
    for step in steps {
        match step.get("type").and_then(Value::as_str) {
            Some("model_output") => {
                for content in step
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if content.get("type").and_then(Value::as_str) == Some("text") {
                        accumulator.text.push_str(
                            content
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        );
                    }
                }
            }
            Some("thought") => {
                for content in step
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if content.get("type").and_then(Value::as_str) == Some("text") {
                        accumulator.reasoning.push_str(
                            content
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        );
                    }
                }
            }
            Some("function_call") => {
                let call = parse_function_call(step)?;
                accumulator.tools.insert(call.id.clone(), call);
            }
            _ => {}
        }
    }
    complete(config, request, rate_limits, events, accumulator)
}

fn complete(
    config: ProviderConfig,
    request: ProviderRequest,
    rate_limits: Option<RateLimitMetadata>,
    events: Arc<dyn ProviderEventSink>,
    mut accumulator: Accumulator,
) -> Result<ProviderCompletion, ProviderError> {
    accumulator.sequence += 1;
    events.emit(ProviderEvent {
        request_id: request.request_id.clone(),
        sequence: accumulator.sequence,
        event: ProviderEventKind::Completed {
            finish_reason: accumulator.finish_reason.clone(),
            usage: accumulator.usage.clone(),
        },
    });
    Ok(ProviderCompletion {
        provider_id: config.id,
        request_id: request.request_id,
        model_id: request.model_id,
        text: accumulator.text,
        reasoning: (!accumulator.reasoning.is_empty()).then_some(accumulator.reasoning),
        tool_calls: accumulator.tools.into_values().collect(),
        finish_reason: accumulator.finish_reason,
        usage: accumulator.usage,
        rate_limits,
    })
}

fn parse_function_call(value: &Value) -> Result<ToolCall, ProviderError> {
    Ok(ToolCall {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ProviderError::MalformedResponse)?
            .into(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .ok_or(ProviderError::MalformedResponse)?
            .into(),
        arguments: value
            .get("arguments")
            .cloned()
            .ok_or(ProviderError::MalformedResponse)?,
    })
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    TokenUsage {
        input_tokens: value
            .and_then(|value| value.get("total_input_tokens"))
            .and_then(Value::as_u64),
        output_tokens: value
            .and_then(|value| value.get("total_output_tokens"))
            .and_then(Value::as_u64),
        total_tokens: value
            .and_then(|value| value.get("total_tokens"))
            .and_then(Value::as_u64),
    }
}

fn rate_limits(headers: &reqwest::header::HeaderMap) -> Option<RateLimitMetadata> {
    let retry_after_ms = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds * 1_000);
    retry_after_ms.map(|retry_after_ms| RateLimitMetadata {
        retry_after_ms: Some(retry_after_ms),
        ..Default::default()
    })
}

async fn http_error(
    response: Response,
    credential: &SecretString,
    rate_limits: Option<RateLimitMetadata>,
) -> ProviderError {
    let status = response.status();
    let diagnostic = response
        .text()
        .await
        .ok()
        .map(|body| bounded_redacted_diagnostic(&body, &[credential.expose()]))
        .unwrap_or_default();
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Authentication,
        StatusCode::TOO_MANY_REQUESTS => {
            ProviderError::RateLimited(rate_limits.unwrap_or_default())
        }
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ProviderError::InvalidRequest(diagnostic)
        }
        _ => ProviderError::Unavailable(diagnostic),
    }
}

fn is_loopback(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(ToOwned::to_owned))
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CredentialStore, MemoryCredentialStore, ProviderRegistry, ResponseFormat, ToolDefinition,
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    struct FakeServer {
        url: String,
        request: Arc<Mutex<String>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn sse(body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let request = Arc::new(Mutex::new(String::new()));
            let captured = request.clone();
            let body = body.to_owned();
            let thread = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 2048];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                *captured.lock().unwrap() = String::from_utf8_lossy(&bytes).into_owned();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            });
            Self {
                url: format!("http://{address}"),
                request,
                thread: Some(thread),
            }
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            request_id: "gemini-request".into(),
            model_id: "test-gemini".into(),
            messages: vec![
                ProviderMessage {
                    role: MessageRole::System,
                    content: "Review only".into(),
                    tool_call_id: None,
                    name: None,
                    tool_calls: Vec::new(),
                },
                ProviderMessage {
                    role: MessageRole::User,
                    content: "Inspect".into(),
                    tool_call_id: None,
                    name: None,
                    tool_calls: Vec::new(),
                },
            ],
            tools: vec![ToolDefinition {
                name: "inspect_file".into(),
                description: "Inspect approved file".into(),
                parameters: json!({"type":"object"}),
                strict: None,
            }],
            response_format: ResponseFormat::JsonObject,
            max_output_tokens: Some(100),
            timeout_ms: 2_000,
        }
    }

    #[tokio::test]
    async fn gemini_interactions_stream_is_normalized() {
        let body = concat!(
            "data: {\"event_type\":\"interaction.created\",\"interaction\":{\"id\":\"interaction-1\"}}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\"hello\"},\"metadata\":{\"total_usage\":{\"total_input_tokens\":2,\"total_output_tokens\":3,\"total_tokens\":5}}}\n\n",
            "data: {\"event_type\":\"step.start\",\"index\":1,\"step\":{\"type\":\"function_call\",\"id\":\"call-1\",\"name\":\"inspect_file\",\"arguments\":{\"path\":\"src/lib.rs\"}}}\n\n",
            "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\"}}\n\n"
        );
        let server = FakeServer::sse(body);
        let credentials = Arc::new(MemoryCredentialStore::default());
        let adapter = Arc::new(GeminiAdapter::with_base_url("test-gemini", &server.url).unwrap());
        let id = adapter.config.id.clone();
        credentials
            .set(
                adapter.config.credential.as_ref().unwrap(),
                SecretString::new("gemini-key").unwrap(),
            )
            .unwrap();
        let mut registry = ProviderRegistry::new(credentials);
        registry.register_api(adapter).unwrap();
        let completion = registry
            .start(&id, request(), Arc::new(|_| {}))
            .await
            .unwrap()
            .completion()
            .await
            .unwrap();
        assert_eq!(completion.text, "hello");
        assert_eq!(completion.tool_calls[0].name, "inspect_file");
        assert_eq!(completion.usage.total_tokens, Some(5));
        let captured = server.request.lock().unwrap();
        assert!(captured.starts_with("POST /interactions HTTP/1.1"));
        assert!(captured
            .to_ascii_lowercase()
            .contains("x-goog-api-key: gemini-key"));
        assert!(captured.contains("\"system_instruction\":\"Review only\""));
        assert!(captured.contains("\"type\":\"function\""));
        assert!(captured.contains("\"mime_type\":\"application/json\""));
        assert!(!captured
            .to_ascii_lowercase()
            .contains("authorization: bearer"));
    }

    #[test]
    fn gemini_models_and_tool_results_are_validated() {
        assert_eq!(
            GeminiAdapter::new(DEFAULT_GEMINI_MODEL)
                .unwrap()
                .config
                .model_id,
            "gemini-3.6-flash"
        );
        assert!(GeminiAdapter::new("invented-gemini").is_err());
        let message = ProviderMessage {
            role: MessageRole::Tool,
            content: "result".into(),
            tool_call_id: Some("call".into()),
            name: None,
            tool_calls: Vec::new(),
        };
        assert!(validate_tool_results(&[message]).is_err());
    }
}
