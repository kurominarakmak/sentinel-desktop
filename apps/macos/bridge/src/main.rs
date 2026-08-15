//! Narrow, stdio-only service boundary for the native macOS shell.
//!
//! This process owns no workflow decisions. It maps durable V3 state to JSON
//! snapshots and forwards the already-existing final-approval supervisor call.

use sentinel_agent_api::{detect_installation, AgentKind, InstallationStatus};
use sentinel_codex::{CodexAppServer, CodexProgram, CodexTaskStarter, StartedCodexTask};
use sentinel_core::{
    v3::{ApprovalId, CreateTask, TaskId},
    CoreError, RunRepository,
};
use sentinel_git::inspect_repository;
use sentinel_provider_api::{
    openai_compatible::CustomProviderSpec,
    settings::{ProviderSettingsSnapshot, ProviderSettingsStore},
    MacOsKeychainCredentialStore, ProviderId, SecretString, WorkflowProviderConfiguration,
};
use sentinel_review::FinalApprovalSupervisor;
use sentinel_validation::load_repository_profiles;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
    StartTask {
        request_id: String,
        provider: String,
        summary: String,
        prompt: String,
    },
    AttentionAction {
        request_id: String,
        action: String,
        task_id: String,
        approval_id: Option<String>,
    },
    ProviderSettingsMutation {
        request_id: String,
        mutation: ProviderSettingsMutation,
    },
    Supervisor {
        command: String,
        approval_id: Option<String>,
        approve: Option<bool>,
    },
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDto {
    id: String,
    label: String,
    available: bool,
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

/// A native-shell-only account usage reader. Its provider protocol events live
/// in a separate database so usage polling can never become an active V3 task
/// or otherwise affect the workflow repository presented to Swift.
struct CodexUsageCollector {
    program: Option<CodexProgram>,
    repository: RunRepository,
    cwd: Option<PathBuf>,
    server: Option<CodexAppServer>,
}

impl CodexUsageCollector {
    fn new(program: Option<CodexProgram>, repository: RunRepository, cwd: Option<PathBuf>) -> Self {
        Self {
            program,
            repository,
            cwd,
            server: None,
        }
    }

    async fn rate_limits(&mut self) -> Option<Value> {
        if self.server.as_ref().is_some_and(CodexAppServer::is_alive) {
            return self
                .server
                .as_ref()
                .and_then(CodexAppServer::latest_rate_limits)
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

fn usage_database_path(database_path: &Path) -> PathBuf {
    let name = database_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sentinel-native");
    database_path.with_file_name(format!("{name}-codex-usage.sqlite3"))
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

async fn active_task(repository: &RunRepository) -> Result<Option<TaskDto>, CoreError> {
    let mut tasks = repository.v3().list_tasks().await?;
    tasks.sort_by_key(|task| (task.updated_at_ms, task.version));
    Ok(tasks.pop().map(task_dto))
}

fn attention_display_state(task: &sentinel_core::v3::Task) -> &'static str {
    use sentinel_core::v3::TaskLifecycle;
    if task.recovery_condition != sentinel_core::v3::RecoveryCondition::None {
        return "recovery_required";
    }
    match task.lifecycle {
        TaskLifecycle::AwaitingApproval => "waiting_for_approval",
        TaskLifecycle::ReadyForHuman | TaskLifecycle::Reviewing => "ready_for_review",
        TaskLifecycle::Failed | TaskLifecycle::Blocked => "failed",
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
    repository: &RunRepository,
    owned_tasks: &HashMap<TaskId, StartedCodexTask>,
) -> Result<Option<AttentionDto>, CoreError> {
    let Some(task) = active_task_record(repository).await? else {
        return Ok(None);
    };
    let id = task.id.clone();
    let events = repository.v3().list_events(&id).await?;
    let sessions = repository.v3().list_sessions_for_task(&id).await?;
    let validations = repository.v3().list_validation_results(&id).await?;
    let findings = repository.v3().list_review_findings(&id).await?;
    let approvals = repository.v3().list_approvals_for_task(&id).await?;
    let approval_id = approvals
        .iter()
        .rev()
        .find(|approval| {
            approval.action_kind == "v3_final_git_action"
                && approval.lifecycle == sentinel_core::v3::ApprovalLifecycle::Pending
        })
        .map(|approval| approval.id.to_string());
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
    let can_stop = !task.lifecycle.terminal()
        && task.recovery_condition == sentinel_core::v3::RecoveryCondition::None
        && owned_tasks.contains_key(&id);
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
            stop: can_stop,
            approve: approval_id.is_some(),
            reject: approval_id.is_some(),
            approval_id,
        },
        task: task_dto(task),
    }))
}

async fn active_task_record(
    repository: &RunRepository,
) -> Result<Option<sentinel_core::v3::Task>, CoreError> {
    let mut tasks = repository.v3().list_tasks().await?;
    tasks.sort_by_key(|task| (task.updated_at_ms, task.version));
    Ok(tasks.pop())
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
    usage: &mut CodexUsageCollector,
) -> Result<StatusDto, CoreError> {
    let task = active_task_record(repository).await?;
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
            rate_limits: usage.rate_limits().await,
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
    repository: &RunRepository,
    root: Option<&Path>,
    provider_store: &ProviderSettingsStore,
) -> Result<SettingsDto, CoreError> {
    let task_version = active_task_record(repository)
        .await?
        .map(|task| task.version)
        .unwrap_or(0);
    let provider_settings = provider_store.snapshot();
    let version = task_version
        .saturating_mul(1_000_000)
        .saturating_add(provider_settings.version);
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
    Ok(SettingsDto {
        version,
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
    task_id: String,
    owned_tasks: &HashMap<TaskId, StartedCodexTask>,
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
    let detail_actions = attention_state(repository, owned_tasks)
        .await?
        .filter(|state| state.task.id == task_id.0)
        .map(|state| state.actions)
        .unwrap_or(AttentionActionsDto {
            stop: false,
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
    Ok(json!({
        "task": task_dto(task),
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
    let path = std::env::var_os("AGENT_SENTINEL_CODEX_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|candidate| candidate.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("codex"))
                    .find(|candidate| candidate.is_file())
            })
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/bin/codex"))
                .filter(|candidate| candidate.is_file())
        });
    path.and_then(|path| CodexProgram::from_executable(path).ok())
}

fn repository_root() -> Option<PathBuf> {
    std::env::var_os("SENTINEL_REPOSITORY_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .filter(|path| path.is_dir())
}

async fn start_task(
    repository: &RunRepository,
    program: Option<CodexProgram>,
    root: &Path,
    provider: String,
    summary: String,
    prompt: String,
    owned_tasks: &mut HashMap<TaskId, StartedCodexTask>,
) -> Result<TaskDto, &'static str> {
    if provider != "codex" || prompt.trim().is_empty() || summary.trim().is_empty() {
        return Err("task submission was rejected");
    }
    if inspect_repository(root).await.is_err() {
        return Err("repository is unavailable or invalid");
    }
    let starter = CodexTaskStarter::new(program, repository.clone());
    let mut started = starter
        .start_task(summary, root)
        .await
        .map_err(|_| "Codex is unavailable")?;
    started
        .start_turn(&prompt)
        .await
        .map_err(|_| "Codex rejected the task prompt")?;
    let task = task_dto(started.task().clone());
    owned_tasks.insert(TaskId(task.id.clone()), started);
    Ok(task)
}

async fn perform_attention_action(
    repository: &RunRepository,
    owned_tasks: &mut HashMap<TaskId, StartedCodexTask>,
    action: String,
    task_id: String,
    approval_id: Option<String>,
) -> Result<(), &'static str> {
    let state = attention_state(repository, owned_tasks)
        .await
        .map_err(|_| "could not read task state")?
        .ok_or("there is no active task")?;
    if state.task.id != task_id {
        return Err("task is no longer active");
    }
    match (action.as_str(), approval_id) {
        ("stop", None) if state.actions.stop => owned_tasks
            .get_mut(&TaskId(task_id))
            .ok_or("task session is unavailable")?
            .cancel()
            .await
            .map_err(|_| "stop was rejected by the Rust supervisor"),
        ("approve", Some(approval_id))
            if state.actions.approve
                && state.actions.approval_id.as_deref() == Some(&approval_id) =>
        {
            FinalApprovalSupervisor::require_human_approval(
                repository.clone(),
                &ApprovalId(approval_id),
                true,
            )
            .await
            .map(|_| ())
            .map_err(|_| "approval was rejected by the Rust supervisor")
        }
        ("reject", Some(approval_id))
            if state.actions.reject
                && state.actions.approval_id.as_deref() == Some(&approval_id) =>
        {
            FinalApprovalSupervisor::require_human_approval(
                repository.clone(),
                &ApprovalId(approval_id),
                false,
            )
            .await
            .map(|_| ())
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("database path argument is required")?,
    );
    let database_url = format!("sqlite://{}", database_path.display());
    let usage_database_url = format!("sqlite://{}", usage_database_path(&database_path).display());
    let runtime = tokio::runtime::Runtime::new()?;
    let repository = runtime.block_on(RunRepository::open(&database_url))?;
    let provider_settings = ProviderSettingsStore::load(
        database_path.with_file_name("api-providers.json"),
        Arc::new(MacOsKeychainCredentialStore),
    )?;
    let root = repository_root();
    let program = codex_program();
    let usage_repository = runtime.block_on(RunRepository::open(&usage_database_url))?;
    let mut usage = CodexUsageCollector::new(program.clone(), usage_repository, root.clone());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(request) = serde_json::from_str::<Request>(&line) {
                let _ = sender.send(request);
            } else {
                response(json!({"kind":"error","message":"invalid native bridge request"}));
            }
        }
    });

    let mut subscribed = false;
    let mut last_snapshot = None;
    let mut last_status = None;
    let mut owned_tasks = HashMap::new();
    let mut accepted_submissions = HashMap::new();
    let mut completed_actions: HashMap<String, Value> = HashMap::new();
    let mut completed_provider_mutations: HashMap<String, (String, Value)> = HashMap::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(400)) {
            Ok(Request::Capabilities) => response(json!({
                "kind":"capabilities",
                "repository":root.as_ref().map(|path| path.display().to_string()),
                "providers":[
                    ProviderDto { id:"codex".into(), label:"Codex".into(), available:program.is_some() },
                    ProviderDto { id:"claude_code".into(), label:"Claude Code".into(), available:false }
                ]
            })),
            Ok(Request::ActiveTask) => match runtime.block_on(active_task(&repository)) {
                Ok(task) => response(json!({"kind":"active_task","task":task})),
                Err(_) => response(json!({"kind":"error","message":"could not read active task"})),
            },
            Ok(Request::AttentionState) => {
                match runtime.block_on(attention_state(&repository, &owned_tasks)) {
                    Ok(state) => response(json!({"kind":"attention_state","attention":state})),
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not read attention state"}))
                    }
                }
            }
            Ok(Request::Status) => match runtime.block_on(status_snapshot(&repository, &mut usage))
            {
                Ok(status) => response(json!({"kind":"status","status":status})),
                Err(_) => {
                    response(json!({"kind":"error","message":"could not read provider status"}))
                }
            },
            Ok(Request::Settings) => {
                match runtime.block_on(settings_snapshot(
                    &repository,
                    root.as_deref(),
                    &provider_settings,
                )) {
                    Ok(settings) => response(json!({"kind":"settings","settings":settings})),
                    Err(_) => response(json!({"kind":"error","message":"could not read settings"})),
                }
            }
            Ok(Request::TaskDetail { task_id }) => {
                match runtime.block_on(task_detail(&repository, task_id, &owned_tasks)) {
                    Ok(detail) => response(json!({"kind":"task_detail","detail":detail})),
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not read task detail"}))
                    }
                }
            }
            Ok(Request::Subscribe) => {
                subscribed = true;
                last_snapshot = None;
                response(json!({"kind":"subscribed"}));
            }
            Ok(Request::StartTask {
                request_id,
                provider,
                summary,
                prompt,
            }) => {
                if let Some(task) = accepted_submissions.get(&request_id) {
                    response(
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":true,"task":task}),
                    );
                    continue;
                }
                let result = match root.as_deref() {
                    Some(root) => runtime.block_on(start_task(
                        &repository,
                        program.clone(),
                        root,
                        provider,
                        summary,
                        prompt,
                        &mut owned_tasks,
                    )),
                    None => Err("repository is unavailable or invalid"),
                };
                match result {
                    Ok(task) => {
                        accepted_submissions.insert(request_id.clone(), task);
                        response(
                            json!({"kind":"task_start_result","requestId":request_id,"accepted":true,"task":accepted_submissions.get(&request_id)}),
                        );
                    }
                    Err(message) => response(
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":false,"message":message}),
                    ),
                }
            }
            Ok(Request::AttentionAction {
                request_id,
                action,
                task_id,
                approval_id,
            }) => {
                if let Some(result) = completed_actions.get(&request_id) {
                    response(result.clone());
                    continue;
                }
                let result = runtime.block_on(perform_attention_action(
                    &repository,
                    &mut owned_tasks,
                    action,
                    task_id,
                    approval_id,
                ));
                let payload = match result {
                    Ok(()) => {
                        json!({"kind":"attention_action_result","requestId":request_id,"accepted":true})
                    }
                    Err(message) => {
                        json!({"kind":"attention_action_result","requestId":request_id,"accepted":false,"message":message})
                    }
                };
                completed_actions.insert(request_id, payload.clone());
                response(payload);
            }
            Ok(Request::ProviderSettingsMutation {
                request_id,
                mutation,
            }) => {
                let fingerprint = serde_json::to_string(&mutation).unwrap_or_default();
                if let Some((previous_fingerprint, result)) =
                    completed_provider_mutations.get(&request_id)
                {
                    if previous_fingerprint == &fingerprint {
                        response(result.clone());
                    } else {
                        response(
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"the request ID conflicts with an existing provider settings intent"}),
                        );
                    }
                    continue;
                }
                let payload = match apply_provider_settings_mutation(&provider_settings, mutation) {
                    Ok(()) => match runtime.block_on(settings_snapshot(
                        &repository,
                        root.as_deref(),
                        &provider_settings,
                    )) {
                        Ok(settings) => {
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":true,"settings":settings})
                        }
                        Err(_) => {
                            json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":"confirmed provider settings could not be loaded"})
                        }
                    },
                    Err(message) => {
                        json!({"kind":"settings_mutation_result","requestId":request_id,"accepted":false,"message":message})
                    }
                };
                completed_provider_mutations.insert(request_id, (fingerprint, payload.clone()));
                response(payload);
            }
            Ok(Request::Supervisor {
                command,
                approval_id,
                approve,
            }) => {
                let result = match (command.as_str(), approval_id, approve) {
                    ("decide_final_approval", Some(approval_id), Some(approve)) => runtime
                        .block_on(FinalApprovalSupervisor::require_human_approval(
                            repository.clone(),
                            &ApprovalId(approval_id),
                            approve,
                        ))
                        .map_err(|_| ()),
                    _ => Err(()),
                };
                response(if result.is_ok() {
                    json!({"kind":"supervisor_result","ok":true})
                } else {
                    json!({"kind":"supervisor_result","ok":false,"message":"command was rejected by the Rust supervisor"})
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) if subscribed => {
                match runtime.block_on(attention_state(&repository, &owned_tasks)) {
                    Ok(attention) => {
                        let snapshot = serde_json::to_string(&attention).unwrap_or_default();
                        if last_snapshot.as_ref() != Some(&snapshot) {
                            last_snapshot = Some(snapshot);
                            response(json!({"kind":"attention_update","attention":attention}));
                        }
                        if let Ok(status) =
                            runtime.block_on(status_snapshot(&repository, &mut usage))
                        {
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
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_provider_api::{CredentialStore, MemoryCredentialStore};

    #[test]
    fn bridge_requests_are_narrow_and_typed() {
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"kind":"active_task"}"#),
            Ok(Request::ActiveTask)
        ));
        assert!(serde_json::from_str::<Request>(r#"{"kind":"direct_sql"}"#).is_err());
    }

    #[test]
    fn task_start_request_requires_provider_and_intent_fields() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"start_task","request_id":"request-1","provider":"codex","summary":"Task","prompt":"Do it"}"#
            ),
            Ok(Request::StartTask { .. })
        ));
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"start_task","provider":"codex"}"#).is_err()
        );
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
    fn provider_settings_requests_never_return_plaintext_keys() {
        let request = serde_json::from_str::<Request>(
            r#"{"kind":"provider_settings_mutation","request_id":"provider-1","mutation":{"action":"set_credential","provider_id":"glm","api_key":"super-secret-api-key"}}"#,
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
            &ProviderId::new("glm").unwrap(),
        );
        assert_eq!(
            credentials.state(&reference),
            sentinel_provider_api::CredentialState::Configured
        );
    }

    #[test]
    fn invalid_provider_mutations_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProviderSettingsStore::load(
            directory.path().join("api-providers.json"),
            Arc::new(MemoryCredentialStore::default()),
        )
        .unwrap();
        let before = store.snapshot();
        assert!(apply_provider_settings_mutation(
            &store,
            ProviderSettingsMutation::SetModel {
                provider_id: ProviderId::new("missing").unwrap(),
                model_id: "model".into(),
            },
        )
        .is_err());
        assert_eq!(store.snapshot(), before);
        assert!(serde_json::from_str::<Request>(
            r#"{"kind":"provider_settings_mutation","request_id":"provider-2","mutation":{"action":"run_executable","path":"/tmp/script"}}"#,
        )
        .is_err());
    }

    #[test]
    fn native_usage_database_is_isolated_from_workflow_state() {
        let workflow = PathBuf::from("/tmp/phase2.sqlite3");
        assert_eq!(
            usage_database_path(&workflow),
            PathBuf::from("/tmp/phase2-codex-usage.sqlite3")
        );
        assert_ne!(usage_database_path(&workflow), workflow);
    }
}
