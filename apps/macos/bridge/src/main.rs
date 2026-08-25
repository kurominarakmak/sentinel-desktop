//! Narrow, stdio-only service boundary for the native macOS shell.
//!
//! This process owns no workflow decisions. It maps durable V3 state to JSON
//! snapshots and forwards the already-existing final-approval supervisor call.

use sentinel_agent_api::{detect_installation, AgentKind, InstallationStatus};
use sentinel_claude::{ClaudeModelDiscovery, ClaudeProgram};
use sentinel_codex::{CodexAppServer, CodexModelDiscovery, CodexProgram};
use sentinel_core::{
    v3::{ApprovalId, CreateTask, SupervisorRequestReservation, TaskId, WorkflowMode},
    CoreError, RunRepository,
};
use sentinel_git::{inspect_repository, RepositoryState};
use sentinel_omp::{OmpModelDiscovery, OmpProgram};
use sentinel_provider_api::{
    openai_compatible::CustomProviderSpec,
    settings::{ProviderSettingsSnapshot, ProviderSettingsStore},
    MacOsKeychainCredentialStore, ProviderError, ProviderId, ProviderRegistry, ProviderSelection,
    SecretString, WorkflowProviderConfiguration, CLAUDE_PROVIDER_ID, CODEX_PROVIDER_ID,
    OMP_PROVIDER_ID,
};
use sentinel_supervisor::{SentinelSupervisor, SupervisorPrograms};
use sentinel_validation::load_repository_profiles;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Request {
    Capabilities,
    ActiveTask,
    AttentionState,
    Status,
    Settings,
    TaskDetail {
        task_id: String,
    },
    Subscribe,
    DiscoverModels {
        request_id: String,
        provider_id: ProviderId,
    },
    SaveQuickPromptPreferences {
        request_id: String,
        preferences: QuickPromptPreferences,
    },
    StartTask {
        request_id: String,
        summary: String,
        prompt: String,
        implementer: ProviderSelection,
        reviewer: ProviderSelection,
        #[serde(default)]
        workflow_mode: WorkflowMode,
    },
    RetryInterruptedOmp {
        request_id: String,
        task_id: String,
        prompt: String,
    },
    RetryReview {
        request_id: String,
        task_id: String,
    },
    AttentionAction {
        request_id: String,
        action: String,
        task_id: String,
        approval_id: Option<String>,
    },
    SetRepository {
        request_id: String,
        path: String,
    },
    ProviderSettingsMutation {
        request_id: String,
        mutation: ProviderSettingsMutation,
    },
    RuntimeDiagnostics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProviderSettingsMutation {
    SetEnabled {
        provider_id: ProviderId,
        enabled: bool,
    },
    SetModel {
        provider_id: ProviderId,
        model_id: String,
    },
    SetCredential {
        provider_id: ProviderId,
        api_key: String,
    },
    RemoveCredential {
        provider_id: ProviderId,
    },
    SetWorkflow {
        workflow: WorkflowProviderConfiguration,
    },
    UpsertCustom {
        provider: CustomProviderSpec,
    },
    RemoveCustom {
        provider_id: ProviderId,
    },
}

enum BridgeInput {
    Request(Request),
    WorkflowChanged,
    ProviderStatusChanged,
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDto {
    id: String,
    label: String,
    available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuickPromptPreferences {
    implementer: ProviderSelection,
    reviewer: ProviderSelection,
    workflow_mode: WorkflowMode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuickPromptDefaultsDto {
    implementer_provider_id: String,
    reviewer_provider_id: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TaskDto {
    id: String,
    summary: String,
    lifecycle: String,
    recovery_required: bool,
    recovery_reason: Option<String>,
    version: u64,
    updated_at_ms: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct AttentionActionsDto {
    stop: bool,
    retry_review: bool,
    approve: bool,
    reject: bool,
    approval_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct AttentionDto {
    task: TaskDto,
    display_state: String,
    provider: Option<String>,
    activity: Option<String>,
    validation: Option<String>,
    review: Option<String>,
    actions: AttentionActionsDto,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProviderStatusDto {
    name: String,
    installation: String,
    runtime: String,
    usage: String,
    rate_limits: Option<Value>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StatusDto {
    version: u64,
    sentinel: String,
    active_task: Option<TaskDto>,
    recovery_required: bool,
    codex: ProviderStatusDto,
    claude: ProviderStatusDto,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SettingsProviderDto {
    name: String,
    installation: String,
    executable_override: Option<String>,
    supports_executable_override: bool,
    authentication: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SettingsProfileStepDto {
    name: String,
    kind: String,
    cwd: String,
    timeout_ms: u64,
    required: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SettingsProfileDto {
    id: String,
    steps: Vec<SettingsProfileStepDto>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SettingsDto {
    version: u64,
    global_shortcut: String,
    repository: Option<String>,
    default_provider: String,
    codex: SettingsProviderDto,
    claude: SettingsProviderDto,
    validation_profiles: Vec<SettingsProfileDto>,
    validation_error: Option<String>,
    provider_settings: ProviderSettingsSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeConfig {
    #[serde(default)]
    version: u64,
    repository_root: Option<PathBuf>,
    #[serde(default)]
    quick_prompt_preferences: Option<QuickPromptPreferences>,
}

fn config_path(database_path: &Path) -> PathBuf {
    database_path.with_file_name("native-config.json")
}

fn load_config(path: &Path) -> NativeConfig {
    std::fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() <= 16 * 1024)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_config(path: &Path, config: &NativeConfig) -> Result<(), ()> {
    let bytes = serde_json::to_vec(config).map_err(|_| ())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| ())?;
    }
    file.write_all(&bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|_| ())
}

fn valid_quick_prompt_preferences(preferences: &QuickPromptPreferences) -> bool {
    let supported_provider = |id: &ProviderId| {
        matches!(
            id.as_str(),
            CODEX_PROVIDER_ID | CLAUDE_PROVIDER_ID | OMP_PROVIDER_ID
        )
    };
    let valid_value = |value: &str| {
        !value.trim().is_empty()
            && value.len() <= 128
            && !value.contains('\0')
            && !value.chars().any(char::is_control)
    };
    supported_provider(&preferences.implementer.provider_id)
        && supported_provider(&preferences.reviewer.provider_id)
        && valid_value(&preferences.implementer.model_id)
        && valid_value(&preferences.reviewer.model_id)
        && preferences
            .implementer
            .reasoning_effort
            .as_deref()
            .is_none()
        && preferences
            .reviewer
            .reasoning_effort
            .as_deref()
            .is_none_or(valid_value)
}

fn discovery_error(error: &ProviderError) -> (&'static str, String) {
    match error {
        ProviderError::Credential(_) | ProviderError::Authentication => {
            ("authentication_required", "Authentication required".into())
        }
        ProviderError::UnsupportedCapability(_) => (
            "enumeration_unsupported",
            "Model enumeration is unsupported".into(),
        ),
        ProviderError::MalformedResponse => (
            "invalid_response",
            "Provider returned invalid model data".into(),
        ),
        ProviderError::Timeout => ("discovery_timed_out", "Model discovery timed out".into()),
        ProviderError::Unavailable(message) => ("discovery_command_failed", message.clone()),
        ProviderError::ProviderUnavailable(provider_id) => (
            "provider_unavailable",
            format!("Provider is unavailable: {provider_id}"),
        ),
        _ => ("provider_unavailable", "Provider unavailable".into()),
    }
}

fn discovery_response(
    request_id: String,
    provider_id: ProviderId,
    result: Result<sentinel_provider_api::ProviderModelCatalog, ProviderError>,
) -> Value {
    match result {
        Ok(catalog) => json!({
            "kind":"discover_models_result",
            "requestId":request_id,
            "providerId":provider_id,
            "catalog":catalog
        }),
        Err(error) => {
            let (code, message) = discovery_error(&error);
            json!({
                "kind":"discover_models_result",
                "requestId":request_id,
                "providerId":provider_id,
                "error":{"code":code,"message":message}
            })
        }
    }
}

fn runtime_diagnostics(
    programs: &SupervisorPrograms,
    provider_settings: &ProviderSettingsStore,
) -> Value {
    let executable = programs.omp.as_ref().map(|program| program.executable());
    let executable_metadata = executable.and_then(|path| std::fs::metadata(path).ok());
    let environment_value =
        |key: &str| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned());
    json!({
        "kind":"runtime_diagnostics",
        "providerId":"omp",
        "providerEnabled":provider_settings.snapshot().providers.iter()
            .find(|provider| provider.id.as_str() == OMP_PROVIDER_ID)
            .map(|provider| provider.enabled),
        "command":"omp models --json",
        "workingDirectory":std::env::current_dir().ok().map(|path| path.display().to_string()),
        "environment":{
            "PATH":environment_value("PATH"),
            "HOME":environment_value("HOME"),
            "USER":environment_value("USER"),
            "SHELL":environment_value("SHELL"),
            "XDG_CONFIG_HOME":environment_value("XDG_CONFIG_HOME"),
            "XDG_DATA_HOME":environment_value("XDG_DATA_HOME"),
            "OMP_CONFIG_DIR":environment_value("OMP_CONFIG_DIR"),
        },
        "executable":{
            "resolvedPath":executable.map(|path| path.display().to_string()),
            "exists":executable_metadata.is_some(),
            "isFile":executable_metadata.as_ref().is_some_and(|metadata| metadata.is_file()),
            "isExecutable":executable.map(is_executable_file).unwrap_or(false),
        }
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// A native-shell-only account usage reader. Its provider protocol events live
/// in a separate database so usage polling can never become an active V3 task
/// or otherwise affect the workflow repository presented to Swift.
struct CodexUsageCollector {
    program: Option<CodexProgram>,
    repository: RunRepository,
    cwd: Option<PathBuf>,
    server: Option<CodexAppServer>,
    changes: mpsc::Sender<BridgeInput>,
}

impl CodexUsageCollector {
    fn new(
        program: Option<CodexProgram>,
        repository: RunRepository,
        cwd: Option<PathBuf>,
        changes: mpsc::Sender<BridgeInput>,
    ) -> Self {
        Self {
            program,
            repository,
            cwd,
            server: None,
            changes,
        }
    }

    async fn rate_limits(&mut self, refresh: bool) -> Option<Value> {
        if self.server.as_ref().is_some_and(CodexAppServer::is_alive) {
            let server = self.server.as_ref()?;
            return if refresh {
                server
                    .read_rate_limits()
                    .await
                    .ok()
                    .flatten()
                    .or_else(|| server.latest_rate_limits())
            } else {
                server.latest_rate_limits()
            }
            .map(|limits| limits.0);
        }
        self.server = None;
        let (Some(program), Some(cwd)) = (self.program.clone(), self.cwd.as_deref()) else {
            return None;
        };
        let task = self
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "native-provider-usage".into(),
                    summary: "Native Codex account usage".into(),
                },
                now_ms(),
            )
            .await
            .ok()?;
        let server = CodexAppServer::start(program, self.repository.clone(), task.id, cwd)
            .await
            .ok()?;
        let mut updates = server.subscribe_rate_limits();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            while updates.changed().await.is_ok() {
                if changes.send(BridgeInput::ProviderStatusChanged).is_err() {
                    break;
                }
            }
        });
        let latest = server.latest_rate_limits().map(|limits| limits.0);
        self.server = Some(server);
        latest
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn task_dto(task: sentinel_core::v3::Task) -> TaskDto {
    let recovery_required = task.recovery_condition != sentinel_core::v3::RecoveryCondition::None;
    TaskDto {
        id: task.id.to_string(),
        summary: task.summary,
        lifecycle: format!("{:?}", task.lifecycle).to_lowercase(),
        recovery_reason: recovery_required
            .then(|| format!("{:?}", task.recovery_condition).to_lowercase()),
        recovery_required,
        version: task.version,
        updated_at_ms: task.updated_at_ms,
    }
}

async fn active_task(
    supervisor: Option<&SentinelSupervisor>,
) -> Result<Option<TaskDto>, CoreError> {
    match supervisor {
        Some(supervisor) => supervisor
            .active_task()
            .await
            .map(|task| task.map(task_dto))
            .map_err(|_| CoreError::Storage),
        None => Ok(None),
    }
}

fn attention_display_state(task: &sentinel_core::v3::Task) -> &'static str {
    use sentinel_core::v3::TaskLifecycle;
    if task.recovery_condition != sentinel_core::v3::RecoveryCondition::None {
        return "recovery_required";
    }
    match task.lifecycle {
        TaskLifecycle::AwaitingApproval => "waiting_for_approval",
        TaskLifecycle::Implementing => "implementing",
        TaskLifecycle::Validating => "validating",
        TaskLifecycle::Reviewing => "reviewing",
        TaskLifecycle::Integrating => "integrating",
        TaskLifecycle::Integrated => "integrated",
        TaskLifecycle::ReadyForHuman => "ready_for_review",
        TaskLifecycle::Blocked => "blocked",
        TaskLifecycle::Failed => "failed",
        TaskLifecycle::Cancelled => "cancelled",
        _ => "working",
    }
}

fn provider_identity(provider: String) -> String {
    if provider.contains("claude") {
        "Claude Code".into()
    } else if provider.contains("codex") || provider.contains("openai") {
        "Codex".into()
    } else {
        provider
    }
}

async fn attention_state(
    supervisor: Option<&SentinelSupervisor>,
) -> Result<Option<AttentionDto>, CoreError> {
    let Some(supervisor) = supervisor else {
        return Ok(None);
    };
    let Some(task) = supervisor
        .active_task()
        .await
        .map_err(|_| CoreError::Storage)?
    else {
        return Ok(None);
    };
    let repository = supervisor.repository();
    let id = task.id.clone();
    let events = repository.v3().list_events(&id).await?;
    let sessions = repository.v3().list_sessions_for_task(&id).await?;
    let validations = repository.v3().list_validation_results(&id).await?;
    let findings = repository.v3().list_review_findings(&id).await?;
    let latest_validation = validations
        .last()
        .map(|item| format!("{:?}", item.lifecycle).to_lowercase());
    let blockers = findings
        .iter()
        .filter(|finding| {
            matches!(finding.severity.as_str(), "blocker" | "high")
                && !matches!(
                    format!("{:?}", finding.disposition).to_lowercase().as_str(),
                    "resolved" | "dismissed" | "repaired"
                )
        })
        .count();
    let actions = supervisor
        .available_actions(&task)
        .await
        .map_err(|_| CoreError::Storage)?;
    Ok(Some(AttentionDto {
        display_state: attention_display_state(&task).into(),
        provider: sessions
            .last()
            .map(|session| provider_identity(session.provider.clone())),
        activity: events
            .last()
            .map(|event| format!("{:?}", event.kind).to_lowercase()),
        validation: latest_validation,
        review: (blockers > 0)
            .then(|| format!("{blockers} blocker{}", if blockers == 1 { "" } else { "s" })),
        actions: AttentionActionsDto {
            stop: actions.stop,
            retry_review: actions.retry_review,
            approve: actions.approve,
            reject: actions.reject,
            approval_id: actions.approval_id.map(|approval| approval.to_string()),
        },
        task: task_dto(task),
    }))
}

async fn active_task_record(
    supervisor: Option<&SentinelSupervisor>,
) -> Result<Option<sentinel_core::v3::Task>, CoreError> {
    match supervisor {
        Some(supervisor) => supervisor
            .active_task()
            .await
            .map_err(|_| CoreError::Storage),
        None => Ok(None),
    }
}

fn installation_status(status: InstallationStatus) -> String {
    match status {
        InstallationStatus::Available { version, .. } => format!("available · {version}"),
        InstallationStatus::NotInstalled => "not installed".into(),
        InstallationStatus::Unusable { .. } => "unavailable".into(),
    }
}

async fn status_snapshot(
    repository: &RunRepository,
    supervisor: Option<&SentinelSupervisor>,
    usage: &mut CodexUsageCollector,
    refresh_usage: bool,
) -> Result<StatusDto, CoreError> {
    let task = active_task_record(supervisor).await?;
    let (version, recovery_required, sentinel) = match task.as_ref() {
        Some(task) => (
            task.version,
            task.recovery_condition != sentinel_core::v3::RecoveryCondition::None,
            format!("{:?}", task.lifecycle).to_lowercase(),
        ),
        None => (0, false, "ready".into()),
    };
    let codex_sessions = match task.as_ref() {
        Some(task) => repository.v3().list_sessions_for_task(&task.id).await?,
        None => Vec::new(),
    };
    let codex_runtime = codex_sessions
        .iter()
        .rev()
        .find(|session| session.provider.contains("codex") || session.provider.contains("openai"))
        .map(|session| {
            format!(
                "session {}",
                format!("{:?}", session.lifecycle).to_lowercase()
            )
        })
        .unwrap_or_else(|| "no owned session".into());
    let claude_runtime = codex_sessions
        .iter()
        .rev()
        .find(|session| session.provider.contains("claude"))
        .map(|session| {
            format!(
                "session {}",
                format!("{:?}", session.lifecycle).to_lowercase()
            )
        })
        .unwrap_or_else(|| "no owned session".into());
    Ok(StatusDto {
        version,
        sentinel,
        active_task: task.map(task_dto),
        recovery_required,
        codex: ProviderStatusDto {
            name: "Codex".into(),
            installation: installation_status(detect_installation(AgentKind::Codex)),
            runtime: codex_runtime,
            usage: "Codex App Server account/rateLimits/read".into(),
            rate_limits: usage.rate_limits(refresh_usage).await,
        },
        claude: ProviderStatusDto {
            name: "Claude Code".into(),
            installation: installation_status(detect_installation(AgentKind::ClaudeCode)),
            runtime: claude_runtime,
            usage: "Authenticated usage is unavailable in the native bridge.".into(),
            rate_limits: None,
        },
    })
}

async fn settings_snapshot(
    supervisor: Option<&SentinelSupervisor>,
    root: Option<&Path>,
    config_version: u64,
    provider_store: &ProviderSettingsStore,
) -> Result<SettingsDto, CoreError> {
    let _ = active_task_record(supervisor).await?;
    let (validation_profiles, validation_error) = match root {
        Some(root) => match load_repository_profiles(root) {
            Ok(profiles) => (
                profiles
                    .profiles
                    .into_iter()
                    .map(|profile| SettingsProfileDto {
                        id: profile.id,
                        steps: profile
                            .steps
                            .into_iter()
                            .map(|step| SettingsProfileStepDto {
                                name: step.name,
                                kind: format!("{:?}", step.kind).to_lowercase(),
                                cwd: step.cwd,
                                timeout_ms: step.timeout_ms,
                                required: step.required,
                            })
                            .collect(),
                    })
                    .collect(),
                None,
            ),
            Err(_) => (
                Vec::new(),
                Some("Repository validation configuration is unavailable or invalid.".into()),
            ),
        },
        None => (
            Vec::new(),
            Some("No repository context is available.".into()),
        ),
    };
    let codex_override = std::env::var_os("AGENT_SENTINEL_CODEX_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string());
    let provider_settings = provider_store.snapshot();
    Ok(SettingsDto {
        version: config_version
            .saturating_mul(1_000_000)
            .saturating_add(provider_settings.version),
        global_shortcut: "Command+Shift+Space".into(),
        repository: root.map(|path| path.display().to_string()),
        default_provider: "codex".into(),
        codex: SettingsProviderDto {
            name: "Codex".into(),
            installation: installation_status(detect_installation(AgentKind::Codex)),
            executable_override: codex_override,
            supports_executable_override: false,
            authentication: "CLI-owned authentication; credentials are not exposed to Sentinel."
                .into(),
        },
        claude: SettingsProviderDto {
            name: "Claude Code".into(),
            installation: installation_status(detect_installation(AgentKind::ClaudeCode)),
            executable_override: None,
            supports_executable_override: false,
            authentication: "Authentication state is unavailable unless the provider proves it."
                .into(),
        },
        validation_profiles,
        validation_error,
        provider_settings,
    })
}

fn apply_provider_settings_mutation(
    store: &ProviderSettingsStore,
    mutation: ProviderSettingsMutation,
) -> Result<(), &'static str> {
    let result = match mutation {
        ProviderSettingsMutation::SetEnabled {
            provider_id,
            enabled,
        } => store.set_enabled(&provider_id, enabled),
        ProviderSettingsMutation::SetModel {
            provider_id,
            model_id,
        } => store.set_model(&provider_id, model_id),
        ProviderSettingsMutation::SetCredential {
            provider_id,
            api_key,
        } => {
            if api_key.trim() != api_key || api_key.len() < 8 || api_key.contains('\0') {
                return Err("the API key was rejected");
            }
            let secret =
                SecretString::new(api_key.into_bytes()).map_err(|_| "the API key was rejected")?;
            store.set_credential(&provider_id, secret)
        }
        ProviderSettingsMutation::RemoveCredential { provider_id } => {
            store.remove_credential(&provider_id)
        }
        ProviderSettingsMutation::SetWorkflow { workflow } => store.set_workflow(workflow),
        ProviderSettingsMutation::UpsertCustom { provider } => store.upsert_custom(provider),
        ProviderSettingsMutation::RemoveCustom { provider_id } => store.remove_custom(&provider_id),
    };
    result
        .map(|_| ())
        .map_err(|_| "provider settings were rejected by Rust")
}

async fn task_detail(
    repository: &RunRepository,
    supervisor: Option<&SentinelSupervisor>,
    task_id: String,
) -> Result<Value, CoreError> {
    let task_id = sentinel_core::v3::TaskId(task_id);
    let task = repository.v3().get_task(&task_id).await?;
    let events = repository.v3().list_events(&task_id).await?;
    let worktree = repository.v3().get_task_worktree(&task_id).await.ok();
    let merge = repository
        .v3()
        .get_task_worktree_merge_preparation(&task_id)
        .await
        .ok();
    let validations = repository.v3().list_validation_results(&task_id).await?;
    let findings = repository.v3().list_review_findings(&task_id).await?;
    let repair_rounds = repository.v3().list_repair_rounds(&task_id).await?;
    let sessions = repository.v3().list_sessions_for_task(&task_id).await?;
    let artifacts = repository.v3().list_artifacts(&task_id).await?;
    let detail_actions = attention_state(supervisor)
        .await?
        .filter(|state| state.task.id == task_id.0)
        .map(|state| state.actions)
        .unwrap_or(AttentionActionsDto {
            stop: false,
            retry_review: false,
            approve: false,
            reject: false,
            approval_id: None,
        });
    let validation_details = futures_join_validations(repository, validations).await?;
    let final_packet = artifacts
        .iter()
        .rev()
        .find(|artifact| artifact.kind == "final_approval_packet")
        .map(|artifact| serde_json::to_string(&artifact.metadata).unwrap_or_default());
    let pinned_configuration = artifacts
        .iter()
        .rev()
        .find(|artifact| artifact.kind == "provider_role_configuration")
        .and_then(|artifact| {
            serde_json::from_value::<WorkflowProviderConfiguration>(artifact.metadata.clone()).ok()
        })
        .map(|roles| {
            json!({
                "implementer":roles.implementer,
                "reviewer":roles.reviewer,
                "workflowMode":task.workflow_mode
            })
        });
    Ok(json!({
        "task": task_dto(task),
        "configuration": pinned_configuration,
        "sessions": sessions.into_iter().map(|session| json!({"provider":provider_identity(session.provider),"sessionRef":session.provider_session_ref,"state":format!("{:?}",session.lifecycle).to_lowercase(),"updatedAtMs":session.updated_at_ms})).collect::<Vec<_>>(),
        "activity": events.into_iter().map(|event| json!({"kind":format!("{:?}", event.kind).to_lowercase(),"provider":provider_identity(event.provider),"occurredAtMs":event.occurred_at_ms,"payload":serde_json::to_string(&event.payload).unwrap_or_default()})).collect::<Vec<_>>(),
        "worktree": worktree.map(|value| json!({"repositoryRoot":value.repository_root,"path":value.worktree_path,"branch":value.branch,"baseCommit":value.base_commit,"state":value.state})),
        "diff": merge.map(|value| json!({"targetBranch":value.target_branch,"targetAdvanced":value.target_advanced,"mergeReady":value.merge_ready,"summary":value.diff_json,"conflicts":value.conflicts_json})),
        "validations": validation_details,
        "findings": findings.iter().map(|value| json!({"id":value.id.to_string(),"repairRoundId":value.repair_round_id.as_ref().map(ToString::to_string),"severity":value.severity,"disposition":format!("{:?}",value.disposition).to_lowercase(),"summary":value.summary,"evidence":serde_json::to_string(&value.evidence).unwrap_or_default()})).collect::<Vec<_>>(),
        "repairRounds": repair_rounds.into_iter().map(|value| json!({"id":value.id.to_string(),"round":value.round_number,"state":format!("{:?}",value.lifecycle).to_lowercase(),"updatedAtMs":value.updated_at_ms})).collect::<Vec<_>>(),
        "finalApprovalPacket": final_packet,
        "actions": detail_actions
    }))
}

async fn futures_join_validations(
    repository: &RunRepository,
    validations: Vec<sentinel_core::v3::ValidationResult>,
) -> Result<Vec<Value>, CoreError> {
    let mut result = Vec::with_capacity(validations.len());
    for value in validations {
        let execution = repository
            .v3()
            .get_validation_execution(&value.id)
            .await
            .ok();
        result.push(json!({
            "id":value.id.to_string(),"profile":value.profile_id,"check":value.check_name,
            "required":value.required,"state":format!("{:?}",value.lifecycle).to_lowercase(),
            "summary":value.summary,"updatedAtMs":value.updated_at_ms,
            "command":execution.as_ref().map(|entry| &entry.command_json),
            "exitCode":execution.as_ref().and_then(|entry| entry.exit_code),
            "durationMs":execution.as_ref().map(|entry| entry.duration_ms),
            "stdout":execution.as_ref().map(|entry| &entry.stdout),
            "stderr":execution.as_ref().map(|entry| &entry.stderr),
            "outcome":execution.as_ref().map(|entry| &entry.outcome)
        }));
    }
    Ok(result)
}

fn codex_program() -> Option<CodexProgram> {
    executable_path("AGENT_SENTINEL_CODEX_EXECUTABLE", "codex")
        .and_then(|path| CodexProgram::from_executable(path).ok())
}

fn claude_program() -> Option<ClaudeProgram> {
    executable_path("AGENT_SENTINEL_CLAUDE_EXECUTABLE", "claude")
        .and_then(|path| ClaudeProgram::from_executable(path).ok())
}
fn omp_program() -> Option<OmpProgram> {
    executable_path("AGENT_SENTINEL_OMP_EXECUTABLE", "omp")
        .and_then(|path| OmpProgram::from_executable(path).ok())
}

fn executable_path(override_name: &str, executable_name: &str) -> Option<PathBuf> {
    std::env::var_os(override_name)
        .map(PathBuf::from)
        .filter(|candidate| candidate.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join(executable_name))
                    .find(|candidate| candidate.is_file())
            })
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/bin").join(executable_name))
                .filter(|candidate| candidate.is_file())
        })
}

fn repository_root(config: &NativeConfig) -> Option<PathBuf> {
    std::env::var_os("SENTINEL_REPOSITORY_ROOT")
        .map(PathBuf::from)
        .or_else(|| config.repository_root.clone())
        .or_else(|| std::env::current_dir().ok())
        .filter(|path| path.is_dir())
}

fn build_supervisor(
    runtime: &tokio::runtime::Runtime,
    repository: &RunRepository,
    query_repository: &RunRepository,
    root: &Path,
    worktree_root: &Path,
    programs: &SupervisorPrograms,
    provider_settings: &ProviderSettingsStore,
) -> Option<SentinelSupervisor> {
    let inspection = runtime.block_on(inspect_repository(root)).ok()?;
    if !inspection.is_primary || !matches!(inspection.state, RepositoryState::Valid) {
        return None;
    }
    let registry = provider_registry(provider_settings, programs, query_repository, root).ok()?;
    let workflow = provider_settings.snapshot().workflow;
    let mut supervisor = SentinelSupervisor::with_provider_runtime(
        repository.clone(),
        &inspection.primary_root,
        worktree_root,
        programs.clone(),
        Arc::new(registry),
        workflow,
    )
    .ok()?;
    runtime
        .block_on(supervisor.enter_recovery_after_restart())
        .ok()?;
    Some(supervisor)
}

fn provider_registry(
    settings: &ProviderSettingsStore,
    programs: &SupervisorPrograms,
    query_repository: &RunRepository,
    cwd: &Path,
) -> Result<ProviderRegistry, ProviderError> {
    let mut registry = settings
        .build_registry(
            programs.codex.is_some(),
            programs.claude.is_some(),
            programs.omp.is_some(),
        )
        .map_err(|_| ProviderError::InvalidConfiguration("provider settings are invalid".into()))?;
    if let Some(program) = programs.codex.clone() {
        registry.register_model_discovery(Arc::new(CodexModelDiscovery::new(
            program,
            query_repository.clone(),
            cwd,
        )))?;
    }
    if let Some(program) = programs.claude.clone() {
        registry.register_model_discovery(Arc::new(ClaudeModelDiscovery::new(program)))?;
    }
    if let Some(program) = programs.omp.clone() {
        registry.register_model_discovery(Arc::new(OmpModelDiscovery::new(program)))?;
    }
    Ok(registry)
}

fn quick_prompt_providers(
    settings: &ProviderSettingsStore,
    codex_available: bool,
    claude_available: bool,
    omp_available: bool,
) -> Vec<ProviderDto> {
    let snapshot = settings.snapshot();
    snapshot
        .providers
        .into_iter()
        .filter_map(|provider| {
            if !matches!(
                provider.id.as_str(),
                CODEX_PROVIDER_ID | CLAUDE_PROVIDER_ID | OMP_PROVIDER_ID
            ) || (!provider.capabilities.implementation
                && !provider.capabilities.read_only_review)
            {
                return None;
            }
            let transport_available = match provider.id.as_str() {
                CODEX_PROVIDER_ID => codex_available,
                CLAUDE_PROVIDER_ID => claude_available,
                OMP_PROVIDER_ID => omp_available,
                _ => false,
            };
            Some(ProviderDto {
                id: provider.id.to_string(),
                label: provider.display_name,
                available: provider.enabled && transport_available,
            })
        })
        .collect()
}

fn quick_prompt_defaults(settings: &ProviderSettingsStore) -> QuickPromptDefaultsDto {
    let workflow = settings.snapshot().workflow;
    QuickPromptDefaultsDto {
        implementer_provider_id: workflow.implementer.provider_id.to_string(),
        reviewer_provider_id: workflow.reviewer.provider_id.to_string(),
    }
}

async fn perform_attention_action(
    supervisor: &mut SentinelSupervisor,
    action: String,
    task_id: String,
    approval_id: Option<String>,
) -> Result<(), &'static str> {
    let state = attention_state(Some(supervisor))
        .await
        .map_err(|_| "could not read task state")?
        .ok_or("there is no active task")?;
    if state.task.id != task_id {
        return Err("task is no longer active");
    }
    match (action.as_str(), approval_id) {
        ("stop", None) if state.actions.stop => supervisor
            .stop(&TaskId(task_id))
            .await
            .map_err(|_| "stop was rejected by the Rust supervisor"),
        ("approve", Some(approval_id))
            if state.actions.approve
                && state.actions.approval_id.as_deref() == Some(&approval_id) =>
        {
            supervisor
                .decide_final_approval(&TaskId(task_id), &ApprovalId(approval_id), true)
                .await
                .map_err(|_| "approval was rejected by the Rust supervisor")
        }
        ("reject", Some(approval_id))
            if state.actions.reject
                && state.actions.approval_id.as_deref() == Some(&approval_id) =>
        {
            supervisor
                .decide_final_approval(&TaskId(task_id), &ApprovalId(approval_id), false)
                .await
                .map_err(|_| "rejection was rejected by the Rust supervisor")
        }
        _ => Err("action is not authorized for the current task"),
    }
}

fn response(value: Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{}", value);
    let _ = stdout.flush();
}

fn request_fingerprint(value: &Value) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(encoded))
}

fn reserve_mutation(
    runtime: &tokio::runtime::Runtime,
    repository: &RunRepository,
    request_id: &str,
    request_kind: &str,
    fingerprint: &str,
) -> Result<SupervisorRequestReservation, ()> {
    runtime
        .block_on(repository.v3().reserve_supervisor_request(
            request_id,
            request_kind,
            fingerprint,
            now_ms(),
        ))
        .map_err(|_| ())
}

fn complete_mutation(
    runtime: &tokio::runtime::Runtime,
    repository: &RunRepository,
    request_id: &str,
    payload: &Value,
) -> bool {
    serde_json::to_string(payload).is_ok_and(|encoded| {
        runtime
            .block_on(
                repository
                    .v3()
                    .complete_supervisor_request(request_id, &encoded, now_ms()),
            )
            .is_ok()
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let e2e_diagnostics = std::env::args().any(|argument| argument == "--e2e-diagnostics");
    let database_path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("database path argument is required")?,
    );
    let database_url = format!("sqlite://{}", database_path.display());
    let runtime = tokio::runtime::Runtime::new()?;
    let repository = runtime.block_on(RunRepository::open(&database_url))?;
    let native_config_path = config_path(&database_path);
    let provider_settings_path = database_path.with_file_name("api-providers.json");
    let provider_settings = ProviderSettingsStore::load(
        &provider_settings_path,
        Arc::new(MacOsKeychainCredentialStore),
    )?;
    let mut config = load_config(&native_config_path);
    let mut root = repository_root(&config);
    let program = codex_program();
    let claude = claude_program();
    let omp = omp_program();
    let programs = SupervisorPrograms {
        codex: program.clone(),
        claude: claude.clone(),
        omp: omp.clone(),
    };
    let worktree_root = database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("task-worktrees");
    let _usage_directory = tempfile::Builder::new()
        .prefix("agent-sentinel-provider-query-")
        .tempdir()?;
    let usage_database_url = format!(
        "sqlite://{}",
        _usage_directory.path().join("queries.sqlite3").display()
    );
    let usage_repository = runtime.block_on(RunRepository::open(&usage_database_url))?;
    let mut supervisor = root.as_deref().and_then(|root| {
        build_supervisor(
            &runtime,
            &repository,
            &usage_repository,
            root,
            &worktree_root,
            &programs,
            &provider_settings,
        )
    });
    root = supervisor
        .as_ref()
        .map(|supervisor| supervisor.primary_root().to_owned());
    let (sender, receiver) = mpsc::channel();
    let mut usage = CodexUsageCollector::new(
        program.clone(),
        usage_repository.clone(),
        root.clone()
            .or_else(|| database_path.parent().map(Path::to_path_buf)),
        sender.clone(),
    );
    let request_sender = sender.clone();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(request) = serde_json::from_str::<Request>(&line) {
                let _ = request_sender.send(BridgeInput::Request(request));
            } else {
                response(json!({"kind":"error","message":"invalid native bridge request"}));
            }
        }
        let _ = request_sender.send(BridgeInput::Shutdown);
    });
    let mut workflow_changes = repository.subscribe_v3_changes();
    let workflow_sender = sender.clone();
    runtime.spawn(async move {
        loop {
            match workflow_changes.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if workflow_sender.send(BridgeInput::WorkflowChanged).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut subscribed = false;
    let mut last_snapshot = None;
    let mut last_status = None;
    loop {
        match receiver.recv() {
            Ok(BridgeInput::Request(Request::RuntimeDiagnostics)) if e2e_diagnostics => {
                let diagnostics = runtime_diagnostics(&programs, &provider_settings);
                let path = database_path.with_file_name("e2e-runtime-diagnostics.json");
                let _ = std::fs::write(
                    &path,
                    serde_json::to_vec_pretty(&diagnostics).unwrap_or_default(),
                );
                response(diagnostics);
            }
            Ok(BridgeInput::Request(Request::RuntimeDiagnostics)) => {
                response(json!({"kind":"error","message":"runtime diagnostics are disabled"}));
            }
            Ok(BridgeInput::Request(Request::Capabilities)) => response(json!({
                "kind":"capabilities",
                "repository":root.as_ref().map(|root| root.display().to_string()),
                "providers":quick_prompt_providers(&provider_settings, program.is_some(), claude.is_some(), omp.is_some()),
                "quickPromptPreferences":config.quick_prompt_preferences,
                "quickPromptDefaults":quick_prompt_defaults(&provider_settings)
            })),
            Ok(BridgeInput::Request(Request::DiscoverModels {
                request_id,
                provider_id,
            })) => {
                let cwd = root
                    .as_deref()
                    .or_else(|| database_path.parent())
                    .unwrap_or_else(|| Path::new("."));
                let result =
                    provider_registry(&provider_settings, &programs, &usage_repository, cwd)
                        .and_then(|registry| {
                            runtime.block_on(registry.discover_models(&provider_id))
                        });
                response(discovery_response(request_id, provider_id, result));
            }
            Ok(BridgeInput::Request(Request::SaveQuickPromptPreferences {
                request_id,
                preferences,
            })) => {
                let fingerprint =
                    request_fingerprint(&serde_json::to_value(&preferences).unwrap_or(Value::Null));
                let payload = match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "save_quick_prompt_preferences",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| {
                            json!({"kind":"error","message":"stored preference response is invalid"})
                        }));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) => {
                        json!({"kind":"quick_prompt_preferences_result","requestId":request_id,"accepted":false,"message":"preference update is already pending"})
                    }
                    Ok(SupervisorRequestReservation::New) => {
                        if valid_quick_prompt_preferences(&preferences) {
                            let mut candidate = config.clone();
                            candidate.version = candidate.version.saturating_add(1);
                            candidate.quick_prompt_preferences = Some(preferences);
                            if save_config(&native_config_path, &candidate).is_ok() {
                                config = candidate;
                                json!({"kind":"quick_prompt_preferences_result","requestId":request_id,"accepted":true})
                            } else {
                                json!({"kind":"quick_prompt_preferences_result","requestId":request_id,"accepted":false,"message":"preferences could not be persisted"})
                            }
                        } else {
                            json!({"kind":"quick_prompt_preferences_result","requestId":request_id,"accepted":false,"message":"preferences are invalid"})
                        }
                    }
                    Err(()) => {
                        json!({"kind":"quick_prompt_preferences_result","requestId":request_id,"accepted":false,"message":"preference request conflicts with durable intent"})
                    }
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"preference response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::Request(Request::ActiveTask)) => match runtime
                .block_on(active_task(supervisor.as_ref()))
            {
                Ok(task) => response(json!({"kind":"active_task","task":task})),
                Err(_) => response(json!({"kind":"error","message":"could not read active task"})),
            },
            Ok(BridgeInput::Request(Request::AttentionState)) => {
                match runtime.block_on(attention_state(supervisor.as_ref())) {
                    Ok(state) => response(json!({"kind":"attention_state","attention":state})),
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not read attention state"}))
                    }
                }
            }
            Ok(BridgeInput::Request(Request::Status)) => match runtime.block_on(status_snapshot(
                &repository,
                supervisor.as_ref(),
                &mut usage,
                true,
            )) {
                Ok(status) => response(json!({"kind":"status","status":status})),
                Err(_) => {
                    response(json!({"kind":"error","message":"could not read provider status"}))
                }
            },
            Ok(BridgeInput::Request(Request::Settings)) => {
                match runtime.block_on(settings_snapshot(
                    supervisor.as_ref(),
                    root.as_deref(),
                    config.version,
                    &provider_settings,
                )) {
                    Ok(settings) => response(json!({"kind":"settings","settings":settings})),
                    Err(_) => response(json!({"kind":"error","message":"could not read settings"})),
                }
            }
            Ok(BridgeInput::Request(Request::TaskDetail { task_id })) => {
                match runtime.block_on(task_detail(&repository, supervisor.as_ref(), task_id)) {
                    Ok(detail) => response(json!({"kind":"task_detail","detail":detail})),
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not read task detail"}))
                    }
                }
            }
            Ok(BridgeInput::Request(Request::Subscribe)) => {
                subscribed = true;
                last_snapshot = None;
                last_status = None;
                // A provider may have persisted progress before this client
                // attaches. Reconcile that durable evidence now rather than
                // waiting for an unrelated future repository notification.
                if let Some(supervisor) = supervisor.as_mut() {
                    let _ = runtime.block_on(supervisor.reconcile_progress());
                }
                response(json!({"kind":"subscribed"}));
            }
            Ok(BridgeInput::Request(Request::StartTask {
                request_id,
                summary,
                prompt,
                implementer,
                reviewer,
                workflow_mode,
            })) => {
                let configuration = WorkflowProviderConfiguration {
                    implementer: implementer.clone(),
                    reviewer: reviewer.clone(),
                    repair: None,
                };
                let fingerprint = request_fingerprint(
                    &json!({"implementer":implementer,"reviewer":reviewer,"summary":summary,"prompt":prompt,"workflow_mode":workflow_mode}),
                );
                match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "start_task",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(
                            |_| json!({"kind":"error","message":"stored task response is invalid"}),
                        ));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) => {
                        response(
                            json!({"kind":"task_start_result","requestId":request_id,"accepted":false,"message":"the prior task request has an unproven outcome; durable state must be reconciled"}),
                        );
                        continue;
                    }
                    Ok(SupervisorRequestReservation::New) => {}
                    Err(()) => {
                        response(
                            json!({"kind":"task_start_result","requestId":request_id,"accepted":false,"message":"the request ID conflicts with an existing durable intent"}),
                        );
                        continue;
                    }
                }
                let result = supervisor
                    .as_mut()
                    .ok_or("repository is unavailable or invalid")
                    .and_then(|supervisor| {
                        runtime
                            .block_on(supervisor.start_task_with_configuration(
                                configuration,
                                summary,
                                prompt,
                                workflow_mode,
                            ))
                            .map(task_dto)
                            .map_err(|error| match error {
                                sentinel_supervisor::SupervisorError::RecoveryRequired => {
                                    "the active task requires recovery"
                                }
                                sentinel_supervisor::SupervisorError::Ownership => {
                                    "repository or worktree ownership could not be proven"
                                }
                                sentinel_supervisor::SupervisorError::ProviderUnavailable => {
                                    "selected provider is unavailable"
                                }
                                _ => "task submission was rejected by the Rust supervisor",
                            })
                    });
                let payload = match result {
                    Ok(task) => {
                        let preferences = QuickPromptPreferences {
                            implementer,
                            reviewer,
                            workflow_mode,
                        };
                        let mut candidate = config.clone();
                        candidate.version = candidate.version.saturating_add(1);
                        candidate.quick_prompt_preferences = Some(preferences);
                        if save_config(&native_config_path, &candidate).is_ok() {
                            config = candidate;
                        }
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":true,"task":task})
                    }
                    Err(message) => {
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":false,"message":message})
                    }
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"task response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::Request(Request::RetryInterruptedOmp {
                request_id,
                task_id,
                prompt,
            })) => {
                let fingerprint = request_fingerprint(&json!({"task_id":task_id,"prompt":prompt}));
                let payload = match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "retry_interrupted_omp",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| json!({"kind":"error","message":"stored retry response is invalid"})));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) | Err(()) => {
                        json!({"kind":"retry_omp_result","requestId":request_id,"accepted":false,"message":"retry request was rejected"})
                    }
                    Ok(SupervisorRequestReservation::New) => match supervisor.as_mut() {
                        Some(supervisor) => match runtime.block_on(
                            supervisor.retry_interrupted_omp(&TaskId(task_id.clone()), prompt),
                        ) {
                            Ok(()) => match runtime
                                .block_on(supervisor.repository().v3().get_task(&TaskId(task_id)))
                            {
                                Ok(task) => {
                                    json!({"kind":"retry_omp_result","requestId":request_id,"accepted":true,"task":task_dto(task)})
                                }
                                Err(_) => {
                                    json!({"kind":"retry_omp_result","requestId":request_id,"accepted":false,"message":"refreshed task state unavailable"})
                                }
                            },
                            Err(_) => {
                                json!({"kind":"retry_omp_result","requestId":request_id,"accepted":false,"message":"OMP retry was rejected"})
                            }
                        },
                        None => {
                            json!({"kind":"retry_omp_result","requestId":request_id,"accepted":false,"message":"repository is unavailable or invalid"})
                        }
                    },
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"retry response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::Request(Request::RetryReview {
                request_id,
                task_id,
            })) => {
                let fingerprint = request_fingerprint(&json!({"task_id": task_id}));
                let payload = match reserve_mutation(&runtime, &repository, &request_id, "retry_review", &fingerprint) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| json!({"kind":"error","message":"stored review retry response is invalid"})));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) | Err(()) => json!({"kind":"retry_review_result","requestId":request_id,"accepted":false,"message":"review retry is already pending or conflicts"}),
                    Ok(SupervisorRequestReservation::New) => match supervisor.as_mut() {
                        Some(supervisor) => match runtime.block_on(supervisor.retry_review(&TaskId(task_id.clone()))) {
                            Ok(()) => runtime.block_on(supervisor.repository().v3().get_task(&TaskId(task_id))).map(|task| json!({"kind":"retry_review_result","requestId":request_id,"accepted":true,"task":task_dto(task)})).unwrap_or_else(|_| json!({"kind":"retry_review_result","requestId":request_id,"accepted":false,"message":"refreshed task state unavailable"})),
                            Err(_) => json!({"kind":"retry_review_result","requestId":request_id,"accepted":false,"message":"review retry was rejected; revalidation may be required"}),
                        },
                        None => json!({"kind":"retry_review_result","requestId":request_id,"accepted":false,"message":"repository is unavailable or invalid"}),
                    },
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"review retry response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::Request(Request::AttentionAction {
                request_id,
                action,
                task_id,
                approval_id,
            })) => {
                let fingerprint = request_fingerprint(
                    &json!({"action":action,"task_id":task_id,"approval_id":approval_id}),
                );
                match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "attention_action",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| {
                            json!({"kind":"error","message":"stored action response is invalid"})
                        }));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) => {
                        response(
                            json!({"kind":"attention_action_result","requestId":request_id,"accepted":false,"message":"the prior action has an unproven outcome and will not be replayed"}),
                        );
                        continue;
                    }
                    Ok(SupervisorRequestReservation::New) => {}
                    Err(()) => {
                        response(
                            json!({"kind":"attention_action_result","requestId":request_id,"accepted":false,"message":"the request ID conflicts with an existing durable intent"}),
                        );
                        continue;
                    }
                }
                let result = match supervisor.as_mut() {
                    Some(supervisor) => runtime.block_on(perform_attention_action(
                        supervisor,
                        action,
                        task_id,
                        approval_id,
                    )),
                    None => Err("repository is unavailable or invalid"),
                };
                let payload = match result {
                    Ok(()) => {
                        json!({"kind":"attention_action_result","requestId":request_id,"accepted":true})
                    }
                    Err(message) => {
                        json!({"kind":"attention_action_result","requestId":request_id,"accepted":false,"message":message})
                    }
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"action response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::Request(Request::SetRepository { request_id, path })) => {
                let fingerprint = request_fingerprint(&json!({"path":path}));
                match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "set_repository",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| {
                            json!({"kind":"error","message":"stored settings response is invalid"})
                        }));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) => {
                        response(
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"the prior settings request has an unproven outcome and will not be replayed"}),
                        );
                        continue;
                    }
                    Ok(SupervisorRequestReservation::New) => {}
                    Err(()) => {
                        response(
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"the request ID conflicts with an existing durable intent"}),
                        );
                        continue;
                    }
                }
                let requested = PathBuf::from(&path);
                let active_blocks_change = supervisor.as_ref().is_some_and(|current| {
                    runtime
                        .block_on(current.active_task())
                        .ok()
                        .flatten()
                        .is_some_and(|task| !task.lifecycle.terminal())
                });
                let next = if path.is_empty() || path.len() > 4096 || path.contains('\0') {
                    Err("repository path is invalid")
                } else if active_blocks_change
                    && root.as_deref() != requested.canonicalize().ok().as_deref()
                {
                    Err("an active task prevents changing repositories")
                } else {
                    build_supervisor(
                        &runtime,
                        &repository,
                        &usage_repository,
                        &requested,
                        &worktree_root,
                        &programs,
                        &provider_settings,
                    )
                    .ok_or("repository is unavailable or not a primary checkout")
                };
                match next {
                    Ok(next) => {
                        let confirmed = next.primary_root().to_owned();
                        config.repository_root = Some(confirmed.clone());
                        config.version = config.version.saturating_add(1);
                        if save_config(&native_config_path, &config).is_err() {
                            let payload = json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"repository setting could not be persisted"});
                            if complete_mutation(&runtime, &repository, &request_id, &payload) {
                                response(payload);
                            } else {
                                response(
                                    json!({"kind":"error","message":"settings response could not be durably confirmed"}),
                                );
                            }
                            continue;
                        }
                        root = Some(confirmed);
                        supervisor = Some(next);
                        let payload = match runtime.block_on(settings_snapshot(
                            supervisor.as_ref(),
                            root.as_deref(),
                            config.version,
                            &provider_settings,
                        )) {
                            Ok(settings) => {
                                json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":true,"settings":settings})
                            }
                            Err(_) => {
                                json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"confirmed settings could not be loaded"})
                            }
                        };
                        if complete_mutation(&runtime, &repository, &request_id, &payload) {
                            response(payload);
                        } else {
                            response(
                                json!({"kind":"error","message":"settings response could not be durably confirmed"}),
                            );
                        }
                    }
                    Err(message) => {
                        let payload = json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":message});
                        if complete_mutation(&runtime, &repository, &request_id, &payload) {
                            response(payload);
                        } else {
                            response(
                                json!({"kind":"error","message":"settings response could not be durably confirmed"}),
                            );
                        }
                    }
                }
            }
            Ok(BridgeInput::Request(Request::ProviderSettingsMutation {
                request_id,
                mutation,
            })) => {
                let fingerprint =
                    request_fingerprint(&serde_json::to_value(&mutation).unwrap_or(Value::Null));
                match reserve_mutation(
                    &runtime,
                    &repository,
                    &request_id,
                    "provider_settings_mutation",
                    &fingerprint,
                ) {
                    Ok(SupervisorRequestReservation::Completed(encoded)) => {
                        response(serde_json::from_str(&encoded).unwrap_or_else(|_| {
                            json!({"kind":"error","message":"stored provider settings response is invalid"})
                        }));
                        continue;
                    }
                    Ok(SupervisorRequestReservation::Pending) => {
                        response(
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"the prior provider settings request has an unproven outcome and will not be replayed"}),
                        );
                        continue;
                    }
                    Ok(SupervisorRequestReservation::New) => {}
                    Err(()) => {
                        response(
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"the request ID conflicts with an existing durable intent"}),
                        );
                        continue;
                    }
                }
                let payload = match apply_provider_settings_mutation(&provider_settings, mutation) {
                    Ok(()) => {
                        let runtime_update = supervisor
                            .as_mut()
                            .map(|supervisor| {
                                provider_registry(
                                    &provider_settings,
                                    &programs,
                                    &usage_repository,
                                    supervisor.primary_root(),
                                )
                                .map_err(|_| "provider registry could not be rebuilt")
                                .and_then(|registry| {
                                    supervisor
                                        .update_provider_runtime(
                                            Arc::new(registry),
                                            provider_settings.snapshot().workflow,
                                        )
                                        .map_err(|_| "provider routing could not be applied")
                                })
                            })
                            .transpose();
                        match runtime_update {
                            Err(message) => {
                                json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":message})
                            }
                            Ok(_) => match runtime.block_on(settings_snapshot(
                                supervisor.as_ref(),
                                root.as_deref(),
                                config.version,
                                &provider_settings,
                            )) {
                                Ok(settings) => {
                                    json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":true,"settings":settings})
                                }
                                Err(_) => {
                                    json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"confirmed provider settings could not be loaded"})
                                }
                            },
                        }
                    }
                    Err(message) => {
                        json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":message})
                    }
                };
                if complete_mutation(&runtime, &repository, &request_id, &payload) {
                    response(payload);
                } else {
                    response(
                        json!({"kind":"error","message":"provider settings response could not be durably confirmed"}),
                    );
                }
            }
            Ok(BridgeInput::WorkflowChanged) if subscribed => {
                if let Some(supervisor) = supervisor.as_mut() {
                    let _ = runtime.block_on(supervisor.reconcile_progress());
                }
                match runtime.block_on(attention_state(supervisor.as_ref())) {
                    Ok(attention) => {
                        let snapshot = serde_json::to_string(&attention).unwrap_or_default();
                        if last_snapshot.as_ref() != Some(&snapshot) {
                            last_snapshot = Some(snapshot);
                            response(json!({"kind":"attention_update","attention":attention}));
                        }
                        if let Ok(status) = runtime.block_on(status_snapshot(
                            &repository,
                            supervisor.as_ref(),
                            &mut usage,
                            false,
                        )) {
                            let snapshot = serde_json::to_string(&status).unwrap_or_default();
                            if last_status.as_ref() != Some(&snapshot) {
                                last_status = Some(snapshot);
                                response(json!({"kind":"status_update","status":status}));
                            }
                        }
                    }
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not refresh task state"}))
                    }
                }
            }
            Ok(BridgeInput::ProviderStatusChanged) if subscribed => {
                if let Ok(status) = runtime.block_on(status_snapshot(
                    &repository,
                    supervisor.as_ref(),
                    &mut usage,
                    false,
                )) {
                    let snapshot = serde_json::to_string(&status).unwrap_or_default();
                    if last_status.as_ref() != Some(&snapshot) {
                        last_status = Some(snapshot);
                        response(json!({"kind":"status_update","status":status}));
                    }
                }
            }
            Ok(BridgeInput::WorkflowChanged | BridgeInput::ProviderStatusChanged) => {}
            Ok(BridgeInput::Shutdown) => {
                // EOF on the client stream is not provider completion. Persist
                // recovery before dropping the supervisor (which closes OMP's
                // stdin and makes RPC mode exit cleanly).
                if let Some(supervisor) = supervisor.as_mut() {
                    let _ = runtime.block_on(supervisor.prepare_for_host_shutdown());
                }
                break;
            }
            Err(_) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{RecoveryCondition, Task, TaskId, TaskLifecycle};
    use sentinel_provider_api::{
        CredentialStore, MemoryCredentialStore, ModelDiscoveryKind, ProviderModel,
        ProviderModelAvailability, ProviderModelCatalog,
    };
    use std::{fs, sync::Arc};

    fn display_task(lifecycle: TaskLifecycle) -> Task {
        Task {
            id: TaskId("display-task".into()),
            project_id: None,
            workflow_id: "display-workflow".into(),
            workflow_mode: WorkflowMode::Manual,
            summary: "display state".into(),
            lifecycle,
            recovery_condition: RecoveryCondition::None,
            recovery_previous_lifecycle: None,
            version: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            terminal_at_ms: None,
        }
    }

    #[test]
    fn workflow_attention_states_render_without_inventing_approval() {
        assert_eq!(
            attention_display_state(&display_task(TaskLifecycle::ReadyForHuman)),
            "ready_for_review"
        );
        assert_eq!(
            attention_display_state(&display_task(TaskLifecycle::Integrating)),
            "integrating"
        );
        assert_eq!(
            attention_display_state(&display_task(TaskLifecycle::Integrated)),
            "integrated"
        );
        assert_eq!(
            attention_display_state(&display_task(TaskLifecycle::Blocked)),
            "blocked"
        );
    }

    #[test]
    fn bridge_requests_are_narrow_and_typed() {
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"kind":"active_task"}"#),
            Ok(Request::ActiveTask)
        ));
        assert!(serde_json::from_str::<Request>(r#"{"kind":"direct_sql"}"#).is_err());
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"retry_review","request_id":"request-1","task_id":"task-1"}"#
            ),
            Ok(Request::RetryReview { .. })
        ));
    }

    #[test]
    fn task_start_request_requires_pinned_selections_and_intent_fields() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"start_task","request_id":"request-1","summary":"Task","prompt":"Do it","implementer":{"providerId":"omp","modelId":"provider/model"},"reviewer":{"providerId":"codex","modelId":"review-model","reasoningEffort":"medium"}}"#
            ),
            Ok(Request::StartTask { .. })
        ));
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"start_task","summary":"Task"}"#).is_err()
        );
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"start_task","request_id":"request-2","summary":"Task","prompt":"Do it","implementer":{"providerId":"omp","modelId":"provider/model"},"reviewer":{"providerId":"codex","modelId":"review-model","reasoningEffort":"medium"},"workflow_mode":"auto_integrate"}"#
            ),
            Ok(Request::StartTask {
                workflow_mode: WorkflowMode::AutoIntegrate,
                ..
            })
        ));
    }

    #[test]
    fn discovery_and_preferences_requests_are_typed_queries_and_mutations() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"discover_models","request_id":"models-1","provider_id":"omp"}"#
            ),
            Ok(Request::DiscoverModels { .. })
        ));
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"save_quick_prompt_preferences","request_id":"prefs-1","preferences":{"implementer":{"providerId":"omp","modelId":"provider/model"},"reviewer":{"providerId":"codex","modelId":"review-model","reasoningEffort":"medium"},"workflowMode":"manual"}}"#
            ),
            Ok(Request::SaveQuickPromptPreferences { .. })
        ));
    }

    #[test]
    fn discovery_response_preserves_mocked_omp_data_empty_and_error_states() {
        let provider_id = ProviderId::new(OMP_PROVIDER_ID).unwrap();
        let model = ProviderModel {
            provider_id: provider_id.clone(),
            model_id: "fixture/runtime-model".into(),
            display_name: Some("Runtime Model".into()),
            supported_reasoning_efforts: vec!["low".into(), "future-effort".into()],
            default_reasoning_effort: Some("future-effort".into()),
            is_default: false,
            availability: ProviderModelAvailability::Available,
        };
        let catalog = ProviderModelCatalog {
            provider_id: provider_id.clone(),
            discovery_kind: ModelDiscoveryKind::Enumerated,
            models: vec![model],
        };
        let expected_catalog = serde_json::to_value(&catalog).unwrap();
        let payload = discovery_response("models-omp".into(), provider_id.clone(), Ok(catalog));
        assert_eq!(payload["catalog"], expected_catalog);

        let empty = discovery_response(
            "models-empty".into(),
            provider_id.clone(),
            Ok(ProviderModelCatalog {
                provider_id: provider_id.clone(),
                discovery_kind: ModelDiscoveryKind::Enumerated,
                models: Vec::new(),
            }),
        );
        assert_eq!(empty["catalog"]["models"], json!([]));

        let failed = discovery_response(
            "models-failed".into(),
            provider_id,
            Err(ProviderError::Unavailable("fixture".into())),
        );
        assert_eq!(failed["error"]["code"], "discovery_command_failed");
        assert_eq!(failed["error"]["message"], "fixture");
        assert!(failed.get("catalog").is_none());
    }

    #[test]
    fn attention_actions_are_typed_and_do_not_accept_arbitrary_commands() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"attention_action","request_id":"action-1","action":"stop","task_id":"task-1"}"#
            ),
            Ok(Request::AttentionAction { .. })
        ));
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"attention_action","action":"merge"}"#)
                .is_err()
        );
    }

    #[test]
    fn repository_setting_is_typed_and_persisted_without_secret_fields() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"set_repository","request_id":"setting-1","path":"/repo"}"#
            ),
            Ok(Request::SetRepository { .. })
        ));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native-config.json");
        let config = NativeConfig {
            version: 4,
            repository_root: Some(PathBuf::from("/repo")),
            quick_prompt_preferences: Some(QuickPromptPreferences {
                implementer: ProviderSelection {
                    provider_id: ProviderId::new(OMP_PROVIDER_ID).unwrap(),
                    model_id: "fixture/runtime".into(),
                    reasoning_effort: None,
                },
                reviewer: ProviderSelection {
                    provider_id: ProviderId::new(CODEX_PROVIDER_ID).unwrap(),
                    model_id: "fixture-review".into(),
                    reasoning_effort: Some("medium".into()),
                },
                workflow_mode: WorkflowMode::AutoIntegrate,
            }),
        };
        save_config(&path, &config).unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("/repo"));
        assert!(!persisted.to_ascii_lowercase().contains("token"));
        assert_eq!(load_config(&path).version, 4);
        assert_eq!(load_config(&path).repository_root, config.repository_root);
        assert_eq!(
            load_config(&path).quick_prompt_preferences,
            config.quick_prompt_preferences
        );
    }

    #[test]
    fn provider_settings_requests_are_typed_and_never_return_plaintext_keys() {
        let request = serde_json::from_str::<Request>(
            r#"{"kind":"provider_settings_mutation","request_id":"provider-1","mutation":{"action":"set_credential","provider_id":"gemini","api_key":"super-secret-api-key"}}"#,
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentialStore::default());
        let store = ProviderSettingsStore::load(
            directory.path().join("api-providers.json"),
            credentials.clone(),
        )
        .unwrap();
        let Request::ProviderSettingsMutation { mutation, .. } = request else {
            panic!("expected provider settings mutation");
        };

        apply_provider_settings_mutation(&store, mutation).unwrap();
        let snapshot = serde_json::to_string(&store.snapshot()).unwrap();
        assert!(snapshot.contains("configured"));
        assert!(!snapshot.contains("super-secret-api-key"));
        assert!(!snapshot.to_ascii_lowercase().contains("api_key"));

        let reference = sentinel_provider_api::CredentialReference::for_provider(
            &ProviderId::new("gemini").unwrap(),
        );
        assert_eq!(
            credentials.state(&reference),
            sentinel_provider_api::CredentialState::Configured
        );
    }

    #[test]
    fn invalid_provider_mutations_fail_closed_without_changing_confirmed_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProviderSettingsStore::load(
            directory.path().join("api-providers.json"),
            Arc::new(MemoryCredentialStore::default()),
        )
        .unwrap();
        let before = store.snapshot();
        let mutation = ProviderSettingsMutation::SetModel {
            provider_id: ProviderId::new("missing").unwrap(),
            model_id: "model".into(),
        };
        assert!(apply_provider_settings_mutation(&store, mutation).is_err());
        assert_eq!(store.snapshot(), before);

        assert!(serde_json::from_str::<Request>(
            r#"{"kind":"provider_settings_mutation","request_id":"provider-2","mutation":{"action":"run_executable","path":"/tmp/script"}}"#,
        )
        .is_err());
    }

    #[tokio::test]
    async fn live_codex_usage_refresh_replaces_the_previous_percentage() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex.py");
        fs::write(
            &executable,
            r#"#!/usr/bin/python3
import json, sys
used = 23
for raw in sys.stdin:
    request = json.loads(raw)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion":"1"}
    elif method == "account/rateLimits/read":
        used += 1
        result = {"rateLimits":{"primary":{"usedPercent":used,"resetsAt":1999999999}}}
    else:
        result = {}
    print(json.dumps({"jsonrpc":"2.0","id":request["id"],"result":result}), flush=True)
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let repository = RunRepository::open(&format!(
            "sqlite://{}",
            directory.path().join("usage.sqlite3").display()
        ))
        .await
        .unwrap();
        let (changes, _receiver) = mpsc::channel();
        let mut collector = CodexUsageCollector::new(
            CodexProgram::from_executable(executable).ok(),
            repository,
            Some(directory.path().to_path_buf()),
            changes,
        );
        assert_eq!(
            collector.rate_limits(true).await.unwrap()["primary"]["usedPercent"],
            24
        );
        assert_eq!(
            collector.rate_limits(true).await.unwrap()["primary"]["usedPercent"],
            25
        );
    }
}
