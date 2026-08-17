//! Moonshot Kimi adapter using the official Chat Completions API.
//!
//! Wire reference: <https://platform.kimi.ai/docs/api/chat>

use crate::{
    openai_chat::{api_config, execute, OpenAiDialect},
    CancellationHandle, ModelLimits, ProviderAdapter, ProviderCapabilities, ProviderConfig,
    ProviderError, ProviderEventSink, ProviderFuture, ProviderRequest, ProviderRun, SecretString,
};
use reqwest::Client;
use std::sync::Arc;

pub const DEFAULT_KIMI_BASE_URL: &str = "https://api.moonshot.ai/v1";
pub const DEFAULT_KIMI_MODEL: &str = "kimi-k3";
pub const SUPPORTED_KIMI_MODELS: &[&str] = &[
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
    "kimi-k2.5",
    "moonshot-v1",
];

pub struct KimiAdapter {
    config: ProviderConfig,
    client: Client,
}

impl KimiAdapter {
    pub fn new(model_id: &str) -> Result<Self, ProviderError> {
        Self::with_base_url(model_id, DEFAULT_KIMI_BASE_URL)
    }

    pub fn with_base_url(model_id: &str, base_url: &str) -> Result<Self, ProviderError> {
        if !SUPPORTED_KIMI_MODELS.contains(&model_id) && !is_loopback(base_url) {
            return Err(ProviderError::InvalidConfiguration(
                "unsupported Kimi model ID".into(),
            ));
        }
        let config = api_config(
            "kimi",
            "Kimi",
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
                // The official K3 reference documents up to 1,048,576 output
                // tokens. Other models remain provider-validated rather than
                // inheriting a guessed Sentinel-side ceiling.
                max_output_tokens: (model_id == "kimi-k3").then_some(1_048_576),
            },
        )?;
        Self::from_config(config)
    }

    pub fn from_config(config: ProviderConfig) -> Result<Self, ProviderError> {
        if config.id.as_str() != "kimi"
            || config.transport != crate::ProviderTransport::Api
            || (!SUPPORTED_KIMI_MODELS.contains(&config.model_id.as_str())
                && !config.base_url.as_deref().is_some_and(is_loopback))
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Kimi provider configuration".into(),
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

impl ProviderAdapter for KimiAdapter {
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
                    OpenAiDialect::Kimi,
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
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    struct FakeServer {
        url: String,
        request: Arc<Mutex<Option<String>>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn respond(status: &str, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let request = Arc::new(Mutex::new(None));
            let captured = request.clone();
            let status = status.to_owned();
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
                        let header_text = String::from_utf8_lossy(&bytes[..header_end]);
                        let content_length = header_text
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
                *captured.lock().unwrap() = Some(String::from_utf8_lossy(&bytes).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

        fn captured(&self) -> String {
            self.request.lock().unwrap().clone().unwrap_or_default()
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

    fn request() -> ProviderRequest {
        ProviderRequest {
            request_id: "kimi-request-1".into(),
            model_id: "test-kimi".into(),
            messages: vec![ProviderMessage {
                role: MessageRole::User,
                content: "hello".into(),
                tool_call_id: None,
                name: None,
                tool_calls: Vec::new(),
            }],
            tools: vec![ToolDefinition {
                name: "inspect".into(),
                description: "Inspect approved evidence".into(),
                parameters: json!({"type":"object"}),
                strict: Some(true),
            }],
            response_format: ResponseFormat::JsonObject,
            max_output_tokens: Some(128),
            timeout_ms: 2_000,
        }
    }

    fn registry(server: &FakeServer) -> (ProviderRegistry, ProviderId) {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let adapter = Arc::new(KimiAdapter::with_base_url("test-kimi", &server.url).unwrap());
        let id = adapter.config.id.clone();
        credentials
            .set(
                adapter.config.credential.as_ref().unwrap(),
                SecretString::new("kimi-test-key").unwrap(),
            )
            .unwrap();
        let mut registry = ProviderRegistry::new(credentials);
        registry.register_api(adapter).unwrap();
        (registry, id)
    }

    #[tokio::test]
    async fn kimi_wire_and_streaming_follow_official_contract() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"consider\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"ok\\\":true}\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );
        let server = FakeServer::respond("200 OK", body);
        let (registry, id) = registry(&server);
        let events = Arc::new(Mutex::new(Vec::<ProviderEvent>::new()));
        let emitted = events.clone();
        let completion = registry
            .start(
                &id,
                request(),
                Arc::new(move |event| emitted.lock().unwrap().push(event)),
            )
            .await
            .unwrap()
            .completion()
            .await
            .unwrap();
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!(completion.reasoning.as_deref(), Some("consider"));
        assert_eq!(completion.usage.total_tokens, Some(5));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, ProviderEventKind::ReasoningDelta { .. })));
        let captured = server.captured();
        assert!(captured.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(captured.contains("\"max_completion_tokens\":128"));
        assert!(captured.contains("\"stream_options\":{\"include_usage\":true}"));
        assert!(captured.contains("\"strict\":true"));
        assert!(!captured.contains("tool_stream"));
    }

    #[tokio::test]
    async fn kimi_rate_limit_error_is_normalized() {
        let server = FakeServer::respond(
            "429 Too Many Requests",
            "{\"error\":{\"message\":\"limit for kimi-test-key\"}}",
        );
        let (registry, id) = registry(&server);
        let error = registry
            .start(&id, request(), Arc::new(|_| {}))
            .await
            .unwrap()
            .completion()
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::RateLimited(_)));
        assert!(!error.to_string().contains("kimi-test-key"));
    }

    #[test]
    fn production_model_ids_are_documented_kimi_models() {
        assert_eq!(
            KimiAdapter::new(DEFAULT_KIMI_MODEL)
                .unwrap()
                .config
                .model_id,
            "kimi-k3"
        );
        assert!(KimiAdapter::new("invented-kimi").is_err());
    }
}
