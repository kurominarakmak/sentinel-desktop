//! Z.AI GLM adapter using the official Chat Completions API.
//!
//! Wire reference: <https://docs.z.ai/api-reference/llm/chat-completion>

use crate::{
    openai_chat::{api_config, execute, OpenAiDialect},
    CancellationHandle, ModelLimits, ProviderAdapter, ProviderCapabilities, ProviderConfig,
    ProviderError, ProviderEventSink, ProviderFuture, ProviderRequest, ProviderRun, SecretString,
};
use reqwest::Client;
use std::sync::Arc;

pub const DEFAULT_GLM_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
pub const DEFAULT_GLM_MODEL: &str = "glm-5.2";
pub const SUPPORTED_GLM_MODELS: &[&str] = &[
    "glm-5.2",
    "glm-5.1",
    "glm-5-turbo",
    "glm-5",
    "glm-4.7",
    "glm-4.7-flash",
    "glm-4.7-flashx",
    "glm-4.6",
    "glm-4.5",
    "glm-4.5-air",
    "glm-4.5-x",
    "glm-4.5-airx",
    "glm-4.5-flash",
    "glm-4-32b-0414-128k",
];

pub struct GlmAdapter {
    config: ProviderConfig,
    client: Client,
}

impl GlmAdapter {
    pub fn new(model_id: &str) -> Result<Self, ProviderError> {
        Self::with_base_url(model_id, DEFAULT_GLM_BASE_URL)
    }

    pub fn with_base_url(model_id: &str, base_url: &str) -> Result<Self, ProviderError> {
        if !SUPPORTED_GLM_MODELS.contains(&model_id) && !is_loopback(base_url) {
            return Err(ProviderError::InvalidConfiguration(
                "unsupported GLM model ID".into(),
            ));
        }
        let config = api_config(
            "glm",
            "GLM",
            model_id,
            base_url,
            ProviderCapabilities {
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
            ModelLimits {
                context_tokens: None,
                max_output_tokens: Some(131_072),
            },
        )?;
        Self::from_config(config)
    }

    pub fn from_config(config: ProviderConfig) -> Result<Self, ProviderError> {
        if config.id.as_str() != "glm"
            || config.transport != crate::ProviderTransport::Api
            || (!SUPPORTED_GLM_MODELS.contains(&config.model_id.as_str())
                && !config.base_url.as_deref().is_some_and(is_loopback))
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid GLM provider configuration".into(),
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

impl ProviderAdapter for GlmAdapter {
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
                    OpenAiDialect::Glm,
                )
                .await
            });
            Ok(ProviderRun::new(cancellation, completion))
        })
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
        CredentialStore, MemoryCredentialStore, MessageRole, ProviderEvent, ProviderEventKind,
        ProviderId, ProviderMessage, ProviderRegistry, ResponseFormat, ToolDefinition,
    };
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Condvar, Mutex},
        thread,
        time::Duration,
    };

    struct FakeServer {
        url: String,
        request: Arc<(Mutex<Option<String>>, Condvar)>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn respond(status: &str, headers: &[(&str, &str)], body: &str, delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let request = Arc::new((Mutex::new(None), Condvar::new()));
            let captured = request.clone();
            let status = status.to_owned();
            let headers = headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<Vec<_>>();
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
                    if let Some(header_end) = find_bytes(&bytes, b"\r\n\r\n") {
                        let headers_text = String::from_utf8_lossy(&bytes[..header_end]);
                        let content_length = headers_text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                let (lock, ready) = &*captured;
                *lock.lock().unwrap() = Some(String::from_utf8_lossy(&bytes).into_owned());
                ready.notify_all();
                thread::sleep(delay);
                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&body);
                let _ = stream.write_all(response.as_bytes());
            });
            Self {
                url: format!("http://{address}"),
                request,
                thread: Some(thread),
            }
        }

        fn captured(&self) -> String {
            self.request.0.lock().unwrap().clone().unwrap_or_default()
        }

        fn wait_for_request(&self) {
            let (lock, ready) = &*self.request;
            let request = lock.lock().unwrap();
            let (request, wait) = ready
                .wait_timeout_while(request, Duration::from_secs(2), |request| request.is_none())
                .unwrap();
            assert!(!wait.timed_out() && request.is_some());
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn request(timeout_ms: u64) -> ProviderRequest {
        ProviderRequest {
            request_id: "glm-request-1".into(),
            model_id: "test-glm".into(),
            messages: vec![ProviderMessage {
                role: MessageRole::User,
                content: "hello".into(),
                tool_call_id: None,
                name: None,
            }],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read an approved file".into(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
                strict: Some(true),
            }],
            response_format: ResponseFormat::Text,
            max_output_tokens: Some(100),
            timeout_ms,
        }
    }

    fn registry(server: &FakeServer) -> (ProviderRegistry, ProviderId) {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let adapter = Arc::new(GlmAdapter::with_base_url("test-glm", &server.url).unwrap());
        let id = adapter.config.id.clone();
        credentials
            .set(
                adapter.config.credential.as_ref().unwrap(),
                SecretString::new("glm-test-key").unwrap(),
            )
            .unwrap();
        let mut registry = ProviderRegistry::new(credentials);
        registry.register_api(adapter).unwrap();
        (registry, id)
    }

    #[tokio::test]
    async fn streams_text_reasoning_tools_usage_and_rate_limits() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"world\",\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"src/lib.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        );
        let server = FakeServer::respond(
            "200 OK",
            &[
                ("Content-Type", "text/event-stream"),
                ("x-ratelimit-remaining-requests", "9"),
            ],
            body,
            Duration::ZERO,
        );
        let (registry, id) = registry(&server);
        let events = Arc::new(Mutex::new(Vec::<ProviderEvent>::new()));
        let captured = events.clone();
        let run = registry
            .start(
                &id,
                request(2_000),
                Arc::new(move |event| captured.lock().unwrap().push(event)),
            )
            .await
            .unwrap();
        let completion = run.completion().await.unwrap();
        assert_eq!(completion.text, "hello world");
        assert_eq!(completion.reasoning.as_deref(), Some("think"));
        assert_eq!(completion.tool_calls[0].name, "read_file");
        assert_eq!(
            completion.tool_calls[0].arguments,
            json!({"path":"src/lib.rs"})
        );
        assert_eq!(completion.usage.total_tokens, Some(7));
        assert_eq!(completion.rate_limits.unwrap().request_remaining, Some(9));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, ProviderEventKind::ToolCallDelta { .. })));
        let captured = server.captured();
        assert!(captured.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(captured
            .to_ascii_lowercase()
            .contains("authorization: bearer glm-test-key"));
        assert!(captured.contains("\"tool_stream\":true"));
    }

    #[tokio::test]
    async fn auth_failure_is_classified_without_leaking_key() {
        let server = FakeServer::respond(
            "401 Unauthorized",
            &[("Content-Type", "application/json")],
            "{\"error\":\"invalid glm-test-key\"}",
            Duration::ZERO,
        );
        let (registry, id) = registry(&server);
        let error = registry
            .start(&id, request(1_000), Arc::new(|_| {}))
            .await
            .unwrap()
            .completion()
            .await
            .unwrap_err();
        assert_eq!(error, ProviderError::Authentication);
        assert!(!error.to_string().contains("glm-test-key"));
    }

    #[tokio::test]
    async fn malformed_stream_fails_closed() {
        let server = FakeServer::respond(
            "200 OK",
            &[("Content-Type", "text/event-stream")],
            "data: not-json\n\n",
            Duration::ZERO,
        );
        let (registry, id) = registry(&server);
        let error = registry
            .start(&id, request(1_000), Arc::new(|_| {}))
            .await
            .unwrap()
            .completion()
            .await
            .unwrap_err();
        assert_eq!(error, ProviderError::MalformedResponse);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_and_cancellation_are_deterministic() {
        let timeout_server = FakeServer::respond(
            "200 OK",
            &[("Content-Type", "application/json")],
            "{\"choices\":[{\"message\":{\"content\":\"late\"}}]}",
            Duration::from_millis(100),
        );
        let (timeout_registry, id) = registry(&timeout_server);
        let timeout = timeout_registry
            .start(&id, request(10), Arc::new(|_| {}))
            .await
            .unwrap()
            .completion()
            .await
            .unwrap_err();
        assert_eq!(timeout, ProviderError::Timeout);

        let cancel_server = FakeServer::respond(
            "200 OK",
            &[("Content-Type", "application/json")],
            "{\"choices\":[{\"message\":{\"content\":\"late\"}}]}",
            Duration::from_millis(100),
        );
        let (cancel_registry, id) = registry(&cancel_server);
        let run = cancel_registry
            .start(&id, request(1_000), Arc::new(|_| {}))
            .await
            .unwrap();
        cancel_server.wait_for_request();
        run.cancellation.cancel();
        assert_eq!(
            run.completion().await.unwrap_err(),
            ProviderError::Cancelled
        );
    }

    #[test]
    fn production_model_ids_come_from_documented_glm_set() {
        assert_eq!(
            GlmAdapter::new(DEFAULT_GLM_MODEL).unwrap().config.model_id,
            "glm-5.2"
        );
        assert!(GlmAdapter::new("invented-glm").is_err());
    }
}
