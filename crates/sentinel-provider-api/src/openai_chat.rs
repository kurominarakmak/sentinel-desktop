use crate::{
    bounded_redacted_diagnostic, CancellationHandle, MessageRole, ProviderCompletion,
    ProviderConfig, ProviderError, ProviderEvent, ProviderEventKind, ProviderEventSink, ProviderId,
    ProviderMessage, ProviderRequest, RateLimitMetadata, ResponseFormat, SecretString, TokenUsage,
    ToolCall,
};
use futures_util::StreamExt;
use reqwest::{header::HeaderMap, Client, Response, StatusCode};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenAiDialect {
    Glm,
    Kimi,
}

pub(crate) async fn execute(
    client: Client,
    config: ProviderConfig,
    request: ProviderRequest,
    credential: SecretString,
    events: Arc<dyn ProviderEventSink>,
    cancellation: CancellationHandle,
    dialect: OpenAiDialect,
) -> Result<ProviderCompletion, ProviderError> {
    let endpoint = chat_endpoint(
        config
            .base_url
            .as_deref()
            .ok_or_else(|| ProviderError::InvalidConfiguration("base URL is missing".into()))?,
    );
    let body = request_body(&request, dialect);
    let request_id = request.request_id.clone();
    events.emit(ProviderEvent {
        request_id: request_id.clone(),
        sequence: 1,
        event: ProviderEventKind::Started,
    });
    let send = client
        .post(endpoint)
        .bearer_auth(
            std::str::from_utf8(credential.expose())
                .map_err(|_| ProviderError::Credential(crate::CredentialError::InvalidSecret))?,
        )
        .header("content-type", "application/json")
        .json(&body)
        .send();
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        result = tokio::time::timeout(Duration::from_millis(request.timeout_ms), send) => {
            result.map_err(|_| ProviderError::Timeout)?
                .map_err(|error| transport_error(&error, &credential))?
        }
    };
    let rate_limits = rate_limit_metadata(response.headers());
    if !response.status().is_success() {
        return Err(http_error(response, &credential, rate_limits).await);
    }
    if let Some(metadata) = rate_limits.clone() {
        events.emit(ProviderEvent {
            request_id: request_id.clone(),
            sequence: 2,
            event: ProviderEventKind::RateLimits { metadata },
        });
    }
    if response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
    {
        parse_stream(
            response,
            &config,
            &request,
            rate_limits,
            events,
            cancellation,
        )
        .await
    } else {
        parse_complete(response, &config, &request, rate_limits, events).await
    }
}

fn chat_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.into()
    } else {
        format!("{base}/chat/completions")
    }
}

fn request_body(request: &ProviderRequest, dialect: OpenAiDialect) -> Value {
    let messages = request
        .messages
        .iter()
        .map(message_value)
        .collect::<Vec<_>>();
    let mut body = Map::from_iter([
        ("model".into(), Value::String(request.model_id.clone())),
        ("messages".into(), Value::Array(messages)),
        ("stream".into(), Value::Bool(true)),
    ]);
    if let Some(max_tokens) = request.max_output_tokens {
        body.insert("max_tokens".into(), Value::from(max_tokens));
    }
    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        let mut function = Map::from_iter([
                            ("name".into(), Value::String(tool.name.clone())),
                            (
                                "description".into(),
                                Value::String(tool.description.clone()),
                            ),
                            ("parameters".into(), tool.parameters.clone()),
                        ]);
                        if dialect == OpenAiDialect::Kimi {
                            if let Some(strict) = tool.strict {
                                function.insert("strict".into(), Value::Bool(strict));
                            }
                        }
                        json!({"type":"function","function":function})
                    })
                    .collect(),
            ),
        );
        body.insert("tool_choice".into(), Value::String("auto".into()));
        if dialect == OpenAiDialect::Glm {
            body.insert("tool_stream".into(), Value::Bool(true));
        }
    }
    if request.response_format == ResponseFormat::JsonObject {
        body.insert("response_format".into(), json!({"type":"json_object"}));
    }
    if dialect == OpenAiDialect::Glm {
        body.insert(
            "request_id".into(),
            Value::String(request.request_id.clone()),
        );
    }
    Value::Object(body)
}

fn message_value(message: &ProviderMessage) -> Value {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    let mut value = Map::from_iter([
        ("role".into(), Value::String(role.into())),
        ("content".into(), Value::String(message.content.clone())),
    ]);
    if let Some(tool_call_id) = &message.tool_call_id {
        value.insert("tool_call_id".into(), Value::String(tool_call_id.clone()));
    }
    Value::Object(value)
}

async fn parse_complete(
    response: Response,
    config: &ProviderConfig,
    request: &ProviderRequest,
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
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or(ProviderError::MalformedResponse)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or(ProviderError::MalformedResponse)?;
    let text = optional_string(message.get("content")).unwrap_or_default();
    let reasoning = optional_string(message.get("reasoning_content"));
    let tool_calls = parse_tool_calls(message.get("tool_calls"))?;
    let finish_reason = optional_string(choice.get("finish_reason"));
    let usage = parse_usage(value.get("usage"));
    let mut sequence = 3;
    if !text.is_empty() {
        events.emit(ProviderEvent {
            request_id: request.request_id.clone(),
            sequence,
            event: ProviderEventKind::TextDelta { text: text.clone() },
        });
        sequence += 1;
    }
    for call in &tool_calls {
        events.emit(ProviderEvent {
            request_id: request.request_id.clone(),
            sequence,
            event: ProviderEventKind::ToolCall { call: call.clone() },
        });
        sequence += 1;
    }
    events.emit(ProviderEvent {
        request_id: request.request_id.clone(),
        sequence,
        event: ProviderEventKind::Completed {
            finish_reason: finish_reason.clone(),
            usage: usage.clone(),
        },
    });
    Ok(ProviderCompletion {
        provider_id: config.id.clone(),
        request_id: request.request_id.clone(),
        model_id: request.model_id.clone(),
        text,
        reasoning,
        tool_calls,
        finish_reason,
        usage,
        rate_limits,
    })
}

#[derive(Default)]
struct StreamAccumulator {
    text: String,
    reasoning: String,
    tools: BTreeMap<usize, ToolAccumulator>,
    finish_reason: Option<String>,
    usage: TokenUsage,
    sequence: u64,
}

#[derive(Default)]
struct ToolAccumulator {
    id: String,
    name: String,
    arguments: String,
}

async fn parse_stream(
    response: Response,
    config: &ProviderConfig,
    request: &ProviderRequest,
    rate_limits: Option<RateLimitMetadata>,
    events: Arc<dyn ProviderEventSink>,
    cancellation: CancellationHandle,
) -> Result<ProviderCompletion, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut total = 0usize;
    let mut accumulator = StreamAccumulator {
        sequence: if rate_limits.is_some() { 2 } else { 1 },
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
                .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
            {
                line.pop();
            }
            process_sse_line(&line, request, &events, &mut accumulator)?;
        }
    }
    if !buffer.is_empty() {
        process_sse_line(&buffer, request, &events, &mut accumulator)?;
    }
    let mut tool_calls = Vec::new();
    for (index, tool) in accumulator.tools {
        let arguments =
            serde_json::from_str(&tool.arguments).map_err(|_| ProviderError::MalformedResponse)?;
        let call = ToolCall {
            id: if tool.id.is_empty() {
                format!("tool-{index}")
            } else {
                tool.id
            },
            name: tool.name,
            arguments,
        };
        accumulator.sequence += 1;
        events.emit(ProviderEvent {
            request_id: request.request_id.clone(),
            sequence: accumulator.sequence,
            event: ProviderEventKind::ToolCall { call: call.clone() },
        });
        tool_calls.push(call);
    }
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
        provider_id: config.id.clone(),
        request_id: request.request_id.clone(),
        model_id: request.model_id.clone(),
        text: accumulator.text,
        reasoning: (!accumulator.reasoning.is_empty()).then_some(accumulator.reasoning),
        tool_calls,
        finish_reason: accumulator.finish_reason,
        usage: accumulator.usage,
        rate_limits,
    })
}

fn process_sse_line(
    line: &[u8],
    request: &ProviderRequest,
    events: &Arc<dyn ProviderEventSink>,
    accumulator: &mut StreamAccumulator,
) -> Result<(), ProviderError> {
    if line.is_empty() || line.starts_with(b":") {
        return Ok(());
    }
    let Some(data) = line.strip_prefix(b"data:") else {
        return Ok(());
    };
    let data = data.strip_prefix(b" ").unwrap_or(data);
    if data == b"[DONE]" {
        return Ok(());
    }
    let value: Value =
        serde_json::from_slice(data).map_err(|_| ProviderError::MalformedResponse)?;
    if let Some(usage) = value.get("usage") {
        accumulator.usage = parse_usage(Some(usage));
    }
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    if let Some(reason) = optional_string(choice.get("finish_reason")) {
        accumulator.finish_reason = Some(reason);
    }
    let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
        return Ok(());
    };
    if let Some(text) = optional_string(delta.get("content")) {
        accumulator.text.push_str(&text);
        accumulator.sequence += 1;
        events.emit(ProviderEvent {
            request_id: request.request_id.clone(),
            sequence: accumulator.sequence,
            event: ProviderEventKind::TextDelta { text },
        });
    }
    if let Some(text) = optional_string(delta.get("reasoning_content")) {
        accumulator.reasoning.push_str(&text);
        accumulator.sequence += 1;
        events.emit(ProviderEvent {
            request_id: request.request_id.clone(),
            sequence: accumulator.sequence,
            event: ProviderEventKind::ReasoningDelta { text },
        });
    }
    if let Some(tools) = delta.get("tool_calls").and_then(Value::as_array) {
        for tool in tools {
            let index = tool.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = accumulator.tools.entry(index).or_default();
            let id = optional_string(tool.get("id"));
            if let Some(id) = &id {
                entry.id.push_str(id);
            }
            let function = tool.get("function").and_then(Value::as_object);
            let name = function.and_then(|value| optional_string(value.get("name")));
            if let Some(name) = &name {
                entry.name.push_str(name);
            }
            let arguments = function
                .and_then(|value| optional_string(value.get("arguments")))
                .unwrap_or_default();
            entry.arguments.push_str(&arguments);
            accumulator.sequence += 1;
            events.emit(ProviderEvent {
                request_id: request.request_id.clone(),
                sequence: accumulator.sequence,
                event: ProviderEventKind::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta: arguments,
                },
            });
        }
    }
    Ok(())
}

fn parse_tool_calls(value: Option<&Value>) -> Result<Vec<ToolCall>, ProviderError> {
    let Some(calls) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    calls
        .iter()
        .map(|call| {
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or(ProviderError::MalformedResponse)?;
            let arguments = match function.get("arguments") {
                Some(Value::String(value)) => {
                    serde_json::from_str(value).map_err(|_| ProviderError::MalformedResponse)?
                }
                Some(value @ Value::Object(_)) => value.clone(),
                _ => return Err(ProviderError::MalformedResponse),
            };
            Ok(ToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::MalformedResponse)?
                    .into(),
                arguments,
            })
        })
        .collect()
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    TokenUsage {
        input_tokens: value
            .and_then(|value| value.get("prompt_tokens"))
            .and_then(Value::as_u64),
        output_tokens: value
            .and_then(|value| value.get("completion_tokens"))
            .and_then(Value::as_u64),
        total_tokens: value
            .and_then(|value| value.get("total_tokens"))
            .and_then(Value::as_u64),
    }
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

fn rate_limit_metadata(headers: &HeaderMap) -> Option<RateLimitMetadata> {
    let metadata = RateLimitMetadata {
        request_limit: header_u64(headers, "x-ratelimit-limit-requests"),
        request_remaining: header_u64(headers, "x-ratelimit-remaining-requests"),
        token_limit: header_u64(headers, "x-ratelimit-limit-tokens"),
        token_remaining: header_u64(headers, "x-ratelimit-remaining-tokens"),
        reset_at: header_string(headers, "x-ratelimit-reset-requests")
            .or_else(|| header_string(headers, "x-ratelimit-reset")),
        retry_after_ms: header_u64(headers, "retry-after").map(|seconds| seconds * 1_000),
    };
    (metadata != RateLimitMetadata::default()).then_some(metadata)
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .map(|value| value.chars().take(128).collect())
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

fn transport_error(error: &reqwest::Error, credential: &SecretString) -> ProviderError {
    ProviderError::Transport(bounded_redacted_diagnostic(
        &error.to_string(),
        &[credential.expose()],
    ))
}

pub(crate) fn validate_api_base_url(base_url: &str) -> Result<(), ProviderError> {
    let parsed = reqwest::Url::parse(base_url)
        .map_err(|_| ProviderError::InvalidConfiguration("base URL is invalid".into()))?;
    let scheme_allowed = parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && parsed
                .host_str()
                .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1")));
    if !scheme_allowed
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ProviderError::InvalidConfiguration(
            "base URL must be HTTPS without embedded credentials (HTTP is loopback-test-only)"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn api_config(
    id: &str,
    display_name: &str,
    model_id: &str,
    base_url: &str,
    capabilities: crate::ProviderCapabilities,
    limits: crate::ModelLimits,
) -> Result<ProviderConfig, ProviderError> {
    validate_api_base_url(base_url)?;
    let id = ProviderId::new(id)?;
    let config = ProviderConfig {
        id: id.clone(),
        display_name: display_name.into(),
        model_id: model_id.into(),
        base_url: Some(base_url.trim_end_matches('/').into()),
        enabled: true,
        transport: crate::ProviderTransport::Api,
        capabilities,
        limits,
        credential: Some(crate::CredentialReference::for_provider(&id)),
    };
    config.validate()?;
    Ok(config)
}
