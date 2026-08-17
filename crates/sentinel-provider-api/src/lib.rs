//! Provider-neutral request, capability, credential, and routing boundary.
//!
//! Sentinel's workflow remains authoritative. Providers produce normalized
//! evidence and tool requests; provider completion never completes a task or
//! authorizes an action. Managed CLI providers can participate in the same
//! capability registry without replacing their proven lifecycle adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use thiserror::Error;
use tokio::{sync::Notify, task::JoinHandle};

pub mod agent;
pub mod gemini;
pub mod glm;
pub mod kimi;
mod openai_chat;
pub mod openai_compatible;
pub mod settings;

pub const CODEX_PROVIDER_ID: &str = "codex";
pub const CLAUDE_PROVIDER_ID: &str = "claude_code";
pub const DEFAULT_KEYCHAIN_SERVICE: &str = "dev.agent-sentinel.api-provider";
pub const MAX_PERSISTED_DIAGNOSTIC_BYTES: usize = 4_096;

pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte)
            })
        {
            return Err(ProviderError::InvalidConfiguration(
                "provider ID must use 1-64 lowercase ASCII letters, digits, '-', '_', or '.'"
                    .into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransport {
    ManagedCli,
    Api,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub cancellation: bool,
    pub tool_calling: bool,
    pub structured_output: bool,
    pub model_selection: bool,
    pub rate_limits: bool,
    pub implementation: bool,
    pub read_only_review: bool,
    pub repair: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelLimits {
    pub context_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialReference {
    pub service: String,
    pub account: String,
}

impl CredentialReference {
    pub fn for_provider(provider_id: &ProviderId) -> Self {
        Self {
            service: DEFAULT_KEYCHAIN_SERVICE.into(),
            account: provider_id.to_string(),
        }
    }

    fn validate(&self) -> Result<(), CredentialError> {
        if self.service.is_empty()
            || self.service.len() > 128
            || self.account.is_empty()
            || self.account.len() > 128
            || self.service.contains('\0')
            || self.account.contains('\0')
        {
            return Err(CredentialError::InvalidReference);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub display_name: String,
    pub model_id: String,
    pub base_url: Option<String>,
    pub enabled: bool,
    pub transport: ProviderTransport,
    pub capabilities: ProviderCapabilities,
    pub limits: ModelLimits,
    pub credential: Option<CredentialReference>,
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), ProviderError> {
        ProviderId::new(self.id.to_string())?;
        if self.display_name.trim().is_empty()
            || self.display_name.len() > 80
            || self.model_id.trim().is_empty()
            || self.model_id.len() > 128
            || self.model_id.contains('\0')
        {
            return Err(ProviderError::InvalidConfiguration(
                "provider display name or model ID is invalid".into(),
            ));
        }
        if self.transport == ProviderTransport::Api && self.credential.is_none() {
            return Err(ProviderError::InvalidConfiguration(
                "API providers require an opaque credential reference".into(),
            ));
        }
        if let Some(reference) = &self.credential {
            reference.validate().map_err(ProviderError::Credential)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole {
    Implementer,
    Reviewer,
    Repair,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSelection {
    pub provider_id: ProviderId,
    pub model_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowProviderConfiguration {
    pub implementer: ProviderSelection,
    pub reviewer: ProviderSelection,
    pub repair: Option<ProviderSelection>,
}

impl WorkflowProviderConfiguration {
    pub fn repair_selection(&self) -> &ProviderSelection {
        self.repair.as_ref().unwrap_or(&self.implementer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderMessage {
    pub role: MessageRole,
    pub content: String,
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub strict: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequest {
    pub request_id: String,
    pub model_id: String,
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<ToolDefinition>,
    pub response_format: ResponseFormat,
    pub max_output_tokens: Option<u64>,
    pub timeout_ms: u64,
}

impl ProviderRequest {
    pub fn validate(&self, config: &ProviderConfig) -> Result<(), ProviderError> {
        if self.request_id.len() < 6
            || self.request_id.len() > 128
            || self.request_id.contains('\0')
            || self.messages.is_empty()
            || self.messages.len() > 1_024
            || self.timeout_ms == 0
            || self.timeout_ms > 3_600_000
            || self.model_id != config.model_id
        {
            return Err(ProviderError::InvalidRequest(
                "request identity, model, messages, or timeout is invalid".into(),
            ));
        }
        if !self.tools.is_empty() && !config.capabilities.tool_calling {
            return Err(ProviderError::UnsupportedCapability("tool_calling"));
        }
        if self.response_format == ResponseFormat::JsonObject
            && !config.capabilities.structured_output
        {
            return Err(ProviderError::UnsupportedCapability("structured_output"));
        }
        if let (Some(requested), Some(limit)) =
            (self.max_output_tokens, config.limits.max_output_tokens)
        {
            if requested == 0 || requested > limit {
                return Err(ProviderError::InvalidRequest(
                    "requested output limit exceeds provider configuration".into(),
                ));
            }
        }
        for message in &self.messages {
            if message.content.len() > 2 * 1024 * 1024 || message.content.contains('\0') {
                return Err(ProviderError::InvalidRequest(
                    "message content is invalid or too large".into(),
                ));
            }
            if message.role != MessageRole::Assistant && !message.tool_calls.is_empty() {
                return Err(ProviderError::InvalidRequest(
                    "only assistant messages may contain tool calls".into(),
                ));
            }
            if message.role == MessageRole::Tool
                && message
                    .tool_call_id
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
            {
                return Err(ProviderError::InvalidRequest(
                    "tool results require a tool call ID".into(),
                ));
            }
        }
        for tool in &self.tools {
            if tool.name.is_empty()
                || tool.name.len() > 64
                || !tool
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                return Err(ProviderError::InvalidRequest("tool name is invalid".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitMetadata {
    pub request_limit: Option<u64>,
    pub request_remaining: Option<u64>,
    pub token_limit: Option<u64>,
    pub token_remaining: Option<u64>,
    pub reset_at: Option<String>,
    pub retry_after_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEventKind {
    Started,
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    RateLimits {
        metadata: RateLimitMetadata,
    },
    Completed {
        finish_reason: Option<String>,
        usage: TokenUsage,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderEvent {
    pub request_id: String,
    pub sequence: u64,
    pub event: ProviderEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCompletion {
    pub provider_id: ProviderId,
    pub request_id: String,
    pub model_id: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: TokenUsage,
    pub rate_limits: Option<RateLimitMetadata>,
}

#[derive(Debug)]
pub struct ProviderRun {
    pub cancellation: CancellationHandle,
    completion: JoinHandle<Result<ProviderCompletion, ProviderError>>,
}

impl ProviderRun {
    pub fn new(
        cancellation: CancellationHandle,
        completion: JoinHandle<Result<ProviderCompletion, ProviderError>>,
    ) -> Self {
        Self {
            cancellation,
            completion,
        }
    }

    pub async fn completion(self) -> Result<ProviderCompletion, ProviderError> {
        self.completion
            .await
            .map_err(|_| ProviderError::Unavailable("provider worker stopped".into()))?
    }
}

#[derive(Clone, Debug)]
pub struct CancellationHandle {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Default for CancellationHandle {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl CancellationHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }
}

pub trait ProviderEventSink: Send + Sync {
    fn emit(&self, event: ProviderEvent);
}

impl<F> ProviderEventSink for F
where
    F: Fn(ProviderEvent) + Send + Sync,
{
    fn emit(&self, event: ProviderEvent) {
        self(event)
    }
}

pub trait ProviderAdapter: Send + Sync {
    fn config(&self) -> &ProviderConfig;

    fn start<'a>(
        &'a self,
        request: ProviderRequest,
        credential: SecretString,
        events: Arc<dyn ProviderEventSink>,
    ) -> ProviderFuture<'a, ProviderRun>;
}

struct RegistryEntry {
    config: ProviderConfig,
    adapter: Option<Arc<dyn ProviderAdapter>>,
}

pub struct ProviderRegistry {
    entries: HashMap<ProviderId, RegistryEntry>,
    credentials: Arc<dyn CredentialStore>,
}

impl ProviderRegistry {
    pub fn new(credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            entries: HashMap::new(),
            credentials,
        }
    }

    pub fn with_managed_cli_providers(
        credentials: Arc<dyn CredentialStore>,
        codex_available: bool,
        claude_available: bool,
    ) -> Self {
        let mut registry = Self::new(credentials);
        registry
            .register_managed(managed_codex_config(codex_available))
            .expect("built-in Codex descriptor is valid");
        registry
            .register_managed(managed_claude_config(claude_available))
            .expect("built-in Claude descriptor is valid");
        registry
    }

    pub fn register_managed(&mut self, config: ProviderConfig) -> Result<(), ProviderError> {
        if config.transport != ProviderTransport::ManagedCli {
            return Err(ProviderError::InvalidConfiguration(
                "managed registration requires managed_cli transport".into(),
            ));
        }
        self.register_entry(config, None)
    }

    pub fn register_api(&mut self, adapter: Arc<dyn ProviderAdapter>) -> Result<(), ProviderError> {
        let config = adapter.config().clone();
        if config.transport != ProviderTransport::Api {
            return Err(ProviderError::InvalidConfiguration(
                "API registration requires api transport".into(),
            ));
        }
        self.register_entry(config, Some(adapter))
    }

    fn register_entry(
        &mut self,
        config: ProviderConfig,
        adapter: Option<Arc<dyn ProviderAdapter>>,
    ) -> Result<(), ProviderError> {
        config.validate()?;
        if self.entries.contains_key(&config.id) {
            return Err(ProviderError::DuplicateProvider(config.id.to_string()));
        }
        self.entries
            .insert(config.id.clone(), RegistryEntry { config, adapter });
        Ok(())
    }

    pub fn providers(&self) -> Vec<ProviderConfig> {
        let mut providers = self
            .entries
            .values()
            .map(|entry| entry.config.clone())
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        providers
    }

    pub fn provider(&self, id: &ProviderId) -> Option<&ProviderConfig> {
        self.entries.get(id).map(|entry| &entry.config)
    }

    pub fn credential_state(&self, id: &ProviderId) -> CredentialState {
        let Some(reference) = self
            .provider(id)
            .and_then(|config| config.credential.as_ref())
        else {
            return CredentialState::NotRequired;
        };
        self.credentials.state(reference)
    }

    pub fn set_credential(
        &self,
        id: &ProviderId,
        secret: SecretString,
    ) -> Result<(), ProviderError> {
        let reference = self
            .provider(id)
            .and_then(|config| config.credential.as_ref())
            .ok_or_else(|| ProviderError::InvalidConfiguration("credential is not used".into()))?;
        self.credentials
            .set(reference, secret)
            .map_err(ProviderError::Credential)
    }

    pub fn remove_credential(&self, id: &ProviderId) -> Result<(), ProviderError> {
        let reference = self
            .provider(id)
            .and_then(|config| config.credential.as_ref())
            .ok_or_else(|| ProviderError::InvalidConfiguration("credential is not used".into()))?;
        self.credentials
            .remove(reference)
            .map_err(ProviderError::Credential)
    }

    pub fn validate_selection(
        &self,
        selection: &ProviderSelection,
        role: ProviderRole,
    ) -> Result<(), ProviderError> {
        let config = self
            .provider(&selection.provider_id)
            .ok_or_else(|| ProviderError::ProviderUnavailable(selection.provider_id.to_string()))?;
        if !config.enabled || selection.model_id != config.model_id {
            return Err(ProviderError::ProviderUnavailable(
                selection.provider_id.to_string(),
            ));
        }
        let supported = match role {
            ProviderRole::Implementer => config.capabilities.implementation,
            ProviderRole::Reviewer => config.capabilities.read_only_review,
            ProviderRole::Repair => config.capabilities.repair,
        };
        if !supported {
            return Err(ProviderError::RoleUnavailable(role));
        }
        if config.transport == ProviderTransport::Api
            && self.credential_state(&config.id) != CredentialState::Configured
        {
            return Err(ProviderError::Credential(CredentialError::Missing));
        }
        Ok(())
    }

    pub fn validate_workflow_configuration(
        &self,
        configuration: &WorkflowProviderConfiguration,
    ) -> Result<(), ProviderError> {
        self.validate_selection(&configuration.implementer, ProviderRole::Implementer)?;
        self.validate_selection(&configuration.reviewer, ProviderRole::Reviewer)?;
        self.validate_selection(configuration.repair_selection(), ProviderRole::Repair)
    }

    pub async fn start(
        &self,
        id: &ProviderId,
        request: ProviderRequest,
        events: Arc<dyn ProviderEventSink>,
    ) -> Result<ProviderRun, ProviderError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| ProviderError::ProviderUnavailable(id.to_string()))?;
        if !entry.config.enabled {
            return Err(ProviderError::ProviderUnavailable(id.to_string()));
        }
        request.validate(&entry.config)?;
        let adapter = entry
            .adapter
            .as_ref()
            .ok_or_else(|| ProviderError::ManagedLifecycle(id.to_string()))?;
        let reference =
            entry.config.credential.as_ref().ok_or_else(|| {
                ProviderError::InvalidConfiguration("credential is missing".into())
            })?;
        let credential = self
            .credentials
            .get(reference)
            .map_err(ProviderError::Credential)?;
        adapter.start(request, credential, events).await
    }
}

pub fn managed_codex_config(available: bool) -> ProviderConfig {
    ProviderConfig {
        id: ProviderId::new(CODEX_PROVIDER_ID).expect("constant provider ID"),
        display_name: "Codex".into(),
        model_id: "cli-owned".into(),
        base_url: None,
        enabled: available,
        transport: ProviderTransport::ManagedCli,
        capabilities: ProviderCapabilities {
            streaming: true,
            cancellation: true,
            tool_calling: true,
            structured_output: true,
            model_selection: false,
            rate_limits: true,
            implementation: true,
            read_only_review: true,
            repair: true,
        },
        limits: ModelLimits {
            context_tokens: None,
            max_output_tokens: None,
        },
        credential: None,
    }
}

pub fn managed_claude_config(available: bool) -> ProviderConfig {
    ProviderConfig {
        id: ProviderId::new(CLAUDE_PROVIDER_ID).expect("constant provider ID"),
        display_name: "Claude Code".into(),
        model_id: "cli-owned".into(),
        base_url: None,
        enabled: available,
        transport: ProviderTransport::ManagedCli,
        capabilities: ProviderCapabilities {
            streaming: true,
            cancellation: true,
            tool_calling: true,
            structured_output: true,
            model_selection: false,
            rate_limits: false,
            implementation: true,
            read_only_review: true,
            repair: true,
        },
        limits: ModelLimits {
            context_tokens: None,
            max_output_tokens: None,
        },
        credential: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    NotRequired,
    Configured,
    Missing,
    Invalid,
}

pub struct SecretString(Vec<u8>);

impl SecretString {
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self, CredentialError> {
        let value = value.into();
        if value.is_empty() || value.len() > 16 * 1024 || value.contains(&0) {
            return Err(CredentialError::InvalidSecret);
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            // Volatile writes keep credential erasure observable to the
            // optimizer without introducing a second secret-handling crate.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

pub trait CredentialStore: Send + Sync {
    fn state(&self, reference: &CredentialReference) -> CredentialState;
    fn get(&self, reference: &CredentialReference) -> Result<SecretString, CredentialError>;
    fn set(
        &self,
        reference: &CredentialReference,
        secret: SecretString,
    ) -> Result<(), CredentialError>;
    fn remove(&self, reference: &CredentialReference) -> Result<(), CredentialError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("credential is missing")]
    Missing,
    #[error("credential reference is invalid")]
    InvalidReference,
    #[error("credential value is invalid")]
    InvalidSecret,
    #[error("secure credential storage is unavailable")]
    SecureStorageUnavailable,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider configuration is invalid: {0}")]
    InvalidConfiguration(String),
    #[error("provider request is invalid: {0}")]
    InvalidRequest(String),
    #[error("provider is unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("provider is already registered: {0}")]
    DuplicateProvider(String),
    #[error("managed provider lifecycle remains owned by its existing adapter: {0}")]
    ManagedLifecycle(String),
    #[error("provider role is unavailable: {0:?}")]
    RoleUnavailable(ProviderRole),
    #[error("provider capability is unsupported: {0}")]
    UnsupportedCapability(&'static str),
    #[error("provider authentication failed")]
    Authentication,
    #[error("provider rate limit was reached")]
    RateLimited(RateLimitMetadata),
    #[error("provider response was malformed")]
    MalformedResponse,
    #[error("provider request timed out")]
    Timeout,
    #[error("provider request was cancelled")]
    Cancelled,
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
    #[error("provider transport failed: {0}")]
    Transport(String),
    #[error(transparent)]
    Credential(#[from] CredentialError),
}

pub fn bounded_redacted_diagnostic(input: &str, secrets: &[&[u8]]) -> String {
    let mut output = input
        .chars()
        .take(MAX_PERSISTED_DIAGNOSTIC_BYTES)
        .collect::<String>();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        if let Ok(secret) = std::str::from_utf8(secret) {
            output = output.replace(secret, "[REDACTED]");
        }
    }
    output = redact_named_fields(output);
    output
}

fn redact_named_fields(mut input: String) -> String {
    const MARKERS: [&str; 7] = [
        "authorization:",
        "authorization=",
        "api_key=",
        "api-key=",
        "apikey=",
        "token=",
        "bearer ",
    ];
    let mut offset = 0;
    while offset < input.len() {
        let lowercase = input[offset..].to_ascii_lowercase();
        let Some((position, marker)) = MARKERS
            .iter()
            .filter_map(|marker| lowercase.find(marker).map(|position| (position, *marker)))
            .min_by_key(|(position, _)| *position)
        else {
            break;
        };
        let value_start = offset + position + marker.len();
        let value_end = input[value_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, ',' | '}' | '"')
            })
            .map(|end| value_start + end)
            .unwrap_or(input.len());
        input.replace_range(value_start..value_end, "[REDACTED]");
        offset = value_start + "[REDACTED]".len();
    }
    input
}

/// macOS Keychain implementation. Only the opaque reference is serializable;
/// plaintext credentials enter Rust transiently and never appear in config.
#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsKeychainCredentialStore;

#[cfg(target_os = "macos")]
mod macos_keychain {
    use super::{CredentialError, CredentialReference, CredentialState, SecretString};
    use std::{ffi::c_void, ptr};

    type OSStatus = i32;
    type SecKeychainItemRef = *mut c_void;
    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_name_length: u32,
            service_name: *const c_void,
            account_name_length: u32,
            account_name: *const c_void,
            password_length: u32,
            password_data: *const c_void,
            item_ref: *mut SecKeychainItemRef,
        ) -> OSStatus;
        fn SecKeychainFindGenericPassword(
            keychain_or_array: *const c_void,
            service_name_length: u32,
            service_name: *const c_void,
            account_name_length: u32,
            account_name: *const c_void,
            password_length: *mut u32,
            password_data: *mut *mut c_void,
            item_ref: *mut SecKeychainItemRef,
        ) -> OSStatus;
        fn SecKeychainItemModifyAttributesAndData(
            item_ref: SecKeychainItemRef,
            attributes: *const c_void,
            length: u32,
            data: *const c_void,
        ) -> OSStatus;
        fn SecKeychainItemDelete(item_ref: SecKeychainItemRef) -> OSStatus;
        fn SecKeychainItemFreeContent(attributes: *const c_void, data: *mut c_void) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const c_void);
    }

    fn find(
        reference: &CredentialReference,
        include_secret: bool,
    ) -> Result<(Option<SecKeychainItemRef>, Option<SecretString>), CredentialError> {
        reference.validate()?;
        let service = reference.service.as_bytes();
        let account = reference.account.as_bytes();
        let mut item = ptr::null_mut();
        let mut length = 0u32;
        let mut data = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(),
                service.len() as u32,
                service.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                if include_secret {
                    &mut length
                } else {
                    ptr::null_mut()
                },
                if include_secret {
                    &mut data
                } else {
                    ptr::null_mut()
                },
                &mut item,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok((None, None));
        }
        if status != ERR_SEC_SUCCESS {
            return Err(CredentialError::SecureStorageUnavailable);
        }
        let secret = if include_secret {
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) };
            let secret = SecretString::new(bytes.to_vec())?;
            let free_status = unsafe { SecKeychainItemFreeContent(ptr::null(), data) };
            if free_status != ERR_SEC_SUCCESS {
                if !item.is_null() {
                    unsafe { CFRelease(item.cast()) };
                }
                return Err(CredentialError::SecureStorageUnavailable);
            }
            Some(secret)
        } else {
            None
        };
        Ok((Some(item), secret))
    }

    fn release(item: SecKeychainItemRef) {
        if !item.is_null() {
            unsafe { CFRelease(item.cast()) };
        }
    }

    pub(super) fn state(reference: &CredentialReference) -> CredentialState {
        match find(reference, false) {
            Ok((Some(item), _)) => {
                release(item);
                CredentialState::Configured
            }
            Ok((None, _)) => CredentialState::Missing,
            Err(_) => CredentialState::Invalid,
        }
    }

    pub(super) fn get(reference: &CredentialReference) -> Result<SecretString, CredentialError> {
        let (item, secret) = find(reference, true)?;
        if let Some(item) = item {
            release(item);
        }
        secret.ok_or(CredentialError::Missing)
    }

    pub(super) fn set(
        reference: &CredentialReference,
        secret: SecretString,
    ) -> Result<(), CredentialError> {
        reference.validate()?;
        let (item, _) = find(reference, false)?;
        let status = if let Some(item) = item {
            let status = unsafe {
                SecKeychainItemModifyAttributesAndData(
                    item,
                    ptr::null(),
                    secret.expose().len() as u32,
                    secret.expose().as_ptr().cast(),
                )
            };
            release(item);
            status
        } else {
            unsafe {
                SecKeychainAddGenericPassword(
                    ptr::null(),
                    reference.service.len() as u32,
                    reference.service.as_ptr().cast(),
                    reference.account.len() as u32,
                    reference.account.as_ptr().cast(),
                    secret.expose().len() as u32,
                    secret.expose().as_ptr().cast(),
                    ptr::null_mut(),
                )
            }
        };
        if status == ERR_SEC_SUCCESS {
            Ok(())
        } else {
            Err(CredentialError::SecureStorageUnavailable)
        }
    }

    pub(super) fn remove(reference: &CredentialReference) -> Result<(), CredentialError> {
        let (item, _) = find(reference, false)?;
        let Some(item) = item else {
            return Ok(());
        };
        let status = unsafe { SecKeychainItemDelete(item) };
        release(item);
        if status == ERR_SEC_SUCCESS || status == ERR_SEC_ITEM_NOT_FOUND {
            Ok(())
        } else {
            Err(CredentialError::SecureStorageUnavailable)
        }
    }
}

impl CredentialStore for MacOsKeychainCredentialStore {
    fn state(&self, reference: &CredentialReference) -> CredentialState {
        #[cfg(target_os = "macos")]
        {
            macos_keychain::state(reference)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reference;
            CredentialState::Invalid
        }
    }

    fn get(&self, reference: &CredentialReference) -> Result<SecretString, CredentialError> {
        #[cfg(target_os = "macos")]
        {
            macos_keychain::get(reference)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reference;
            Err(CredentialError::SecureStorageUnavailable)
        }
    }

    fn set(
        &self,
        reference: &CredentialReference,
        secret: SecretString,
    ) -> Result<(), CredentialError> {
        #[cfg(target_os = "macos")]
        {
            macos_keychain::set(reference, secret)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (reference, secret);
            Err(CredentialError::SecureStorageUnavailable)
        }
    }

    fn remove(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        #[cfg(target_os = "macos")]
        {
            macos_keychain::remove(reference)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reference;
            Err(CredentialError::SecureStorageUnavailable)
        }
    }
}

#[derive(Default)]
pub struct MemoryCredentialStore {
    values: std::sync::Mutex<HashMap<CredentialReferenceKey, Vec<u8>>>,
    invalid: std::sync::Mutex<HashSet<CredentialReferenceKey>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CredentialReferenceKey {
    service: String,
    account: String,
}

impl From<&CredentialReference> for CredentialReferenceKey {
    fn from(reference: &CredentialReference) -> Self {
        Self {
            service: reference.service.clone(),
            account: reference.account.clone(),
        }
    }
}

impl MemoryCredentialStore {
    pub fn mark_invalid(&self, reference: &CredentialReference) {
        self.invalid
            .lock()
            .expect("credential test store lock")
            .insert(reference.into());
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn state(&self, reference: &CredentialReference) -> CredentialState {
        let key = reference.into();
        if self
            .invalid
            .lock()
            .expect("credential test store lock")
            .contains(&key)
        {
            CredentialState::Invalid
        } else if self
            .values
            .lock()
            .expect("credential test store lock")
            .contains_key(&key)
        {
            CredentialState::Configured
        } else {
            CredentialState::Missing
        }
    }

    fn get(&self, reference: &CredentialReference) -> Result<SecretString, CredentialError> {
        if self.state(reference) == CredentialState::Invalid {
            return Err(CredentialError::SecureStorageUnavailable);
        }
        self.values
            .lock()
            .expect("credential test store lock")
            .get(&reference.into())
            .cloned()
            .ok_or(CredentialError::Missing)
            .and_then(SecretString::new)
    }

    fn set(
        &self,
        reference: &CredentialReference,
        secret: SecretString,
    ) -> Result<(), CredentialError> {
        reference.validate()?;
        if self.state(reference) == CredentialState::Invalid {
            return Err(CredentialError::SecureStorageUnavailable);
        }
        self.values
            .lock()
            .expect("credential test store lock")
            .insert(reference.into(), secret.expose().to_vec());
        Ok(())
    }

    fn remove(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        if self.state(reference) == CredentialState::Invalid {
            return Err(CredentialError::SecureStorageUnavailable);
        }
        self.values
            .lock()
            .expect("credential test store lock")
            .remove(&reference.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeAdapter {
        config: ProviderConfig,
        starts: Arc<Mutex<Vec<String>>>,
    }

    impl ProviderAdapter for FakeAdapter {
        fn config(&self) -> &ProviderConfig {
            &self.config
        }

        fn start<'a>(
            &'a self,
            request: ProviderRequest,
            credential: SecretString,
            events: Arc<dyn ProviderEventSink>,
        ) -> ProviderFuture<'a, ProviderRun> {
            self.starts.lock().unwrap().push(request.request_id.clone());
            Box::pin(async move {
                assert_eq!(credential.expose(), b"secret");
                let cancellation = CancellationHandle::default();
                events.emit(ProviderEvent {
                    request_id: request.request_id.clone(),
                    sequence: 1,
                    event: ProviderEventKind::Started,
                });
                let provider_id = ProviderId::new("fake")?;
                let request_id = request.request_id;
                let model_id = request.model_id;
                let completion = tokio::spawn(async move {
                    Ok(ProviderCompletion {
                        provider_id,
                        request_id,
                        model_id,
                        text: "ok".into(),
                        reasoning: None,
                        tool_calls: Vec::new(),
                        finish_reason: Some("stop".into()),
                        usage: TokenUsage::default(),
                        rate_limits: None,
                    })
                });
                Ok(ProviderRun::new(cancellation, completion))
            })
        }
    }

    fn fake_config(enabled: bool) -> ProviderConfig {
        let id = ProviderId::new("fake").unwrap();
        ProviderConfig {
            id: id.clone(),
            display_name: "Fake API".into(),
            model_id: "fake-1".into(),
            base_url: Some("https://example.invalid/v1".into()),
            enabled,
            transport: ProviderTransport::Api,
            capabilities: ProviderCapabilities {
                streaming: true,
                cancellation: true,
                tool_calling: false,
                structured_output: false,
                model_selection: true,
                rate_limits: false,
                implementation: true,
                read_only_review: true,
                repair: true,
            },
            limits: ModelLimits {
                context_tokens: Some(1_000),
                max_output_tokens: Some(100),
            },
            credential: Some(CredentialReference::for_provider(&id)),
        }
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            request_id: "request-1".into(),
            model_id: "fake-1".into(),
            messages: vec![ProviderMessage {
                role: MessageRole::User,
                content: "hello".into(),
                tool_call_id: None,
                name: None,
                tool_calls: Vec::new(),
            }],
            tools: Vec::new(),
            response_format: ResponseFormat::Text,
            max_output_tokens: Some(10),
            timeout_ms: 1_000,
        }
    }

    #[tokio::test]
    async fn registry_routes_api_provider_without_cli_branching() {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let starts = Arc::new(Mutex::new(Vec::new()));
        let adapter = Arc::new(FakeAdapter {
            config: fake_config(true),
            starts: starts.clone(),
        });
        let id = adapter.config.id.clone();
        credentials
            .set(
                adapter.config.credential.as_ref().unwrap(),
                SecretString::new("secret").unwrap(),
            )
            .unwrap();
        let mut registry = ProviderRegistry::with_managed_cli_providers(credentials, true, false);
        registry.register_api(adapter).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        let run = registry
            .start(
                &id,
                request(),
                Arc::new(move |event| sink_seen.lock().unwrap().push(event)),
            )
            .await
            .unwrap();
        assert_eq!(run.completion().await.unwrap().text, "ok");
        assert_eq!(starts.lock().unwrap().as_slice(), ["request-1"]);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(matches!(
            registry
                .start(
                    &ProviderId::new("codex").unwrap(),
                    request(),
                    Arc::new(|_| {})
                )
                .await,
            Err(ProviderError::InvalidRequest(_)) | Err(ProviderError::ManagedLifecycle(_))
        ));
    }

    #[test]
    fn capability_and_role_differences_fail_closed() {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let registry = ProviderRegistry::with_managed_cli_providers(credentials, true, false);
        let codex = registry
            .provider(&ProviderId::new("codex").unwrap())
            .unwrap();
        let claude = registry
            .provider(&ProviderId::new("claude_code").unwrap())
            .unwrap();
        assert!(codex.capabilities.rate_limits);
        assert!(!claude.capabilities.rate_limits);
        assert!(!claude.enabled);
    }

    #[test]
    fn credential_store_supports_missing_replace_and_remove() {
        let store = MemoryCredentialStore::default();
        let reference = CredentialReference::for_provider(&ProviderId::new("glm").unwrap());
        assert_eq!(store.state(&reference), CredentialState::Missing);
        store
            .set(&reference, SecretString::new("first").unwrap())
            .unwrap();
        store
            .set(&reference, SecretString::new("replacement").unwrap())
            .unwrap();
        assert_eq!(store.get(&reference).unwrap().expose(), b"replacement");
        store.remove(&reference).unwrap();
        assert_eq!(store.state(&reference), CredentialState::Missing);
        assert_eq!(store.get(&reference).unwrap_err(), CredentialError::Missing);
    }

    #[test]
    fn serialized_provider_config_contains_only_credential_reference() {
        let serialized = serde_json::to_string(&fake_config(true)).unwrap();
        assert!(serialized.contains(DEFAULT_KEYCHAIN_SERVICE));
        assert!(!serialized.contains("secret-value"));
        assert_eq!(
            format!("{:?}", SecretString::new("secret-value").unwrap()),
            "SecretString([REDACTED])"
        );
    }

    #[test]
    fn diagnostics_redact_secret_headers_and_are_bounded() {
        let diagnostic = bounded_redacted_diagnostic(
            "Authorization: Bearer top-secret api_key=second token=third",
            &[b"top-secret"],
        );
        assert!(!diagnostic.contains("top-secret"));
        assert!(!diagnostic.contains("second"));
        assert!(!diagnostic.contains("third"));
        assert!(diagnostic.contains("[REDACTED]"));
        assert!(bounded_redacted_diagnostic(&"x".repeat(10_000), &[]).len() <= 4_096);
    }

    #[test]
    fn workflow_selection_uses_registry_capabilities() {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let mut registry = ProviderRegistry::with_managed_cli_providers(credentials, true, true);
        let mut config = fake_config(true);
        config.capabilities.repair = false;
        let adapter = Arc::new(FakeAdapter {
            config,
            starts: Arc::new(Mutex::new(Vec::new())),
        });
        registry.register_api(adapter).unwrap();
        let configuration = WorkflowProviderConfiguration {
            implementer: ProviderSelection {
                provider_id: ProviderId::new("codex").unwrap(),
                model_id: "cli-owned".into(),
            },
            reviewer: ProviderSelection {
                provider_id: ProviderId::new("claude_code").unwrap(),
                model_id: "cli-owned".into(),
            },
            repair: Some(ProviderSelection {
                provider_id: ProviderId::new("fake").unwrap(),
                model_id: "fake-1".into(),
            }),
        };
        assert_eq!(
            registry
                .validate_workflow_configuration(&configuration)
                .unwrap_err(),
            ProviderError::RoleUnavailable(ProviderRole::Repair)
        );
    }
}
