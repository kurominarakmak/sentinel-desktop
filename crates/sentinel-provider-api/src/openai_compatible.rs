//! Explicitly configured OpenAI-compatible Chat Completions adapter.
//!
//! Compatibility reference: <https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create>

use crate::{
    openai_chat::{api_config, execute, validate_api_base_url, OpenAiDialect},
    CancellationHandle, ModelLimits, ProviderAdapter, ProviderCapabilities, ProviderConfig,
    ProviderError, ProviderEventSink, ProviderFuture, ProviderRequest, ProviderRun, SecretString,
    CLAUDE_PROVIDER_ID, CODEX_PROVIDER_ID,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProviderSpec {
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    pub model_id: String,
    pub enabled: bool,
    pub capabilities: ProviderCapabilities,
    pub limits: ModelLimits,
}

impl CustomProviderSpec {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if !self.provider_id.starts_with("custom.")
            || matches!(
                self.provider_id.as_str(),
                CODEX_PROVIDER_ID | CLAUDE_PROVIDER_ID | "glm" | "kimi" | "gemini"
            )
        {
            return Err(ProviderError::InvalidConfiguration(
                "custom provider IDs must begin with 'custom.' and cannot replace built-ins".into(),
            ));
        }
        validate_api_base_url(&self.base_url)?;
        if !self.capabilities.streaming || !self.capabilities.cancellation {
            return Err(ProviderError::InvalidConfiguration(
                "custom providers must support Sentinel streaming and cancellation".into(),
            ));
        }
        Ok(())
    }
}

pub struct OpenAiCompatibleAdapter {
    config: ProviderConfig,
    client: Client,
}

impl OpenAiCompatibleAdapter {
    pub fn new(spec: CustomProviderSpec) -> Result<Self, ProviderError> {
        spec.validate()?;
        let mut config = api_config(
            &spec.provider_id,
            &spec.display_name,
            &spec.model_id,
            &spec.base_url,
            spec.capabilities,
            spec.limits,
        )?;
        config.enabled = spec.enabled;
        Self::from_config(config)
    }

    pub fn from_config(config: ProviderConfig) -> Result<Self, ProviderError> {
        let spec = CustomProviderSpec {
            provider_id: config.id.to_string(),
            display_name: config.display_name.clone(),
            base_url: config.base_url.clone().unwrap_or_default(),
            model_id: config.model_id.clone(),
            enabled: config.enabled,
            capabilities: config.capabilities,
            limits: config.limits.clone(),
        };
        spec.validate()?;
        config.validate()?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Unavailable("HTTP client unavailable".into()))?;
        Ok(Self { config, client })
    }
}

impl ProviderAdapter for OpenAiCompatibleAdapter {
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
                    OpenAiDialect::Compatible,
                )
                .await
            });
            Ok(ProviderRun::new(cancellation, completion))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CredentialStore, MemoryCredentialStore, MessageRole, ProviderMessage, ProviderRegistry,
        ResponseFormat,
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            cancellation: true,
            tool_calling: false,
            structured_output: false,
            model_selection: true,
            rate_limits: false,
            implementation: true,
            read_only_review: true,
            repair: true,
        }
    }

    fn spec(base_url: String) -> CustomProviderSpec {
        CustomProviderSpec {
            provider_id: "custom.local".into(),
            display_name: "Local Compatible".into(),
            base_url,
            model_id: "local-model".into(),
            enabled: true,
            capabilities: capabilities(),
            limits: ModelLimits {
                context_tokens: Some(32_000),
                max_output_tokens: Some(4_096),
            },
        }
    }

    #[test]
    fn custom_endpoint_validation_rejects_unsafe_configuration() {
        for url in [
            "file:///tmp/provider",
            "http://example.com/v1",
            "https://user:password@example.com/v1",
            "https://example.com/v1?api_key=secret",
            "https://example.com/v1#fragment",
        ] {
            assert!(
                OpenAiCompatibleAdapter::new(spec(url.into())).is_err(),
                "{url}"
            );
        }
        let mut reserved = spec("https://example.com/v1".into());
        reserved.provider_id = "codex".into();
        assert!(OpenAiCompatibleAdapter::new(reserved).is_err());
        let mut no_stream = spec("https://example.com/v1".into());
        no_stream.capabilities.streaming = false;
        assert!(OpenAiCompatibleAdapter::new(no_stream).is_err());
    }

    #[tokio::test]
    async fn custom_provider_routes_only_http_chat_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let server_capture = captured.clone();
        let server = thread::spawn(move || {
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
                if let Some(header_end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            *server_capture.lock().unwrap() = String::from_utf8_lossy(&bytes).into_owned();
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"custom\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let credentials = Arc::new(MemoryCredentialStore::default());
        let adapter =
            Arc::new(OpenAiCompatibleAdapter::new(spec(format!("http://{address}/v1"))).unwrap());
        let id = adapter.config.id.clone();
        credentials
            .set(
                adapter.config.credential.as_ref().unwrap(),
                SecretString::new("custom-key").unwrap(),
            )
            .unwrap();
        let mut registry = ProviderRegistry::new(credentials);
        registry.register_api(adapter).unwrap();
        let completion = registry
            .start(
                &id,
                ProviderRequest {
                    request_id: "custom-request".into(),
                    model_id: "local-model".into(),
                    messages: vec![ProviderMessage {
                        role: MessageRole::User,
                        content: "hello".into(),
                        tool_call_id: None,
                        name: None,
                    }],
                    tools: Vec::new(),
                    response_format: ResponseFormat::Text,
                    max_output_tokens: Some(100),
                    timeout_ms: 2_000,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap()
            .completion()
            .await
            .unwrap();
        assert_eq!(completion.text, "custom");
        server.join().unwrap();
        let request = captured.lock().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(request.contains("\"model\":\"local-model\""));
        assert!(!request.contains("executable"));
    }
}
