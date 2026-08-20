//! Bounded provider-neutral tool loop.
//!
//! The loop routes requests and returns provider claims. It owns no workflow
//! transitions, filesystem access, validation decisions, or approval power.

use crate::{
    CancellationHandle, MessageRole, ProviderCompletion, ProviderError, ProviderEventSink,
    ProviderFuture, ProviderId, ProviderMessage, ProviderRegistry, ProviderRequest,
    ProviderSelection, ResponseFormat, ToolCall, ToolDefinition,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_AGENT_ROUNDS: u8 = 16;
const MAX_TOOL_CALLS_PER_ROUND: usize = 32;
const MAX_TOOL_RESULT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLoopRequest {
    pub request_id: String,
    pub selection: ProviderSelection,
    pub role: crate::ProviderRole,
    pub system_prompt: String,
    pub user_prompt: String,
    pub response_format: ResponseFormat,
    pub max_output_tokens: Option<u64>,
    pub timeout_ms: u64,
    pub max_rounds: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolExecutionResult {
    pub content: String,
}

pub trait AgentToolExecutor: Send + Sync {
    fn definitions(&self) -> Vec<ToolDefinition>;

    fn execute<'a>(&'a self, call: &'a ToolCall) -> ProviderFuture<'a, ToolExecutionResult>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentLoopResult {
    pub provider_id: ProviderId,
    pub model_id: String,
    pub final_text: String,
    pub reasoning: Option<String>,
    pub rounds: u8,
    pub tool_calls: usize,
}

pub async fn run_agent_loop(
    registry: &ProviderRegistry,
    request: AgentLoopRequest,
    tools: Arc<dyn AgentToolExecutor>,
    events: Arc<dyn ProviderEventSink>,
    cancellation: CancellationHandle,
) -> Result<AgentLoopResult, ProviderError> {
    if request.request_id.len() < 6
        || request.request_id.len() > 100
        || request.max_rounds == 0
        || request.max_rounds > MAX_AGENT_ROUNDS
        || request.system_prompt.len() > 256 * 1024
        || request.user_prompt.len() > 2 * 1024 * 1024
    {
        return Err(ProviderError::InvalidRequest(
            "agent loop request is invalid".into(),
        ));
    }
    if request.role == crate::ProviderRole::Reviewer {
        return Err(ProviderError::InvalidRequest(
            "reviewers cannot enter the write-capable agent loop".into(),
        ));
    }
    registry.validate_selection(&request.selection, request.role)?;
    let definitions = tools.definitions();
    let mut messages = vec![
        ProviderMessage {
            role: MessageRole::System,
            content: request.system_prompt,
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        },
        ProviderMessage {
            role: MessageRole::User,
            content: request.user_prompt,
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        },
    ];
    let mut total_tool_calls = 0usize;
    for round in 1..=request.max_rounds {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let provider_request = ProviderRequest {
            request_id: format!("{}-{round:02}", request.request_id),
            model_id: request.selection.model_id.clone(),
            messages: messages.clone(),
            tools: definitions.clone(),
            response_format: request.response_format,
            max_output_tokens: request.max_output_tokens,
            timeout_ms: request.timeout_ms,
        };
        let run = registry
            .start(
                &request.selection.provider_id,
                provider_request,
                events.clone(),
            )
            .await?;
        let provider_cancellation = run.cancellation.clone();
        let completion = tokio::select! {
            _ = cancellation.cancelled() => {
                provider_cancellation.cancel();
                return Err(ProviderError::Cancelled);
            }
            completion = run.completion() => completion?,
        };
        if completion.tool_calls.is_empty() {
            return Ok(loop_result(completion, round, total_tool_calls));
        }
        if completion.tool_calls.len() > MAX_TOOL_CALLS_PER_ROUND {
            return Err(ProviderError::InvalidRequest(
                "provider returned too many tool calls".into(),
            ));
        }
        total_tool_calls = total_tool_calls.saturating_add(completion.tool_calls.len());
        messages.push(ProviderMessage {
            role: MessageRole::Assistant,
            content: completion.text,
            tool_call_id: None,
            name: None,
            tool_calls: completion.tool_calls.clone(),
        });
        for call in completion.tool_calls {
            let result = tools.execute(&call).await?;
            if result.content.len() > MAX_TOOL_RESULT_BYTES {
                return Err(ProviderError::InvalidRequest(
                    "tool result exceeds the bounded context limit".into(),
                ));
            }
            messages.push(ProviderMessage {
                role: MessageRole::Tool,
                content: result.content,
                tool_call_id: Some(call.id),
                name: Some(call.name),
                tool_calls: Vec::new(),
            });
        }
    }
    Err(ProviderError::Unavailable(
        "bounded agent round limit reached".into(),
    ))
}

fn loop_result(completion: ProviderCompletion, rounds: u8, tool_calls: usize) -> AgentLoopResult {
    AgentLoopResult {
        provider_id: completion.provider_id,
        model_id: completion.model_id,
        final_text: completion.text,
        reasoning: completion.reasoning,
        rounds,
        tool_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CredentialReference, CredentialStore, MemoryCredentialStore, ModelLimits, ProviderAdapter,
        ProviderCapabilities, ProviderConfig, ProviderRun, ProviderTransport, SecretString,
        TokenUsage,
    };
    use serde_json::json;
    use std::{collections::VecDeque, sync::Mutex};

    struct ScriptedAdapter {
        config: ProviderConfig,
        completions: Mutex<VecDeque<ProviderCompletion>>,
        requests: Arc<Mutex<Vec<ProviderRequest>>>,
    }

    impl ProviderAdapter for ScriptedAdapter {
        fn config(&self) -> &ProviderConfig {
            &self.config
        }

        fn start<'a>(
            &'a self,
            request: ProviderRequest,
            _credential: SecretString,
            _events: Arc<dyn ProviderEventSink>,
        ) -> ProviderFuture<'a, ProviderRun> {
            self.requests.lock().unwrap().push(request);
            let completion = self.completions.lock().unwrap().pop_front().unwrap();
            Box::pin(async move {
                Ok(ProviderRun::new(
                    CancellationHandle::default(),
                    tokio::spawn(async move { Ok(completion) }),
                ))
            })
        }
    }

    struct RecordingTools(Arc<Mutex<Vec<String>>>);

    impl AgentToolExecutor for RecordingTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "write_file".into(),
                description: "write one file".into(),
                parameters: json!({"type":"object"}),
                strict: Some(true),
            }]
        }

        fn execute<'a>(&'a self, call: &'a ToolCall) -> ProviderFuture<'a, ToolExecutionResult> {
            self.0.lock().unwrap().push(call.name.clone());
            Box::pin(async {
                Ok(ToolExecutionResult {
                    content: "{\"ok\":true}".into(),
                })
            })
        }
    }

    fn completion(text: &str, tool_calls: Vec<ToolCall>) -> ProviderCompletion {
        ProviderCompletion {
            provider_id: ProviderId::new("fake").unwrap(),
            request_id: "provider-request".into(),
            model_id: "fake-model".into(),
            text: text.into(),
            reasoning: None,
            tool_calls,
            finish_reason: Some("stop".into()),
            usage: TokenUsage::default(),
            rate_limits: None,
        }
    }

    #[tokio::test]
    async fn configured_implementer_runs_bounded_tools_in_order() {
        let id = ProviderId::new("fake").unwrap();
        let reference = CredentialReference::for_provider(&id);
        let credentials = Arc::new(MemoryCredentialStore::default());
        credentials
            .set(&reference, SecretString::new("test-key").unwrap())
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let adapter = Arc::new(ScriptedAdapter {
            config: ProviderConfig {
                id: id.clone(),
                display_name: "Fake".into(),
                model_id: "fake-model".into(),
                base_url: Some("https://example.invalid/v1".into()),
                enabled: true,
                transport: ProviderTransport::Api,
                capabilities: ProviderCapabilities {
                    streaming: true,
                    cancellation: true,
                    tool_calling: true,
                    structured_output: true,
                    model_selection: true,
                    rate_limits: false,
                    implementation: true,
                    read_only_review: true,
                    repair: true,
                },
                limits: ModelLimits {
                    context_tokens: None,
                    max_output_tokens: None,
                },
                credential: Some(reference),
            },
            completions: Mutex::new(VecDeque::from([
                completion(
                    "",
                    vec![ToolCall {
                        id: "call-1".into(),
                        name: "write_file".into(),
                        arguments: json!({"path":"src/lib.rs","content":"changed"}),
                    }],
                ),
                completion("done", Vec::new()),
            ])),
            requests: requests.clone(),
        });
        let mut registry = ProviderRegistry::new(credentials);
        registry.register_api(adapter).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let result = run_agent_loop(
            &registry,
            AgentLoopRequest {
                request_id: "task-test".into(),
                selection: ProviderSelection {
                    provider_id: id,
                    model_id: "fake-model".into(),
                    reasoning_effort: None,
                },
                role: crate::ProviderRole::Implementer,
                system_prompt: "implement".into(),
                user_prompt: "change it".into(),
                response_format: ResponseFormat::Text,
                max_output_tokens: None,
                timeout_ms: 1_000,
                max_rounds: 3,
            },
            Arc::new(RecordingTools(calls.clone())),
            Arc::new(|_| {}),
            CancellationHandle::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.final_text, "done");
        assert_eq!(result.rounds, 2);
        assert_eq!(calls.lock().unwrap().as_slice(), ["write_file"]);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].messages.last().unwrap().role, MessageRole::Tool);
        assert_eq!(requests[1].messages[2].tool_calls[0].id, "call-1");
    }
}
