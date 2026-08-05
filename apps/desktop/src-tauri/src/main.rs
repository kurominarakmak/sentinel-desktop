#[allow(dead_code)]
mod b2_a;
mod windowing;

use sentinel_agent_api::{
    codex_capabilities, detect_installation, AgentKind, CodexCapabilities, InstallationStatus,
};
use sentinel_core::{
    ApprovalDecision, ApprovalRequest, ApprovalState, ClaudeRunContext, CodexRunContext,
    CodexRunLifecycle, CoreError, CreateClaudeRunContext, CreateCodexRunContext, ManagedWorktree,
    ManagedWorktreeState, NormalizedAgentEvent, Project, ProjectFingerprintScheme, ProjectId,
    ProjectRegistration, ProjectValidationState, PublicRunReference, Run, RunId, RunRepository,
    TaskRequest, WorktreeId,
};
use sentinel_fake_agent::FakeAgentScenario;
use sentinel_git::{
    add_detached_worktree, extract_textual_diff, inspect_repository,
    inspect_worktree_changes_at_base, inspect_worktree_destination_no_follow,
    inspect_worktree_filter_attribute, inspect_worktree_numstat, resolve_exact_head,
    with_inventory_operation_context, with_inventory_operation_deadline, worktree_metadata_lookup,
    ChangedFile, FilterAttributeState, GitError, NumstatClassification, RepositoryInspection,
    RepositoryMode, RepositoryRelativePath, RepositoryState, TextEligibleMetadata,
    WorktreeDestinationState, WorktreeMetadataLookup,
};
use sentinel_runtime::{CancellationResult, CodexExecProgram, FakeAgentProgram, RunOrchestrator};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use windowing::{
    should_hide_on_close, tray_action, TrayAction, DEFAULT_GLOBAL_SHORTCUT, PROMPT_WINDOW_LABEL,
};

#[cfg(target_os = "macos")]
const MACOS_ACTIVATION_POLICY: tauri::ActivationPolicy = tauri::ActivationPolicy::Accessory;

#[derive(Serialize)]
struct Diagnostics {
    fake: String,
    codex: String,
    claude_code: String,
}
#[derive(Serialize)]
struct CodexCapabilityDto {
    exec_json: bool,
    app_server_experimental: bool,
}
/// Explicit Phase 7 public allowlist.  The queue never exposes private event
/// data, runtime IDs, paths, or audit rows.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalCapabilityDto {
    available: bool,
    hard_denied_categories: Vec<&'static str>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalOwnerRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    adapter: Option<String>,
    runtime_reference: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalLookupRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    approval_reference: String,
    adapter: Option<String>,
    runtime_reference: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalDecisionRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    approval_reference: String,
    expected_version: u64,
    decision: ApprovalDecision,
    adapter: Option<String>,
    runtime_reference: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRequestDto {
    approval_reference: String,
    profile: sentinel_core::ApprovalProfile,
    action_category: String,
    summary: String,
    state: ApprovalState,
    version: u64,
    decision_available: bool,
}
fn approval_request_dto(value: ApprovalRequest) -> ApprovalRequestDto {
    ApprovalRequestDto {
        approval_reference: value.public_reference.to_string(),
        profile: value.profile,
        action_category: value.action_category,
        summary: value.summary,
        state: value.state,
        version: value.version,
        decision_available: value.state == ApprovalState::Pending,
    }
}
/// The Phase 4 public boundary: no paths, database IDs, argv, environment,
/// raw events, prompts, or process information cross this DTO.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexRunDto {
    public_reference: String,
    status: CodexRunLifecycle,
    progress_summary: Option<String>,
    terminal_summary: Option<String>,
    error_category: Option<String>,
    cancellation_available: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRunStartRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    prompt: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRunDto {
    public_reference: String,
    status: CodexRunLifecycle,
    progress_summary: Option<String>,
    terminal_summary: Option<String>,
    error_category: Option<String>,
    cancellation_available: bool,
    followup_available: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRunStartRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    prompt: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRunLookupRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    public_reference: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeFollowupRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    public_reference: String,
    prompt: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRunLookupRequest {
    project_id: String,
    worktree_id: String,
    task_key: String,
    public_reference: String,
}
#[derive(Clone)]
struct DesktopState {
    repository: RunRepository,
    orchestrator: RunOrchestrator,
    codex_exec_available: bool,
    database_path: PathBuf,
    forwarders: Arc<tokio::sync::Mutex<HashSet<RunId>>>,
    protected_application_repository: Option<ProtectedRepository>,
    worktree_root: PathBuf,
    worktree_projects: Arc<tokio::sync::Mutex<HashSet<ProjectId>>>,
    #[cfg(test)]
    reconciliation_test_hooks: Option<ReconciliationTestHooks>,
    #[cfg(test)]
    b1_test_hooks: Option<B1TestHooks>,
}
#[cfg(test)]
#[derive(Clone)]
struct ReconciliationTestHooks {
    fail_leaf_canonicalization: bool,
    failure_reached: Option<Arc<tokio::sync::Notify>>,
    resume_failure: Option<Arc<tokio::sync::Notify>>,
}
#[cfg(test)]
#[derive(Clone)]
struct B1TestHooks {
    post_numstat_reached: Arc<tokio::sync::Notify>,
    resume_post_numstat: Arc<tokio::sync::Notify>,
    post_c2_equality_reached: Option<Arc<tokio::sync::Notify>>,
    resume_post_c2_equality: Option<Arc<tokio::sync::Notify>>,
    final_validation_reached: Option<Arc<tokio::sync::Notify>>,
    resume_final_validation: Option<Arc<tokio::sync::Notify>>,
    final_persisted_reload_reached: Option<Arc<tokio::sync::Notify>>,
    resume_final_persisted_reload: Option<Arc<tokio::sync::Notify>>,
    observations: Option<tokio::sync::mpsc::Sender<B1TestEvent>>,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum B1TestPass {
    C1,
    C2,
    C3,
}
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum B1TestInventoryStage {
    A,
    B,
    C,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum B1TestSurface {
    Staged,
    Unstaged,
}
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct B1TestInventoryObservation {
    file_count: usize,
    digest: u64,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct B1TestSectionObservation {
    kind: u8,
    additions: Option<u32>,
    deletions: Option<u32>,
}
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct B1TestEvidenceObservation {
    entry_count: usize,
    normalized_path_keys: Vec<u8>,
    staged: B1TestSectionObservation,
    unstaged: B1TestSectionObservation,
    mode_head: u8,
    mode_index: u8,
    mode_worktree: u8,
}
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum B1TestEvent {
    InventoryCompleted {
        stage: B1TestInventoryStage,
        observation: B1TestInventoryObservation,
    },
    ClassificationPassStarted {
        pass: B1TestPass,
    },
    NumstatCompleted {
        pass: B1TestPass,
        path_key: u8,
        surface: B1TestSurface,
        additions: Option<u32>,
        deletions: Option<u32>,
        binary: bool,
        mode_only: bool,
    },
    ClassificationPassCompleted {
        pass: B1TestPass,
        observation: B1TestEvidenceObservation,
    },
    EvidenceComparisonCompleted {
        equal: bool,
    },
    FinalEvidenceComparisonCompleted {
        equal: bool,
    },
    FinalPersistedStateReloadStarted,
    FinalPersistedStateReloadCompleted {
        lifecycle: B1TestLifecycle,
        accepted: bool,
    },
    FinalValidationStarted {
        stage: B1TestInventoryStage,
    },
    FinalValidationCompleted {
        stage: B1TestInventoryStage,
    },
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum B1TestLifecycle {
    Eligible,
    Removing,
    Other,
}
#[derive(Clone)]
struct ProtectedRepository {
    identity: String,
    fingerprint: String,
}
#[derive(Debug, Serialize)]
struct SafeError {
    code: &'static str,
    message: &'static str,
}
#[derive(Serialize)]
struct RuntimeEnvironment {
    schema_version: u16,
    database_initialized: bool,
    fake_agent_available: bool,
}
#[derive(Serialize)]
struct DesktopCapabilities {
    notifications: bool,
    autostart: bool,
    app_server_experimental: bool,
}
#[derive(Clone, Serialize)]
struct EventDto {
    schema_version: u16,
    run_id: String,
    sequence_number: u64,
    event_type: String,
    timestamp_ms: i64,
    payload: serde_json::Value,
}
/// Stable desktop API shape. Do not expose the domain `Run` serde layout directly.
#[derive(Clone, Debug, Serialize)]
struct RunDto {
    id: String,
    task_text: String,
    agent_kind: String,
    status: sentinel_core::RunStatus,
    schema_version: u16,
    created_at_ms: i64,
    started_at_ms: Option<i64>,
    finished_at_ms: Option<i64>,
    exit_code: Option<i32>,
    error_category: Option<String>,
    error_message: Option<String>,
    cancellable: bool,
}
#[derive(Deserialize)]
struct SubmitFakeRun {
    task_text: String,
    scenario: String,
}
#[derive(Deserialize)]
struct RegisterProjectRequest {
    directory: String,
    display_name: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
struct ProjectDto {
    id: String,
    display_name: String,
    validation_state: ProjectValidationState,
    is_primary_worktree: bool,
    branch: Option<String>,
    head: Option<String>,
    last_validated_at_ms: i64,
}
#[derive(Clone, Debug, Serialize)]
struct WorktreeDto {
    id: String,
    project_id: String,
    state: ManagedWorktreeState,
    base_commit: String,
    created_at_ms: i64,
    ready_at_ms: Option<i64>,
    removed_at_ms: Option<i64>,
    error_category: Option<String>,
}
/// The Phase 3C-C public textual-diff bridge. It is intentionally aggregate
/// only: source paths, line text, Git output, commands and private evidence
/// stay behind the desktop trust boundary.
#[derive(Clone, Debug, Serialize)]
struct TextualDiffDto {
    project_id: String,
    worktree_id: String,
    textual_available: bool,
    staged_additions: u64,
    staged_deletions: u64,
    unstaged_additions: u64,
    unstaged_deletions: u64,
    hunk_count: usize,
}

#[derive(Deserialize)]
struct InspectTextualDiffRequest {
    project_id: String,
    worktree_id: String,
    path: String,
}
/// Internal Phase 3C-A result.  It is deliberately not a Tauri DTO yet.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct WorktreeChangeInventory {
    worktree_id: WorktreeId,
    clean: bool,
    files: Vec<ChangedFile>,
}

/// Internal Phase 3C-B1 result. It deliberately contains classification
/// metadata only and is not a bridge DTO.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
struct FileDiffClassification {
    worktree_id: WorktreeId,
    path: RepositoryRelativePath,
    staged: DiffSectionClassification,
    unstaged: DiffSectionClassification,
    mode_head: Option<RepositoryMode>,
    mode_index: Option<RepositoryMode>,
    mode_worktree: Option<RepositoryMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffSectionClassification {
    NotApplicable,
    TextEligible(TextEligibleMetadata),
    Binary,
    ModeOnly,
    SymlinkMetadataOnly,
    SubmoduleMetadataOnly,
    UntrackedContentDeferred,
    ConflictContentDeferred,
    ExternalFilterDeferred,
    UnsupportedType,
}
const RUN_EVENT: &str = "phase2-run-event";

// Tauri requires the returned handle to outlive setup for the tray item to remain visible.
#[allow(dead_code)]
struct TrayState<R: tauri::Runtime>(TrayIcon<R>);

#[tauri::command]
fn spike_diagnostics() -> Diagnostics {
    Diagnostics {
        fake: "available (built-in deterministic sequence)".into(),
        codex: format_installation(detect_installation(AgentKind::Codex)),
        claude_code: format_installation(detect_installation(AgentKind::ClaudeCode)),
    }
}

#[tauri::command]
fn codex_capability() -> CodexCapabilityDto {
    let CodexCapabilities {
        exec_json,
        app_server_experimental,
    } = codex_capabilities(&detect_installation(AgentKind::Codex));
    CodexCapabilityDto {
        exec_json,
        app_server_experimental,
    }
}
fn parse_approval_owner(
    request: &ApprovalOwnerRequest,
) -> Result<
    (
        ProjectId,
        WorktreeId,
        Option<AgentKind>,
        Option<PublicRunReference>,
    ),
    SafeError,
> {
    if request.task_key.trim().is_empty() || request.task_key.len() > 128 {
        return Err(input_error());
    }
    let adapter = match request.adapter.as_deref() {
        None => None,
        Some("codex") => Some(AgentKind::Codex),
        Some("claude_code") => Some(AgentKind::ClaudeCode),
        _ => return Err(input_error()),
    };
    let runtime_reference = request
        .runtime_reference
        .as_ref()
        .map(|value| PublicRunReference::parse(value.clone()))
        .transpose()
        .map_err(|_| input_error())?;
    if adapter.is_some() != runtime_reference.is_some() {
        return Err(input_error());
    }
    Ok((
        ProjectId::from_str(&request.project_id).map_err(|_| input_error())?,
        WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?,
        adapter,
        runtime_reference,
    ))
}
#[tauri::command]
fn approval_capability() -> ApprovalCapabilityDto {
    ApprovalCapabilityDto {
        available: true,
        hard_denied_categories: vec![
            "merge",
            "push",
            "force_push",
            "install_cli",
            "unrestricted_permissions",
        ],
    }
}
#[tauri::command]
async fn list_pending_approvals(
    request: ApprovalOwnerRequest,
    state: State<'_, DesktopState>,
) -> Result<Vec<ApprovalRequestDto>, SafeError> {
    let (project, worktree, adapter, runtime_reference) = parse_approval_owner(&request)?;
    state
        .repository
        .list_pending_approval_requests_owned(
            &project,
            &worktree,
            &request.task_key,
            adapter,
            runtime_reference.as_ref(),
        )
        .await
        .map(|items| items.into_iter().map(approval_request_dto).collect())
        .map_err(|_| safe_error("approval queue"))
}
#[tauri::command]
async fn query_approval_request(
    request: ApprovalLookupRequest,
    state: State<'_, DesktopState>,
) -> Result<ApprovalRequestDto, SafeError> {
    let owner = ApprovalOwnerRequest {
        project_id: request.project_id.clone(),
        worktree_id: request.worktree_id.clone(),
        task_key: request.task_key.clone(),
        adapter: request.adapter.clone(),
        runtime_reference: request.runtime_reference.clone(),
    };
    let (project, worktree, adapter, runtime_reference) = parse_approval_owner(&owner)?;
    let reference =
        PublicRunReference::parse(request.approval_reference).map_err(|_| input_error())?;
    state
        .repository
        .get_approval_request_owned(
            &project,
            &worktree,
            &request.task_key,
            adapter,
            runtime_reference.as_ref(),
            &reference,
        )
        .await
        .map(approval_request_dto)
        .map_err(|_| safe_error("approval lookup"))
}
#[tauri::command]
async fn decide_approval_request(
    request: ApprovalDecisionRequest,
    state: State<'_, DesktopState>,
) -> Result<ApprovalRequestDto, SafeError> {
    let owner = ApprovalOwnerRequest {
        project_id: request.project_id.clone(),
        worktree_id: request.worktree_id.clone(),
        task_key: request.task_key.clone(),
        adapter: request.adapter.clone(),
        runtime_reference: request.runtime_reference.clone(),
    };
    let (project, worktree, adapter, runtime_reference) = parse_approval_owner(&owner)?;
    let reference =
        PublicRunReference::parse(request.approval_reference).map_err(|_| input_error())?;
    let current = state
        .repository
        .get_approval_request_owned(
            &project,
            &worktree,
            &request.task_key,
            adapter,
            runtime_reference.as_ref(),
            &reference,
        )
        .await
        .map_err(|_| safe_error("approval lookup"))?;
    if current.version != request.expected_version {
        return Err(safe_error("approval stale"));
    }
    state
        .repository
        .resolve_approval_request(&current, request.decision)
        .await
        .map(approval_request_dto)
        .map_err(|_| safe_error("approval decision"))
}

fn codex_run_dto(context: CodexRunContext) -> CodexRunDto {
    CodexRunDto {
        public_reference: context.public_reference.to_string(),
        status: context.lifecycle,
        progress_summary: context.progress_summary,
        terminal_summary: context.terminal_summary,
        error_category: context.failure_category,
        cancellation_available: !context.lifecycle.terminal() && !context.cancellation_requested,
    }
}
fn claude_run_dto(context: ClaudeRunContext) -> ClaudeRunDto {
    ClaudeRunDto {
        public_reference: context.public_reference.to_string(),
        status: context.lifecycle,
        progress_summary: context.progress_summary,
        terminal_summary: context.terminal_summary,
        error_category: context.failure_category,
        cancellation_available: !context.lifecycle.terminal() && !context.cancellation_requested,
        followup_available: context.lifecycle == CodexRunLifecycle::Running
            && !context.cancellation_requested
            && context.session_token.is_some(),
    }
}
fn parse_claude_lookup(
    request: &ClaudeRunLookupRequest,
) -> Result<(ProjectId, WorktreeId, PublicRunReference), SafeError> {
    Ok((
        ProjectId::from_str(&request.project_id).map_err(|_| input_error())?,
        WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?,
        PublicRunReference::parse(request.public_reference.clone()).map_err(|_| input_error())?,
    ))
}
#[tauri::command]
async fn query_claude_run(
    request: ClaudeRunLookupRequest,
    state: State<'_, DesktopState>,
) -> Result<ClaudeRunDto, SafeError> {
    let (p, w, r) = parse_claude_lookup(&request)?;
    state
        .repository
        .get_claude_run_context_owned(&p, &w, &request.task_key, &r)
        .await
        .map(claude_run_dto)
        .map_err(|_| safe_error("claude lookup"))
}
#[tauri::command]
async fn cancel_claude_run(
    request: ClaudeRunLookupRequest,
    state: State<'_, DesktopState>,
) -> Result<ClaudeRunDto, SafeError> {
    let (p, w, r) = parse_claude_lookup(&request)?;
    let c = state
        .repository
        .get_claude_run_context_owned(&p, &w, &request.task_key, &r)
        .await
        .map_err(|_| safe_error("claude lookup"))?;
    state
        .repository
        .request_claude_cancellation(&c)
        .await
        .map(claude_run_dto)
        .map_err(|_| safe_error("claude cancellation"))
}
#[tauri::command]
async fn followup_claude_run(
    request: ClaudeFollowupRequest,
    state: State<'_, DesktopState>,
) -> Result<ClaudeRunDto, SafeError> {
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 8_000
        || request.prompt.contains('\0')
    {
        return Err(input_error());
    };
    let lookup = ClaudeRunLookupRequest {
        project_id: request.project_id,
        worktree_id: request.worktree_id,
        task_key: request.task_key,
        public_reference: request.public_reference,
    };
    let (p, w, r) = parse_claude_lookup(&lookup)?;
    let c = state
        .repository
        .get_claude_run_context_owned(&p, &w, &lookup.task_key, &r)
        .await
        .map_err(|_| safe_error("claude lookup"))?;
    state
        .repository
        .reserve_claude_followup(&c)
        .await
        .map(claude_run_dto)
        .map_err(|_| safe_error("claude followup"))
}
#[tauri::command]
async fn start_claude_run(
    request: ClaudeRunStartRequest,
    state: State<'_, DesktopState>,
) -> Result<ClaudeRunDto, SafeError> {
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 8_000
        || request.prompt.contains('\0')
        || request.task_key.trim().is_empty()
        || request.task_key.len() > 128
    {
        return Err(input_error());
    };
    let p = ProjectId::from_str(&request.project_id).map_err(|_| input_error())?;
    let w = WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?;
    let wt = state
        .repository
        .get_worktree(&w)
        .await
        .map_err(|_| safe_error("claude worktree"))?;
    if wt.project_id != p || wt.state != ManagedWorktreeState::Ready {
        return Err(safe_error("claude ownership"));
    };
    let c = state
        .repository
        .create_claude_run_context(CreateClaudeRunContext {
            project_id: p,
            worktree_id: w,
            task_key: request.task_key,
        })
        .await
        .map_err(|_| safe_error("claude creation"))?;
    let failed = state
        .repository
        .transition_claude_run_context(
            &c,
            CodexRunLifecycle::Failed,
            None,
            Some("Claude execution is unavailable in this desktop build."),
            Some("claude_unavailable"),
            None,
        )
        .await
        .map_err(|_| safe_error("claude lifecycle"))?;
    Ok(claude_run_dto(failed))
}

fn parse_codex_lookup(
    request: &CodexRunLookupRequest,
) -> Result<(ProjectId, WorktreeId, PublicRunReference), SafeError> {
    Ok((
        ProjectId::from_str(&request.project_id).map_err(|_| input_error())?,
        WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?,
        PublicRunReference::parse(request.public_reference.clone()).map_err(|_| input_error())?,
    ))
}

#[tauri::command]
async fn query_codex_run(
    request: CodexRunLookupRequest,
    state: State<'_, DesktopState>,
) -> Result<CodexRunDto, SafeError> {
    let (project, worktree, reference) = parse_codex_lookup(&request)?;
    state
        .repository
        .get_codex_run_context_owned(&project, &worktree, &request.task_key, &reference)
        .await
        .map(codex_run_dto)
        .map_err(|_| safe_error("codex run lookup"))
}

#[tauri::command]
async fn cancel_codex_run(
    request: CodexRunLookupRequest,
    state: State<'_, DesktopState>,
) -> Result<CodexRunDto, SafeError> {
    let (project, worktree, reference) = parse_codex_lookup(&request)?;
    let context = state
        .repository
        .get_codex_run_context_owned(&project, &worktree, &request.task_key, &reference)
        .await
        .map_err(|_| safe_error("codex run lookup"))?;
    // There is deliberately no PID or arbitrary process lookup.  A future
    // configured runner must match this same persisted ownership tuple.
    let updated = state
        .repository
        .request_codex_cancellation(&context)
        .await
        .map_err(|_| safe_error("codex cancellation"))?;
    if !updated.lifecycle.terminal() {
        // Absence after restart is safe: persisted cancellation remains
        // authoritative and reconciliation will terminalize it.
        let _ = state.orchestrator.cancel_run(&updated.run_id).await;
    }
    Ok(codex_run_dto(updated))
}

#[tauri::command]
async fn start_codex_run(
    request: CodexRunStartRequest,
    state: State<'_, DesktopState>,
) -> Result<CodexRunDto, SafeError> {
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 8_000
        || request.prompt.contains('\0')
        || request.task_key.trim().is_empty()
        || request.task_key.len() > 128
    {
        return Err(input_error());
    }
    let project = ProjectId::from_str(&request.project_id).map_err(|_| input_error())?;
    let worktree = WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?;
    let project_row = state
        .repository
        .get_project(&project)
        .await
        .map_err(|_| safe_error("codex project"))?;
    let worktree_row = state
        .repository
        .get_worktree(&worktree)
        .await
        .map_err(|_| safe_error("codex worktree"))?;
    if worktree_row.project_id != project
        || worktree_row.state != ManagedWorktreeState::Ready
        || project_row.is_primary_worktree
        || worktree_row.path == project_row.primary_root
    {
        return Err(safe_error("codex ownership"));
    }
    let authoritative_worktree = validate_managed_leaf_for_reconciliation(
        &state,
        &project,
        &worktree,
        Path::new(&worktree_row.path),
    )?;
    let inspection = inspect_repository(&authoritative_worktree)
        .await
        .map_err(|_| safe_error("codex worktree validation"))?;
    if inspection.is_primary
        || inspection.identity != worktree_row.repository_identity
        || inspection.fingerprint.as_str() != worktree_row.repository_fingerprint
        || inspection.head.as_deref() != Some(worktree_row.base_commit.as_str())
    {
        return Err(safe_error("codex worktree validation"));
    }
    if !state.codex_exec_available {
        return Err(SafeError {
            code: "codex_unavailable",
            message: "Codex execution is unavailable in this desktop build.",
        });
    }
    let context = state
        .repository
        .create_codex_run_context(CreateCodexRunContext {
            project_id: project,
            worktree_id: worktree,
            task_key: request.task_key,
        })
        .await
        .map_err(|_| safe_error("codex run creation"))?;
    let starting = state
        .repository
        .transition_codex_run_context(
            &context,
            CodexRunLifecycle::Starting,
            Some("Execution is starting."),
            None,
            None,
        )
        .await
        .map_err(|_| safe_error("codex lifecycle"))?;
    if state
        .orchestrator
        .submit_existing_codex_run(
            starting.run_id.clone(),
            request.prompt,
            authoritative_worktree,
        )
        .await
        .is_err()
    {
        let _ = state
            .repository
            .transition_codex_run_context(
                &starting,
                CodexRunLifecycle::Failed,
                None,
                Some("Execution could not be started."),
                Some("start_failed"),
            )
            .await;
        return Err(safe_error("codex start"));
    }
    let running = match state
        .repository
        .transition_codex_run_context(
            &starting,
            CodexRunLifecycle::Running,
            Some("Execution is running."),
            None,
            None,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = state.orchestrator.cancel_run(&starting.run_id).await;
            let _ = state
                .repository
                .transition_codex_run_context(
                    &starting,
                    CodexRunLifecycle::Failed,
                    None,
                    Some("Execution could not be recorded."),
                    Some("persistence_failed"),
                )
                .await;
            return Err(safe_error("codex lifecycle"));
        }
    };
    let repository = state.repository.clone();
    let orchestrator = state.orchestrator.clone();
    let active = running.clone();
    tauri::async_runtime::spawn(async move {
        let terminal = match orchestrator.wait_for_run(&active.run_id).await {
            Ok(run)
                if run.status == sentinel_core::RunStatus::Completed
                    && !active.cancellation_requested =>
            {
                (CodexRunLifecycle::Succeeded, "Execution completed.", None)
            }
            Ok(run)
                if run.status == sentinel_core::RunStatus::Cancelled
                    || active.cancellation_requested =>
            {
                (
                    CodexRunLifecycle::Cancelled,
                    "Cancellation confirmed.",
                    None,
                )
            }
            _ => (
                CodexRunLifecycle::Failed,
                "Execution did not complete.",
                Some("execution_failed"),
            ),
        };
        if let Ok(current) = repository
            .get_codex_run_context_owned(
                &active.project_id,
                &active.worktree_id,
                &active.task_key,
                &active.public_reference,
            )
            .await
        {
            let next = if current.cancellation_requested {
                CodexRunLifecycle::Cancelled
            } else {
                terminal.0
            };
            let _ = repository
                .transition_codex_run_context(
                    &current,
                    next,
                    None,
                    Some(if next == CodexRunLifecycle::Cancelled {
                        "Cancellation confirmed."
                    } else {
                        terminal.1
                    }),
                    terminal.2,
                )
                .await;
        }
    });
    Ok(codex_run_dto(running))
}

fn safe_error(_: impl std::fmt::Debug) -> SafeError {
    SafeError {
        code: "phase2_error",
        message: "The requested operation could not be completed.",
    }
}
fn input_error() -> SafeError {
    SafeError {
        code: "invalid_input",
        message: "The request contains an invalid identifier or scenario.",
    }
}
fn project_error(error: &GitError) -> SafeError {
    match error {
        GitError::PathNotFound => SafeError {
            code: "path_not_found",
            message: "The selected folder is no longer available.",
        },
        GitError::NotDirectory => SafeError {
            code: "not_directory",
            message: "Select a folder containing a local Git working tree.",
        },
        GitError::NotRepository(_) => SafeError {
            code: "not_git_repository",
            message: "Select a local Git working tree.",
        },
        GitError::BareRepository => SafeError {
            code: "bare_repository_unsupported",
            message: "A working-tree repository is required.",
        },
        GitError::GitNotAvailable => SafeError {
            code: "git_not_available",
            message: "Git is not available on this device.",
        },
        GitError::InventoryUnavailable => SafeError {
            code: "inventory_unavailable",
            message: "A filter-free worktree inventory is not available yet.",
        },
        GitError::CleanlinessUnavailable => SafeError {
            code: "worktree_cleanliness_unavailable",
            message: "The worktree cannot be safely verified for removal.",
        },
        GitError::GitDiscoveryFailed => SafeError {
            code: "git_discovery_failed",
            message: "Git could not be safely prepared for validation.",
        },
        GitError::TimedOut => SafeError {
            code: "git_timeout",
            message: "Repository validation timed out.",
        },
        GitError::StdoutTooLarge | GitError::StderrTooLarge => SafeError {
            code: "git_output_too_large",
            message: "Repository validation returned too much output.",
        },
        GitError::MetadataInvalid | GitError::FingerprintUnavailable => SafeError {
            code: "repository_metadata_invalid",
            message: "The repository metadata could not be validated.",
        },
        _ => SafeError {
            code: "validation_failed",
            message: "The selected repository could not be validated.",
        },
    }
}
fn project_storage_error(error: CoreError) -> SafeError {
    match error {
        CoreError::DuplicateProject => SafeError {
            code: "duplicate_project",
            message: "This repository is already registered.",
        },
        CoreError::NotFound => SafeError {
            code: "project_not_found",
            message: "The registered project is no longer available.",
        },
        CoreError::RepositoryIdentityChanged => SafeError {
            code: "repository_identity_changed",
            message: "The registered repository was replaced or changed identity.",
        },
        _ => SafeError {
            code: "project_registry_failed",
            message: "The project registry could not be updated.",
        },
    }
}
fn project_dto(project: Project) -> ProjectDto {
    ProjectDto {
        id: project.id.to_string(),
        display_name: project.display_name,
        validation_state: project.validation_state,
        is_primary_worktree: project.is_primary_worktree,
        branch: project.branch,
        head: project.head,
        last_validated_at_ms: project.last_validated_at_ms,
    }
}
fn project_display_name(requested: Option<String>, inspection: &RepositoryInspection) -> String {
    let candidate = requested.unwrap_or_else(|| {
        inspection
            .repository_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Registered project")
            .to_owned()
    });
    let filtered: String = candidate
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect();
    if filtered.is_empty() {
        "Registered project".into()
    } else {
        filtered
    }
}
fn project_state(state: &RepositoryState) -> ProjectValidationState {
    match state {
        RepositoryState::Valid => ProjectValidationState::Valid,
        RepositoryState::Detached => ProjectValidationState::Detached,
        RepositoryState::Unborn => ProjectValidationState::Unborn,
        RepositoryState::LinkedWorktree => ProjectValidationState::LinkedWorktree,
    }
}
fn application_repository_root() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)?
        .canonicalize()
        .ok()
}
fn registration_from_inspection(
    inspection: RepositoryInspection,
    display_name: Option<String>,
    protected: Option<&ProtectedRepository>,
) -> Result<ProjectRegistration, SafeError> {
    if protected.is_some_and(|application| {
        application.identity == inspection.identity
            && application.fingerprint == inspection.fingerprint.as_str()
    }) {
        return Err(SafeError {
            code: "repository_unsupported",
            message: "This application repository cannot be registered.",
        });
    }
    Ok(ProjectRegistration {
        display_name: project_display_name(display_name, &inspection),
        repository_identity: inspection.identity,
        repository_fingerprint: inspection.fingerprint.as_str().to_owned(),
        fingerprint_scheme: ProjectFingerprintScheme::StrongV1,
        repository_root: inspection.repository_root.to_string_lossy().into_owned(),
        primary_root: inspection.primary_root.to_string_lossy().into_owned(),
        git_common_dir: inspection.common_dir.to_string_lossy().into_owned(),
        branch: inspection.branch,
        head: inspection.head,
        validation_state: project_state(&inspection.state),
        is_primary_worktree: inspection.is_primary,
    })
}
async fn validate_project_registration(
    directory: PathBuf,
    display_name: Option<String>,
    protected: Option<&ProtectedRepository>,
) -> Result<ProjectRegistration, SafeError> {
    let inspection = inspect_repository(&directory)
        .await
        .map_err(|error| project_error(&error))?;
    registration_from_inspection(inspection, display_name, protected)
}
fn fake_agent_filename() -> &'static str {
    if cfg!(windows) {
        "sentinel-fake-agent.exe"
    } else {
        "sentinel-fake-agent"
    }
}
fn bundled_sidecar_candidate(current_binary: &std::path::Path) -> Result<PathBuf, SafeError> {
    // `bundle.externalBin` consumes `binaries/sentinel-fake-agent-<target>` at
    // build time, then copies it beside the current desktop executable.
    current_binary
        .parent()
        .map(|directory| directory.join(fake_agent_filename()))
        .ok_or(SafeError {
            code: "fake_agent_unavailable",
            message: "The trusted fake-agent executable is unavailable.",
        })
}
fn resolve_fake_agent_path(
    override_path: Option<PathBuf>,
    current_binary: PathBuf,
) -> Result<PathBuf, SafeError> {
    let path = match override_path {
        Some(path) => path,
        None => bundled_sidecar_candidate(&current_binary)?,
    };
    let canonical = path.canonicalize().map_err(|_| SafeError {
        code: "fake_agent_unavailable",
        message: "The trusted fake-agent executable is unavailable.",
    })?;
    let metadata = std::fs::metadata(&canonical).map_err(|_| SafeError {
        code: "fake_agent_unavailable",
        message: "The trusted fake-agent executable is unavailable.",
    })?;
    if !metadata.is_file() {
        return Err(SafeError {
            code: "fake_agent_unavailable",
            message: "The trusted fake-agent executable is unavailable.",
        });
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(SafeError {
            code: "fake_agent_unavailable",
            message: "The trusted fake-agent executable is unavailable.",
        });
    }
    Ok(canonical)
}
fn event_dto(event: NormalizedAgentEvent) -> EventDto {
    EventDto {
        schema_version: event.schema_version,
        run_id: event.run_id.to_string(),
        sequence_number: event.sequence_number,
        event_type: event.event_type,
        timestamp_ms: event.occurred_at_ms,
        payload: event.payload,
    }
}
fn terminal_event(event_type: &str) -> bool {
    matches!(event_type, "run_completed" | "run_failed" | "run_cancelled")
}
/// Replays the persisted source of truth. Emission is best effort: the sequence
/// advances even when a window cannot receive an event, avoiding retry loops;
/// `list_run_events` remains available for history recovery.
async fn replay_persisted_events(
    repository: &RunRepository,
    run_id: &RunId,
    app: &AppHandle,
    last_sequence: &mut u64,
) -> Result<bool, ()> {
    let mut events = repository.list_events(run_id).await.map_err(|_| ())?;
    events.sort_by_key(|event| event.sequence_number);
    for event in events {
        if event.sequence_number <= *last_sequence {
            continue;
        }
        let terminal = terminal_event(&event.event_type);
        *last_sequence = event.sequence_number;
        let _ = app.emit(RUN_EVENT, event_dto(event));
        if terminal {
            return Ok(true);
        }
    }
    Ok(false)
}
async fn run_dto(state: &DesktopState, run: sentinel_core::Run) -> RunDto {
    let cancellable = state.orchestrator.is_cancellable(&run.id).await;
    run_dto_with_capability(run, cancellable)
}
fn run_dto_with_capability(run: Run, cancellable: bool) -> RunDto {
    let (error_category, error_message) = match run.error {
        Some(error) => (Some(error.category), Some(error.message)),
        None => (None, None),
    };
    RunDto {
        id: run.id.to_string(),
        task_text: run.task_text,
        agent_kind: match run.agent {
            AgentKind::Fake => "fake",
            AgentKind::Codex => "codex",
            AgentKind::ClaudeCode => "claude_code",
        }
        .into(),
        status: run.status,
        schema_version: run.schema_version,
        created_at_ms: run.created_at_ms,
        started_at_ms: run.started_at_ms,
        finished_at_ms: run.finished_at_ms,
        exit_code: run.exit_code,
        error_category,
        error_message,
        cancellable,
    }
}
#[tauri::command]
async fn list_runs(state: State<'_, DesktopState>) -> Result<Vec<RunDto>, SafeError> {
    let runs = state
        .repository
        .list_recent_runs()
        .await
        .map_err(safe_error)?;
    let mut result = Vec::with_capacity(runs.len());
    for run in runs {
        result.push(run_dto(&state, run).await);
    }
    Ok(result)
}
#[tauri::command]
async fn list_run_events(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<Vec<EventDto>, SafeError> {
    state
        .repository
        .list_events(&RunId::from_str(&id).map_err(|_| input_error())?)
        .await
        .map(|events| events.into_iter().map(event_dto).collect())
        .map_err(safe_error)
}
#[tauri::command]
async fn get_run(id: String, state: State<'_, DesktopState>) -> Result<RunDto, SafeError> {
    let run = state
        .repository
        .get_run(&RunId::from_str(&id).map_err(|_| input_error())?)
        .await
        .map_err(safe_error)?;
    Ok(run_dto(&state, run).await)
}
#[tauri::command]
fn get_runtime_environment(state: State<'_, DesktopState>) -> RuntimeEnvironment {
    RuntimeEnvironment {
        schema_version: 1,
        database_initialized: state.database_path.is_file(),
        fake_agent_available: true,
    }
}
/// Phase 6 reports unsupported OS integrations explicitly rather than
/// changing login-item or notification permissions behind the user's back.
#[tauri::command]
fn desktop_capabilities() -> DesktopCapabilities {
    DesktopCapabilities {
        notifications: false,
        autostart: false,
        app_server_experimental: false,
    }
}
#[tauri::command]
async fn register_project(
    request: RegisterProjectRequest,
    state: State<'_, DesktopState>,
) -> Result<ProjectDto, SafeError> {
    let registration = validate_project_registration(
        PathBuf::from(request.directory),
        request.display_name,
        state.protected_application_repository.as_ref(),
    )
    .await?;
    state
        .repository
        .register_project(registration)
        .await
        .map(project_dto)
        .map_err(project_storage_error)
}
#[tauri::command]
async fn list_projects(state: State<'_, DesktopState>) -> Result<Vec<ProjectDto>, SafeError> {
    state
        .repository
        .list_projects()
        .await
        .map(|projects| projects.into_iter().map(project_dto).collect())
        .map_err(project_storage_error)
}
#[tauri::command]
async fn get_project(id: String, state: State<'_, DesktopState>) -> Result<ProjectDto, SafeError> {
    let id = ProjectId::from_str(&id).map_err(|_| input_error())?;
    state
        .repository
        .get_project(&id)
        .await
        .map(project_dto)
        .map_err(project_storage_error)
}
#[tauri::command]
async fn revalidate_project(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<ProjectDto, SafeError> {
    let id = ProjectId::from_str(&id).map_err(|_| input_error())?;
    let existing = state
        .repository
        .get_project(&id)
        .await
        .map_err(project_storage_error)?;
    let registration = validate_project_registration(
        PathBuf::from(existing.repository_root),
        Some(existing.display_name),
        state.protected_application_repository.as_ref(),
    )
    .await
    .map_err(|error| {
        if matches!(error.code, "path_not_found" | "not_directory") {
            SafeError {
                code: "repository_moved_or_missing",
                message: "The registered repository was moved or is no longer available.",
            }
        } else {
            error
        }
    })?;
    state
        .repository
        .revalidate_project(&id, registration)
        .await
        .map(project_dto)
        .map_err(project_storage_error)
}
#[tauri::command]
async fn unregister_project(id: String, state: State<'_, DesktopState>) -> Result<(), SafeError> {
    let id = ProjectId::from_str(&id).map_err(|_| input_error())?;
    state
        .repository
        .unregister_project(&id)
        .await
        .map_err(project_storage_error)
}

fn worktree_dto(worktree: ManagedWorktree) -> WorktreeDto {
    WorktreeDto {
        id: worktree.id.to_string(),
        project_id: worktree.project_id.to_string(),
        state: worktree.state,
        base_commit: worktree.base_commit,
        created_at_ms: worktree.created_at_ms,
        ready_at_ms: worktree.ready_at_ms,
        removed_at_ms: worktree.removed_at_ms,
        error_category: worktree.error_category,
    }
}
fn worktree_error(_: impl std::fmt::Debug) -> SafeError {
    SafeError {
        code: "worktree_operation_failed",
        message: "The managed worktree operation could not be completed.",
    }
}
#[allow(dead_code)]
fn inventory_error(error: GitError) -> SafeError {
    match error {
        GitError::GitNotAvailable => SafeError {
            code: "git_not_available",
            message: "Git is not available on this device.",
        },
        GitError::TimedOut => SafeError {
            code: "git_command_failed",
            message: "The worktree inventory could not be completed.",
        },
        GitError::StdoutTooLarge | GitError::StderrTooLarge | GitError::StatusTooManyRecords => {
            SafeError {
                code: "git_output_too_large",
                message: "The worktree inventory exceeds its safe limit.",
            }
        }
        GitError::StatusMalformed | GitError::MetadataInvalid => SafeError {
            code: "git_output_malformed",
            message: "The worktree inventory could not be verified.",
        },
        _ => SafeError {
            code: "git_command_failed",
            message: "The worktree inventory could not be completed.",
        },
    }
}
fn validate_managed_directory(path: &Path) -> Result<(), SafeError> {
    let metadata = std::fs::symlink_metadata(path).map_err(worktree_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(worktree_error("unsafe managed root component"));
    }
    #[cfg(windows)]
    if std::os::windows::fs::MetadataExt::file_attributes(&metadata) & 0x400 != 0 {
        return Err(worktree_error("reparse-point managed root component"));
    }
    Ok(())
}
fn ensure_managed_directory(parent: &Path, name: &str) -> Result<PathBuf, SafeError> {
    validate_managed_directory(parent)?;
    let child = parent.join(name);
    match std::fs::symlink_metadata(&child) {
        Ok(_) => validate_managed_directory(&child)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&child).map_err(worktree_error)?;
            validate_managed_directory(&child)?;
        }
        Err(error) => return Err(worktree_error(error)),
    }
    Ok(child)
}
fn worktree_parent(root: &Path, project: &ProjectId) -> Result<PathBuf, SafeError> {
    let base = root
        .parent()
        .ok_or_else(|| worktree_error("missing application-data parent"))?;
    validate_managed_directory(base)?;
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| worktree_error("invalid root"))?;
    let configured_root = ensure_managed_directory(base, root_name)?;
    let canonical_root = configured_root.canonicalize().map_err(worktree_error)?;
    let parent = ensure_managed_directory(&configured_root, &project.to_string())?;
    let canonical_parent = parent.canonicalize().map_err(worktree_error)?;
    if canonical_parent.parent() != Some(canonical_root.as_path()) {
        return Err(worktree_error("containment"));
    }
    Ok(canonical_parent)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagedLeafState {
    Missing,
    RealDirectory,
    UnsafeLinkOrReparse,
    NonDirectory,
    MetadataUnavailable,
}

/// Checks the backend-derived exact leaf without following it. Missing is
/// returned only for a no-follow `NotFound`; every unsafe or unavailable state
/// stays distinct for removal and reconciliation.
fn inspect_managed_leaf(
    root: &Path,
    project: &ProjectId,
    worktree: &WorktreeId,
    stored: &Path,
) -> Result<ManagedLeafState, SafeError> {
    let parent = worktree_parent(root, project)?;
    let expected = parent.join(worktree.to_string());
    if stored != expected {
        return Err(worktree_error("managed leaf ownership"));
    }
    Ok(match inspect_worktree_destination_no_follow(stored) {
        WorktreeDestinationState::Missing => ManagedLeafState::Missing,
        WorktreeDestinationState::RealDirectory => ManagedLeafState::RealDirectory,
        WorktreeDestinationState::UnsafeLinkOrReparse => ManagedLeafState::UnsafeLinkOrReparse,
        WorktreeDestinationState::NonDirectory => ManagedLeafState::NonDirectory,
        WorktreeDestinationState::MetadataUnavailable => ManagedLeafState::MetadataUnavailable,
    })
}

/// Validates the persisted, backend-generated leaf itself before anything is
/// allowed to follow it. The returned path is the only canonical leaf callers
/// may give to Git inspection or mutation APIs.
fn validate_managed_leaf(
    root: &Path,
    project: &ProjectId,
    worktree: &WorktreeId,
    stored: &Path,
) -> Result<PathBuf, SafeError> {
    validate_managed_leaf_with_canonicalize(root, project, worktree, stored, |path| {
        path.canonicalize()
    })
}

fn validate_managed_leaf_with_canonicalize(
    root: &Path,
    project: &ProjectId,
    worktree: &WorktreeId,
    stored: &Path,
    canonicalize: impl FnOnce(&Path) -> std::io::Result<PathBuf>,
) -> Result<PathBuf, SafeError> {
    if inspect_managed_leaf(root, project, worktree, stored)? != ManagedLeafState::RealDirectory {
        return Err(worktree_error("unsafe managed leaf"));
    }
    let canonical = canonicalize(stored).map_err(worktree_error)?;
    let parent = worktree_parent(root, project)?;
    let expected = parent.join(worktree.to_string());
    if canonical != expected || canonical.parent() != Some(parent.as_path()) {
        return Err(worktree_error("managed leaf containment"));
    }
    Ok(canonical)
}

fn validate_managed_leaf_for_reconciliation(
    state: &DesktopState,
    project: &ProjectId,
    worktree: &WorktreeId,
    stored: &Path,
) -> Result<PathBuf, SafeError> {
    #[cfg(test)]
    if state
        .reconciliation_test_hooks
        .as_ref()
        .is_some_and(|hooks| hooks.fail_leaf_canonicalization)
    {
        return validate_managed_leaf_with_canonicalize(
            &state.worktree_root,
            project,
            worktree,
            stored,
            |_| Err(std::io::Error::other("test canonicalization failure")),
        );
    }
    validate_managed_leaf(&state.worktree_root, project, worktree, stored)
}

#[cfg(test)]
async fn pause_before_reconciliation_failure_transition(state: &DesktopState) {
    let Some(hooks) = state.reconciliation_test_hooks.as_ref() else {
        return;
    };
    if let Some(reached) = &hooks.failure_reached {
        reached.notify_one();
    }
    if let Some(resume) = &hooks.resume_failure {
        resume.notified().await;
    }
}

/// Test-only coordination point for proving that B1's final locked inventory
/// validation rejects a lifecycle transition made after all numstat commands.
#[cfg(test)]
async fn pause_after_b1_numstat(state: &DesktopState) {
    let Some(hooks) = state.b1_test_hooks.as_ref() else {
        return;
    };
    hooks.post_numstat_reached.notify_one();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        hooks.resume_post_numstat.notified(),
    )
    .await;
}

#[cfg(test)]
async fn pause_after_b1_c2_equality(state: &DesktopState) {
    let Some(hooks) = state.b1_test_hooks.as_ref() else {
        return;
    };
    if let Some(reached) = &hooks.post_c2_equality_reached {
        reached.notify_one();
    }
    if let Some(resume) = &hooks.resume_post_c2_equality {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), resume.notified()).await;
    }
}
#[cfg(test)]
fn observe_b1(state: &DesktopState, event: B1TestEvent) {
    if let Some(sender) = state
        .b1_test_hooks
        .as_ref()
        .and_then(|hooks| hooks.observations.as_ref())
    {
        let _ = sender.try_send(event);
    }
}

/// Test-only coordination point at the real post-inventory validation boundary.
#[cfg(test)]
async fn pause_before_b1_final_validation(state: &DesktopState) {
    let Some(hooks) = state.b1_test_hooks.as_ref() else {
        return;
    };
    if let Some(reached) = &hooks.final_validation_reached {
        reached.notify_one();
    }
    if let Some(resume) = &hooks.resume_final_validation {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), resume.notified()).await;
    }
}

#[cfg(test)]
async fn pause_before_b1_final_persisted_reload(state: &DesktopState) {
    let Some(hooks) = state.b1_test_hooks.as_ref() else {
        return;
    };
    if let Some(reached) = &hooks.final_persisted_reload_reached {
        reached.notify_one();
    }
    if let Some(resume) = &hooks.resume_final_persisted_reload {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), resume.notified()).await;
    }
}

#[cfg(test)]
fn b1_test_inventory_observation(
    inventory: &WorktreeChangeInventory,
) -> B1TestInventoryObservation {
    // FNV-1a produces a deterministic, path-sanitising digest of precisely the
    // inventory fields the production equality check compares.
    let mut digest = 0xcbf29ce484222325_u64;
    for file in &inventory.files {
        for byte in file.path.as_str().as_bytes() {
            digest ^= u64::from(*byte);
            digest = digest.wrapping_mul(0x100000001b3);
        }
        for value in [
            b1_test_change_kind(file.index_change),
            b1_test_change_kind(file.worktree_change),
            b1_test_conflict_kind(file.conflict),
            u8::from(file.untracked),
            b1_test_mode(file.mode_head),
            b1_test_mode(file.mode_index),
            b1_test_mode(file.mode_worktree),
        ] {
            digest ^= u64::from(value);
            digest = digest.wrapping_mul(0x100000001b3);
        }
        digest ^= u64::from(u8::from(file.submodule.is_some()));
        digest = digest.wrapping_mul(0x100000001b3);
        if let Some(submodule) = file.submodule {
            for value in [
                u8::from(submodule.commit_changed),
                u8::from(submodule.modified),
                u8::from(submodule.untracked),
            ] {
                digest ^= u64::from(value);
                digest = digest.wrapping_mul(0x100000001b3);
            }
        }
    }
    B1TestInventoryObservation {
        file_count: inventory.files.len(),
        digest,
    }
}

#[cfg(test)]
fn b1_test_change_kind(kind: Option<sentinel_git::ChangeKind>) -> u8 {
    match kind {
        None => 0,
        Some(sentinel_git::ChangeKind::Added) => 1,
        Some(sentinel_git::ChangeKind::Modified) => 2,
        Some(sentinel_git::ChangeKind::Deleted) => 3,
        Some(sentinel_git::ChangeKind::TypeChanged) => 4,
    }
}

#[cfg(test)]
fn b1_test_conflict_kind(kind: Option<sentinel_git::ConflictKind>) -> u8 {
    match kind {
        None => 0,
        Some(sentinel_git::ConflictKind::BothDeleted) => 1,
        Some(sentinel_git::ConflictKind::AddedByUs) => 2,
        Some(sentinel_git::ConflictKind::DeletedByThem) => 3,
        Some(sentinel_git::ConflictKind::AddedByThem) => 4,
        Some(sentinel_git::ConflictKind::DeletedByUs) => 5,
        Some(sentinel_git::ConflictKind::BothAdded) => 6,
        Some(sentinel_git::ConflictKind::BothModified) => 7,
    }
}

#[cfg(test)]
fn b1_test_mode(mode: Option<RepositoryMode>) -> u8 {
    match mode {
        None => 0,
        Some(RepositoryMode::Absent) => 1,
        Some(RepositoryMode::Regular) => 2,
        Some(RepositoryMode::Executable) => 3,
        Some(RepositoryMode::SymbolicLink) => 4,
        Some(RepositoryMode::Gitlink) => 5,
    }
}

#[cfg(test)]
fn b1_test_section_observation(section: &DiffSectionClassification) -> B1TestSectionObservation {
    match section {
        DiffSectionClassification::NotApplicable => B1TestSectionObservation {
            kind: 0,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::TextEligible(metadata) => B1TestSectionObservation {
            kind: 1,
            additions: Some(metadata.additions),
            deletions: Some(metadata.deletions),
        },
        DiffSectionClassification::Binary => B1TestSectionObservation {
            kind: 2,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::ModeOnly => B1TestSectionObservation {
            kind: 3,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::SymlinkMetadataOnly => B1TestSectionObservation {
            kind: 4,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::SubmoduleMetadataOnly => B1TestSectionObservation {
            kind: 5,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::UntrackedContentDeferred => B1TestSectionObservation {
            kind: 6,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::ConflictContentDeferred => B1TestSectionObservation {
            kind: 7,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::ExternalFilterDeferred => B1TestSectionObservation {
            kind: 8,
            additions: None,
            deletions: None,
        },
        DiffSectionClassification::UnsupportedType => B1TestSectionObservation {
            kind: 9,
            additions: None,
            deletions: None,
        },
    }
}

#[cfg(test)]
fn b1_test_evidence_observation(evidence: &FileDiffClassification) -> B1TestEvidenceObservation {
    B1TestEvidenceObservation {
        entry_count: 1,
        normalized_path_keys: vec![0],
        staged: b1_test_section_observation(&evidence.staged),
        unstaged: b1_test_section_observation(&evidence.unstaged),
        mode_head: b1_test_mode(evidence.mode_head),
        mode_index: b1_test_mode(evidence.mode_index),
        mode_worktree: b1_test_mode(evidence.mode_worktree),
    }
}

#[cfg(test)]
fn observe_b1_numstat(
    state: &DesktopState,
    pass: B1TestPass,
    surface: B1TestSurface,
    section: &DiffSectionClassification,
) {
    let (additions, deletions, binary, mode_only) = match section {
        DiffSectionClassification::TextEligible(metadata) => (
            Some(metadata.additions),
            Some(metadata.deletions),
            false,
            false,
        ),
        DiffSectionClassification::Binary => (None, None, true, false),
        DiffSectionClassification::ModeOnly => (None, None, false, true),
        // These outcomes do not follow a completed numstat observation.
        DiffSectionClassification::NotApplicable
        | DiffSectionClassification::SymlinkMetadataOnly
        | DiffSectionClassification::SubmoduleMetadataOnly
        | DiffSectionClassification::UntrackedContentDeferred
        | DiffSectionClassification::ConflictContentDeferred
        | DiffSectionClassification::ExternalFilterDeferred
        | DiffSectionClassification::UnsupportedType => return,
    };
    observe_b1(
        state,
        B1TestEvent::NumstatCompleted {
            pass,
            path_key: 0,
            surface,
            additions,
            deletions,
            binary,
            mode_only,
        },
    );
}

#[cfg(test)]
fn post_remove_state(
    leaf: Result<ManagedLeafState, SafeError>,
    metadata: Result<WorktreeMetadataLookup, GitError>,
) -> ManagedWorktreeState {
    if matches!(leaf, Ok(ManagedLeafState::Missing))
        && matches!(metadata, Ok(WorktreeMetadataLookup::Absent))
    {
        ManagedWorktreeState::Removed
    } else {
        ManagedWorktreeState::Failed
    }
}

fn reconciliation_validation_failure_state(
    state: ManagedWorktreeState,
) -> Option<ManagedWorktreeState> {
    match state {
        ManagedWorktreeState::Creating | ManagedWorktreeState::Ready => {
            Some(ManagedWorktreeState::Failed)
        }
        _ => None,
    }
}

async fn acquire_project_worktree_lock(
    state: &DesktopState,
    project: &ProjectId,
) -> Result<(), SafeError> {
    let mut active = state.worktree_projects.lock().await;
    if !active.insert(project.clone()) {
        return Err(SafeError {
            code: "worktree_busy",
            message: "Another worktree operation is already in progress for this project.",
        });
    }
    Ok(())
}
async fn release_project_worktree_lock(state: &DesktopState, project: &ProjectId) {
    state.worktree_projects.lock().await.remove(project);
}
async fn strict_project_for_worktree(
    state: &DesktopState,
    id: &ProjectId,
) -> Result<Project, SafeError> {
    let project = state
        .repository
        .get_project(id)
        .await
        .map_err(project_storage_error)?;
    if project.fingerprint_scheme != ProjectFingerprintScheme::StrongV1
        || project.validation_state == ProjectValidationState::RequiresTrustedRevalidation
    {
        return Err(SafeError {
            code: "project_requires_revalidation",
            message: "The project requires trusted revalidation before a worktree can be created.",
        });
    }
    if project.head.is_none() {
        return Err(SafeError {
            code: "project_unborn_head",
            message: "A project without a commit cannot create a worktree.",
        });
    }
    let inspection = inspect_repository(Path::new(&project.repository_root))
        .await
        .map_err(|error| project_error(&error))?;
    if inspection.identity != project.repository_identity
        || inspection.fingerprint.as_str() != project.repository_fingerprint
    {
        return Err(SafeError {
            code: "project_identity_changed",
            message: "The registered repository was replaced or changed identity.",
        });
    }
    Ok(project)
}

#[allow(dead_code)]
fn inventory_eligible(state: ManagedWorktreeState) -> bool {
    matches!(
        state,
        ManagedWorktreeState::Ready | ManagedWorktreeState::RetainedDirty
    )
}

/// Shared production Phase 3C-A inspection orchestration.  It intentionally
/// remains internal until Phase 3C-C defines the public bridge contract.
#[allow(dead_code)]
async fn inspect_worktree_changes_impl(
    state: &DesktopState,
    worktree_id: WorktreeId,
) -> Result<WorktreeChangeInventory, SafeError> {
    let initial = state
        .repository
        .get_worktree(&worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    acquire_project_worktree_lock(state, &initial.project_id).await?;
    let project_id = initial.project_id.clone();
    let result = with_inventory_operation_deadline(inspect_worktree_changes_locked(
        state,
        &worktree_id,
        &project_id,
        #[cfg(test)]
        None,
    ))
    .await;
    release_project_worktree_lock(state, &project_id).await;
    result
}

/// The lock-owning callers above and in Phase 3C-B1 share this exact
/// inventory/ownership orchestration. Keeping it locked avoids recursive
/// ProjectId acquisition while preserving the Phase 3C-A validation sequence.
async fn inspect_worktree_changes_locked(
    state: &DesktopState,
    worktree_id: &WorktreeId,
    project_id: &ProjectId,
    #[cfg(test)] b1_inventory_stage: Option<B1TestInventoryStage>,
) -> Result<WorktreeChangeInventory, SafeError> {
    #[cfg(test)]
    let _ = b1_inventory_stage;
    let row = state
        .repository
        .get_worktree(worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    if row.project_id != *project_id || !inventory_eligible(row.state) {
        return Err(SafeError {
            code: "worktree_not_ready",
            message: "This worktree is not available for inspection.",
        });
    }
    let project = strict_project_for_worktree(state, project_id).await?;
    let stored = PathBuf::from(&row.path);
    let leaf = validate_managed_leaf(&state.worktree_root, project_id, &row.id, &stored).map_err(
        |_| SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        },
    )?;
    let inspection = inspect_repository(&leaf).await.map_err(inventory_error)?;
    if inspection.is_primary
        || inspection.identity != row.repository_identity
        || inspection.fingerprint.as_str() != row.repository_fingerprint
        || inspection.head.as_deref() != Some(row.base_commit.as_str())
        || !matches!(
            worktree_metadata_lookup(Path::new(&project.repository_root), &leaf).await,
            Ok(WorktreeMetadataLookup::Present)
        )
    {
        return Err(SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        });
    }
    let files = inspect_worktree_changes_at_base(&leaf, &row.base_commit)
        .await
        .map_err(inventory_error)?;
    let final_inspection = inspect_repository(&leaf).await.map_err(inventory_error)?;
    if final_inspection.is_primary
        || final_inspection.identity != row.repository_identity
        || final_inspection.fingerprint.as_str() != row.repository_fingerprint
        || final_inspection.head.as_deref() != Some(row.base_commit.as_str())
        || !matches!(
            worktree_metadata_lookup(Path::new(&project.repository_root), &leaf).await,
            Ok(WorktreeMetadataLookup::Present)
        )
    {
        return Err(SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        });
    }
    Ok(WorktreeChangeInventory {
        worktree_id: row.id,
        clean: files.is_empty(),
        files,
    })
}

fn same_b1_project(left: &Project, right: &Project) -> bool {
    left.id == right.id
        && left.repository_identity == right.repository_identity
        && left.repository_fingerprint == right.repository_fingerprint
        && left.fingerprint_scheme == right.fingerprint_scheme
        && left.repository_root == right.repository_root
        && left.primary_root == right.primary_root
        && left.git_common_dir == right.git_common_dir
        && left.validation_state == right.validation_state
        && left.is_primary_worktree == right.is_primary_worktree
}

fn same_b1_worktree(left: &ManagedWorktree, right: &ManagedWorktree) -> bool {
    left.id == right.id
        && left.project_id == right.project_id
        && left.path == right.path
        && left.base_commit == right.base_commit
        && left.repository_identity == right.repository_identity
        && left.repository_fingerprint == right.repository_fingerprint
}

#[cfg(test)]
fn b1_test_lifecycle(state: ManagedWorktreeState) -> B1TestLifecycle {
    match state {
        ManagedWorktreeState::Ready | ManagedWorktreeState::RetainedDirty => {
            B1TestLifecycle::Eligible
        }
        ManagedWorktreeState::Removing => B1TestLifecycle::Removing,
        _ => B1TestLifecycle::Other,
    }
}

async fn refresh_b1_acceptance_state(
    state: &DesktopState,
    worktree_id: &WorktreeId,
    project_id: &ProjectId,
    expected_project: &Project,
    expected_worktree: &ManagedWorktree,
) -> Result<(Project, ManagedWorktree), SafeError> {
    let project = strict_project_for_worktree(state, project_id).await?;
    let row = state
        .repository
        .get_worktree(worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    #[cfg(test)]
    {
        let accepted = same_b1_project(&project, expected_project)
            && same_b1_worktree(&row, expected_worktree)
            && row.project_id == *project_id
            && inventory_eligible(row.state);
        observe_b1(
            state,
            B1TestEvent::FinalPersistedStateReloadCompleted {
                lifecycle: b1_test_lifecycle(row.state),
                accepted,
            },
        );
    }
    if !inventory_eligible(row.state) || row.project_id != *project_id {
        return Err(SafeError {
            code: "worktree_not_ready",
            message: "This worktree is not available for inspection.",
        });
    }
    if !same_b1_project(&project, expected_project) {
        return Err(SafeError {
            code: "project_identity_changed",
            message: "The registered repository was replaced or changed identity.",
        });
    }
    if !same_b1_worktree(&row, expected_worktree) {
        return Err(SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        });
    }
    Ok((project, row))
}

async fn validate_b1_refreshed_repository_state(
    state: &DesktopState,
    project: &Project,
    row: &ManagedWorktree,
) -> Result<(), SafeError> {
    let stored = PathBuf::from(&row.path);
    let leaf = validate_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &stored)
        .map_err(|_| SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        })?;
    let inspection = inspect_repository(&leaf).await.map_err(inventory_error)?;
    if inspection.is_primary
        || inspection.identity != row.repository_identity
        || inspection.fingerprint.as_str() != row.repository_fingerprint
        || inspection.head.as_deref() != Some(row.base_commit.as_str())
        || !matches!(
            worktree_metadata_lookup(Path::new(&project.repository_root), &leaf).await,
            Ok(WorktreeMetadataLookup::Present)
        )
    {
        return Err(SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        });
    }
    Ok(())
}

fn change_not_found_error() -> SafeError {
    SafeError {
        code: "change_not_found",
        message: "The selected change is no longer available.",
    }
}

fn change_stale_error() -> SafeError {
    SafeError {
        code: "change_stale",
        message: "The selected change changed during inspection.",
    }
}

fn section_modes(
    file: &ChangedFile,
    staged: bool,
) -> (Option<RepositoryMode>, Option<RepositoryMode>) {
    if staged {
        (file.mode_head, file.mode_index)
    } else {
        (file.mode_index, file.mode_worktree)
    }
}

fn section_has_change(file: &ChangedFile, staged: bool) -> bool {
    if staged {
        file.index_change.is_some()
    } else {
        file.worktree_change.is_some()
    }
}

fn is_regular_or_absent(mode: Option<RepositoryMode>) -> bool {
    matches!(
        mode,
        None | Some(RepositoryMode::Absent | RepositoryMode::Regular | RepositoryMode::Executable)
    )
}

fn classify_without_numstat(file: &ChangedFile, staged: bool) -> Option<DiffSectionClassification> {
    if file.untracked {
        return Some(if staged {
            DiffSectionClassification::NotApplicable
        } else {
            DiffSectionClassification::UntrackedContentDeferred
        });
    }
    if file.conflict.is_some() {
        return Some(DiffSectionClassification::ConflictContentDeferred);
    }
    if !section_has_change(file, staged) {
        return Some(DiffSectionClassification::NotApplicable);
    }
    let (before, after) = section_modes(file, staged);
    if file.submodule.is_some()
        || matches!(before, Some(RepositoryMode::Gitlink))
        || matches!(after, Some(RepositoryMode::Gitlink))
    {
        return Some(DiffSectionClassification::SubmoduleMetadataOnly);
    }
    if matches!(before, Some(RepositoryMode::SymbolicLink))
        || matches!(after, Some(RepositoryMode::SymbolicLink))
    {
        return Some(DiffSectionClassification::SymlinkMetadataOnly);
    }
    if !is_regular_or_absent(before) || !is_regular_or_absent(after) {
        return Some(DiffSectionClassification::UnsupportedType);
    }
    None
}

async fn validated_numstat_context(
    state: &DesktopState,
    worktree_id: &WorktreeId,
    project_id: &ProjectId,
) -> Result<(ManagedWorktree, PathBuf), SafeError> {
    let row = state
        .repository
        .get_worktree(worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    if row.project_id != *project_id || !inventory_eligible(row.state) {
        return Err(SafeError {
            code: "worktree_not_ready",
            message: "This worktree is not available for inspection.",
        });
    }
    let project = strict_project_for_worktree(state, project_id).await?;
    let stored = PathBuf::from(&row.path);
    let leaf = validate_managed_leaf(&state.worktree_root, project_id, &row.id, &stored).map_err(
        |_| SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        },
    )?;
    let inspection = inspect_repository(&leaf).await.map_err(inventory_error)?;
    if inspection.is_primary
        || inspection.identity != row.repository_identity
        || inspection.fingerprint.as_str() != row.repository_fingerprint
        || inspection.head.as_deref() != Some(row.base_commit.as_str())
        || !matches!(
            worktree_metadata_lookup(Path::new(&project.repository_root), &leaf).await,
            Ok(WorktreeMetadataLookup::Present)
        )
    {
        return Err(SafeError {
            code: "ownership_validation_failed",
            message: "The managed worktree could not be verified.",
        });
    }
    Ok((row, leaf))
}

async fn classify_section_locked(
    state: &DesktopState,
    worktree_id: &WorktreeId,
    project_id: &ProjectId,
    expected_file: &ChangedFile,
    staged: bool,
) -> Result<DiffSectionClassification, SafeError> {
    if let Some(classification) = classify_without_numstat(expected_file, staged) {
        return Ok(classification);
    }
    // Re-run the complete Phase 3C-A orchestration before each path-following
    // command, then use only its freshly generated path entry.
    let fresh = inspect_worktree_changes_locked(
        state,
        worktree_id,
        project_id,
        #[cfg(test)]
        None,
    )
    .await?;
    let fresh_file = fresh
        .files
        .iter()
        .find(|file| file.path == expected_file.path)
        .ok_or_else(change_stale_error)?;
    if fresh_file != expected_file {
        return Err(change_stale_error());
    }
    let (_row, leaf) = validated_numstat_context(state, worktree_id, project_id).await?;
    let filter_state = if staged {
        None
    } else {
        let attribute_state = inspect_worktree_filter_attribute(&leaf, &fresh_file.path)
            .await
            .map_err(inventory_error)?;
        if attribute_state == FilterAttributeState::Deferred {
            return Ok(DiffSectionClassification::ExternalFilterDeferred);
        }
        Some(attribute_state)
    };
    // Repeat exact managed-leaf/identity validation immediately before the
    // path-following numstat command after attribute inspection.
    let (row, leaf) = validated_numstat_context(state, worktree_id, project_id).await?;
    let result = inspect_worktree_numstat(
        &leaf,
        &fresh_file.path,
        if staged {
            Some(row.base_commit.as_str())
        } else {
            None
        },
    )
    .await
    .map_err(inventory_error)?;
    if let Some(filter_state) = filter_state {
        let after_filter = inspect_worktree_filter_attribute(&leaf, &fresh_file.path)
            .await
            .map_err(inventory_error)?;
        if after_filter != filter_state {
            return Err(change_stale_error());
        }
    }
    match result {
        NumstatClassification::TextEligible(metadata) => {
            let (before, after) = section_modes(fresh_file, staged);
            if metadata.additions == 0
                && metadata.deletions == 0
                && matches!(
                    before,
                    Some(RepositoryMode::Regular | RepositoryMode::Executable)
                )
                && matches!(
                    after,
                    Some(RepositoryMode::Regular | RepositoryMode::Executable)
                )
                && before != after
            {
                Ok(DiffSectionClassification::ModeOnly)
            } else {
                Ok(DiffSectionClassification::TextEligible(metadata))
            }
        }
        NumstatClassification::Binary => Ok(DiffSectionClassification::Binary),
        NumstatClassification::NoChanges => {
            let (before, after) = section_modes(fresh_file, staged);
            if matches!(
                before,
                Some(RepositoryMode::Regular | RepositoryMode::Executable)
            ) && matches!(
                after,
                Some(RepositoryMode::Regular | RepositoryMode::Executable)
            ) && before != after
            {
                Ok(DiffSectionClassification::ModeOnly)
            } else {
                Err(change_stale_error())
            }
        }
    }
}

/// Internal Phase 3C-B1 orchestration. The candidate is typed but gains
/// authority only by exact matching against a fresh locked Phase 3C-A result.
#[allow(dead_code)]
async fn inspect_worktree_file_diff_classification_impl(
    state: &DesktopState,
    worktree_id: WorktreeId,
    selected_path: RepositoryRelativePath,
) -> Result<FileDiffClassification, SafeError> {
    let initial = state
        .repository
        .get_worktree(&worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    let project_id = initial.project_id.clone();
    let initial_project = state
        .repository
        .get_project(&project_id)
        .await
        .map_err(project_storage_error)?;
    acquire_project_worktree_lock(state, &project_id).await?;
    let result = match with_inventory_operation_context(async {
        let inventory_a = inspect_worktree_changes_locked(
            state,
            &worktree_id,
            &project_id,
            #[cfg(test)]
            Some(B1TestInventoryStage::A),
        )
        .await?;
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::InventoryCompleted {
                stage: B1TestInventoryStage::A,
                observation: b1_test_inventory_observation(&inventory_a),
            },
        );
        let evidence_one = collect_b1_classification_evidence(
            state,
            &worktree_id,
            &project_id,
            &selected_path,
            &inventory_a,
            #[cfg(test)]
            B1TestPass::C1,
        )
        .await?;
        #[cfg(test)]
        pause_after_b1_numstat(state).await;
        let inventory_b = inspect_worktree_changes_locked(
            state,
            &worktree_id,
            &project_id,
            #[cfg(test)]
            Some(B1TestInventoryStage::B),
        )
        .await?;
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::InventoryCompleted {
                stage: B1TestInventoryStage::B,
                observation: b1_test_inventory_observation(&inventory_b),
            },
        );
        if inventory_a.files != inventory_b.files {
            return Err(change_stale_error());
        }
        let evidence_two = collect_b1_classification_evidence(
            state,
            &worktree_id,
            &project_id,
            &selected_path,
            &inventory_b,
            #[cfg(test)]
            B1TestPass::C2,
        )
        .await?;
        if evidence_one != evidence_two {
            #[cfg(test)]
            observe_b1(
                state,
                B1TestEvent::EvidenceComparisonCompleted { equal: false },
            );
            return Err(change_stale_error());
        }
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::EvidenceComparisonCompleted { equal: true },
        );
        #[cfg(test)]
        pause_after_b1_c2_equality(state).await;
        let inventory_c = inspect_worktree_changes_locked(
            state,
            &worktree_id,
            &project_id,
            #[cfg(test)]
            Some(B1TestInventoryStage::C),
        )
        .await?;
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::InventoryCompleted {
                stage: B1TestInventoryStage::C,
                observation: b1_test_inventory_observation(&inventory_c),
            },
        );
        if inventory_a.files != inventory_c.files || inventory_b.files != inventory_c.files {
            return Err(change_stale_error());
        }
        let evidence_three = collect_b1_classification_evidence(
            state,
            &worktree_id,
            &project_id,
            &selected_path,
            &inventory_c,
            #[cfg(test)]
            B1TestPass::C3,
        )
        .await?;
        if evidence_two != evidence_three {
            #[cfg(test)]
            observe_b1(
                state,
                B1TestEvent::FinalEvidenceComparisonCompleted { equal: false },
            );
            return Err(change_stale_error());
        }
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::FinalEvidenceComparisonCompleted { equal: true },
        );
        #[cfg(test)]
        observe_b1(state, B1TestEvent::FinalPersistedStateReloadStarted);
        #[cfg(test)]
        pause_before_b1_final_persisted_reload(state).await;
        let (project, row) = refresh_b1_acceptance_state(
            state,
            &worktree_id,
            &project_id,
            &initial_project,
            &initial,
        )
        .await?;
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::FinalValidationStarted {
                stage: B1TestInventoryStage::C,
            },
        );
        #[cfg(test)]
        pause_before_b1_final_validation(state).await;
        validate_b1_refreshed_repository_state(state, &project, &row).await?;
        #[cfg(test)]
        observe_b1(
            state,
            B1TestEvent::FinalValidationCompleted {
                stage: B1TestInventoryStage::C,
            },
        );
        Ok(evidence_three)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(inventory_error(error)),
    };
    release_project_worktree_lock(state, &project_id).await;
    result
}

async fn collect_b1_classification_evidence(
    state: &DesktopState,
    worktree_id: &WorktreeId,
    project_id: &ProjectId,
    selected_path: &RepositoryRelativePath,
    inventory: &WorktreeChangeInventory,
    #[cfg(test)] pass: B1TestPass,
) -> Result<FileDiffClassification, SafeError> {
    #[cfg(test)]
    observe_b1(state, B1TestEvent::ClassificationPassStarted { pass });
    let file = inventory
        .files
        .iter()
        .find(|file| file.path == *selected_path)
        .cloned()
        .ok_or_else(change_not_found_error)?;
    let staged = classify_section_locked(state, worktree_id, project_id, &file, true).await?;
    #[cfg(test)]
    observe_b1_numstat(state, pass, B1TestSurface::Staged, &staged);
    let unstaged = classify_section_locked(state, worktree_id, project_id, &file, false).await?;
    #[cfg(test)]
    observe_b1_numstat(state, pass, B1TestSurface::Unstaged, &unstaged);
    let evidence = FileDiffClassification {
        worktree_id: worktree_id.clone(),
        path: file.path,
        staged,
        unstaged,
        mode_head: file.mode_head,
        mode_index: file.mode_index,
        mode_worktree: file.mode_worktree,
    };
    #[cfg(test)]
    observe_b1(
        state,
        B1TestEvent::ClassificationPassCompleted {
            pass,
            observation: b1_test_evidence_observation(&evidence),
        },
    );
    Ok(evidence)
}
#[tauri::command]
async fn create_project_worktree(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<WorktreeDto, SafeError> {
    let project_id = ProjectId::from_str(&id).map_err(|_| input_error())?;
    acquire_project_worktree_lock(&state, &project_id).await?;
    let result = async {
        let project = strict_project_for_worktree(&state, &project_id).await?;
        let base_commit = resolve_exact_head(Path::new(&project.repository_root))
            .await
            .map_err(|error| project_error(&error))?;
        let worktree_id = WorktreeId::new();
        let parent = worktree_parent(&state.worktree_root, &project_id)?;
        let path = parent.join(worktree_id.to_string());
        if inspect_worktree_destination_no_follow(&path) != WorktreeDestinationState::Missing {
            return Err(worktree_error("preexisting leaf"));
        }
        let row = state
            .repository
            .insert_creating_worktree(
                worktree_id,
                &project,
                path.to_string_lossy().into_owned(),
                base_commit.clone(),
            )
            .await
            .map_err(worktree_error)?;
        if let Err(error) =
            add_detached_worktree(Path::new(&project.repository_root), &path, &base_commit).await
        {
            let _ = state
                .repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Failed,
                    Some("worktree_create_failed"),
                )
                .await;
            return Err(project_error(&error));
        }
        let verified_parent = worktree_parent(&state.worktree_root, &project_id)?;
        let leaf = validate_managed_leaf(&state.worktree_root, &project_id, &row.id, &path);
        if verified_parent != parent
            || !matches!(leaf, Ok(ref canonical) if canonical == &path)
            || !matches!(
                worktree_metadata_lookup(Path::new(&project.repository_root), &path)
                    .await
                    .map_err(|error| project_error(&error))?,
                WorktreeMetadataLookup::Present
            )
        {
            let _ = state
                .repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Failed,
                    Some("worktree_verification_failed"),
                )
                .await;
            return Err(SafeError {
                code: "worktree_verification_failed",
                message: "The managed worktree could not be verified.",
            });
        }
        let leaf = validate_managed_leaf(&state.worktree_root, &project_id, &row.id, &path)?;
        let inspection = inspect_repository(&leaf)
            .await
            .map_err(|error| project_error(&error))?;
        if leaf != path
            || inspection.is_primary
            || inspection.identity != project.repository_identity
            || inspection.fingerprint.as_str() != project.repository_fingerprint
            || inspection.head.as_deref() != Some(base_commit.as_str())
        {
            let _ = state
                .repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Failed,
                    Some("worktree_verification_failed"),
                )
                .await;
            return Err(SafeError {
                code: "worktree_verification_failed",
                message: "The managed worktree could not be verified.",
            });
        }
        state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Creating,
                ManagedWorktreeState::Ready,
                None,
            )
            .await
            .map(worktree_dto)
            .map_err(worktree_error)
    }
    .await;
    release_project_worktree_lock(&state, &project_id).await;
    result
}
#[tauri::command]
async fn list_project_worktrees(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<Vec<WorktreeDto>, SafeError> {
    let id = ProjectId::from_str(&id).map_err(|_| input_error())?;
    state
        .repository
        .list_project_worktrees(&id)
        .await
        .map(|rows| rows.into_iter().map(worktree_dto).collect())
        .map_err(worktree_error)
}
#[tauri::command]
async fn remove_project_worktree(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<WorktreeDto, SafeError> {
    let id = WorktreeId::from_str(&id).map_err(|_| input_error())?;
    let row = state
        .repository
        .get_worktree(&id)
        .await
        .map_err(worktree_error)?;
    acquire_project_worktree_lock(&state, &row.project_id).await?;
    let project_id = row.project_id.clone();
    let result = async {
        if row.state != ManagedWorktreeState::Ready {
            return Err(SafeError {
                code: "worktree_not_owned",
                message: "This worktree is not available for managed removal.",
            });
        }
        let project = strict_project_for_worktree(&state, &row.project_id).await?;
        let path = PathBuf::from(&row.path);
        let leaf = validate_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path)
            .map_err(|_| SafeError {
                code: "worktree_not_owned",
                message: "This worktree is not available for managed removal.",
            })?;
        let inspection = inspect_repository(&leaf)
            .await
            .map_err(|error| project_error(&error))?;
        let leaf = validate_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path)
            .map_err(|_| SafeError {
                code: "worktree_not_owned",
                message: "This worktree is not available for managed removal.",
            })?;
        if inspection.is_primary
            || inspection.identity != row.repository_identity
            || inspection.fingerprint.as_str() != row.repository_fingerprint
            || inspection.head.as_deref() != Some(row.base_commit.as_str())
            || !matches!(
                worktree_metadata_lookup(Path::new(&project.repository_root), &leaf)
                    .await
                    .map_err(|error| project_error(&error))?,
                WorktreeMetadataLookup::Present
            )
        {
            return Err(SafeError {
                code: "worktree_not_owned",
                message: "This worktree is not available for managed removal.",
            });
        }
        let _ = state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Ready,
                ManagedWorktreeState::Removing,
                None,
            )
            .await
            .map_err(worktree_error)?;
        // AH2 restores a safe inventory only. Managed-worktree deletion stays
        // deferred until the holistic Phase 3C-A review approves the complete
        // boundary, so no successful clean result can trigger removal here.
        return state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Removing,
                ManagedWorktreeState::RetainedDirty,
                Some("worktree_removal_deferred"),
            )
            .await
            .map(worktree_dto)
            .map_err(worktree_error);
    }
    .await;
    release_project_worktree_lock(&state, &project_id).await;
    result
}
async fn reconcile_project_worktrees_impl(
    state: &DesktopState,
) -> Result<Vec<WorktreeDto>, SafeError> {
    let rows = state
        .repository
        .list_reconcilable_worktrees()
        .await
        .map_err(worktree_error)?;
    for row in rows {
        if acquire_project_worktree_lock(state, &row.project_id)
            .await
            .is_err()
        {
            continue;
        }
        let path = PathBuf::from(&row.path);
        let project = match state.repository.get_project(&row.project_id).await {
            Ok(project) => project,
            Err(_) => {
                let _ = state
                    .repository
                    .transition_worktree(
                        &row.id,
                        row.state,
                        ManagedWorktreeState::Failed,
                        Some("recovery_required"),
                    )
                    .await;
                release_project_worktree_lock(state, &row.project_id).await;
                continue;
            }
        };
        // Metadata lookup may use a missing backend-derived spelling only
        // after this no-follow state proves `NotFound`. Unsafe or unavailable
        // leaves never become missing or removed.
        let leaf_state =
            inspect_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path);
        let next = match leaf_state {
            Ok(ManagedLeafState::Missing) => {
                let metadata =
                    worktree_metadata_lookup(Path::new(&project.repository_root), &path).await;
                if row.state == ManagedWorktreeState::Removing
                    && matches!(metadata, Ok(WorktreeMetadataLookup::Absent))
                {
                    Some(ManagedWorktreeState::Removed)
                } else if row.state == ManagedWorktreeState::Creating {
                    Some(ManagedWorktreeState::Failed)
                } else {
                    Some(ManagedWorktreeState::Missing)
                }
            }
            Ok(ManagedLeafState::RealDirectory)
                if matches!(
                    row.state,
                    ManagedWorktreeState::Creating | ManagedWorktreeState::Ready
                ) =>
            {
                match validate_managed_leaf_for_reconciliation(
                    state,
                    &row.project_id,
                    &row.id,
                    &path,
                ) {
                    Ok(leaf) => {
                        match worktree_metadata_lookup(Path::new(&project.repository_root), &leaf)
                            .await
                        {
                            Ok(WorktreeMetadataLookup::Present) => {
                                match inspect_repository(&leaf).await {
                                    Ok(inspection)
                                        if !inspection.is_primary
                                            && inspection.identity == row.repository_identity
                                            && inspection.fingerprint.as_str()
                                                == row.repository_fingerprint
                                            && inspection.head.as_deref()
                                                == Some(row.base_commit.as_str()) =>
                                    {
                                        if row.state == ManagedWorktreeState::Creating {
                                            Some(ManagedWorktreeState::Ready)
                                        } else {
                                            None
                                        }
                                    }
                                    Ok(_) => Some(ManagedWorktreeState::IdentityChanged),
                                    Err(_) => Some(ManagedWorktreeState::Failed),
                                }
                            }
                            _ => Some(ManagedWorktreeState::Failed),
                        }
                    }
                    Err(_) => {
                        #[cfg(test)]
                        pause_before_reconciliation_failure_transition(state).await;
                        reconciliation_validation_failure_state(row.state)
                    }
                }
            }
            Ok(ManagedLeafState::RealDirectory) if row.state == ManagedWorktreeState::Removing => {
                Some(ManagedWorktreeState::Failed)
            }
            Ok(ManagedLeafState::RealDirectory) => None,
            Ok(ManagedLeafState::UnsafeLinkOrReparse)
            | Ok(ManagedLeafState::NonDirectory)
            | Ok(ManagedLeafState::MetadataUnavailable)
            | Err(_) => Some(ManagedWorktreeState::Failed),
        };
        if let Some(next) = next {
            let _ = state
                .repository
                .transition_worktree(&row.id, row.state, next, Some("recovery_required"))
                .await;
        }
        release_project_worktree_lock(state, &row.project_id).await;
    }
    state
        .repository
        .list_reconcilable_worktrees()
        .await
        .map(|rows| rows.into_iter().map(worktree_dto).collect())
        .map_err(worktree_error)
}

#[tauri::command]
async fn reconcile_project_worktrees(
    state: State<'_, DesktopState>,
) -> Result<Vec<WorktreeDto>, SafeError> {
    reconcile_project_worktrees_impl(&state).await
}
#[tauri::command]
async fn submit_fake_run(
    request: SubmitFakeRun,
    state: State<'_, DesktopState>,
    app: AppHandle,
) -> Result<RunDto, SafeError> {
    let scenario = FakeAgentScenario::from_str(&request.scenario).map_err(|_| input_error())?;
    let run = state
        .orchestrator
        .submit_run(
            TaskRequest {
                task_text: request.task_text,
            },
            scenario,
        )
        .await
        .map_err(safe_error)?;
    let registered = { state.forwarders.lock().await.insert(run.id.clone()) };
    if !registered {
        return Ok(run_dto(&state, run).await);
    }
    let mut receiver = match state.orchestrator.subscribe_to_run_events(&run.id).await {
        Ok(value) => value,
        Err(_) => {
            // A fast run may already have completed. Replay its persisted history
            // asynchronously; submission itself remains successful.
            let forwarders = state.forwarders.clone();
            let repository = state.repository.clone();
            let run_id = run.id.clone();
            let replay_app = app.clone();
            tauri::async_runtime::spawn(async move {
                let mut last_sequence = 0;
                let _ =
                    replay_persisted_events(&repository, &run_id, &replay_app, &mut last_sequence)
                        .await;
                forwarders.lock().await.remove(&run_id);
            });
            return Ok(run_dto(&state, run).await);
        }
    };
    let forwarders = state.forwarders.clone();
    let run_id = run.id.clone();
    let repository = state.repository.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_sequence = 0;
        // Subscribe first: events produced during this persisted replay remain
        // buffered by Tokio and are deduplicated below by sequence number.
        match replay_persisted_events(&repository, &run_id, &app, &mut last_sequence).await {
            Ok(true) | Err(()) => {
                forwarders.lock().await.remove(&run_id);
                return;
            }
            Ok(false) => {}
        }
        loop {
            let event = match receiver.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match replay_persisted_events(&repository, &run_id, &app, &mut last_sequence)
                        .await
                    {
                        Ok(true) | Err(()) => break,
                        Ok(false) => {}
                    }
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let _ = replay_persisted_events(&repository, &run_id, &app, &mut last_sequence)
                        .await;
                    break;
                }
            };
            if event.sequence_number <= last_sequence {
                continue;
            }
            let terminal = terminal_event(&event.event_type);
            last_sequence = event.sequence_number;
            let dto = event_dto(event);
            let _ = app.emit(RUN_EVENT, dto);
            if terminal {
                break;
            }
        }
        forwarders.lock().await.remove(&run_id);
    });
    Ok(run_dto(&state, run).await)
}
async fn capture_b2_diff_pair(
    row: &ManagedWorktree,
    path: &RepositoryRelativePath,
) -> Result<(Vec<u8>, Vec<u8>), SafeError> {
    let staged = extract_textual_diff(Path::new(&row.path), path, Some(&row.base_commit))
        .await
        .map_err(|_| SafeError {
            code: "diff_unavailable",
            message: "The textual change is no longer available.",
        })?;
    let unstaged = extract_textual_diff(Path::new(&row.path), path, None)
        .await
        .map_err(|_| SafeError {
            code: "diff_unavailable",
            message: "The textual change is no longer available.",
        })?;
    Ok((staged, unstaged))
}

/// Phase 3C-C bridge over the private B2 extraction pipeline. The caller's
/// path is only a lexical candidate; B1 exact inventory/classification and
/// the persisted ProjectId/WorktreeId binding remain the authority.
#[tauri::command]
async fn inspect_textual_diff(
    request: InspectTextualDiffRequest,
    state: State<'_, DesktopState>,
) -> Result<TextualDiffDto, SafeError> {
    let project_id = ProjectId::from_str(&request.project_id).map_err(|_| input_error())?;
    let worktree_id = WorktreeId::from_str(&request.worktree_id).map_err(|_| input_error())?;
    let selected_path =
        RepositoryRelativePath::from_selection(&request.path).map_err(|_| input_error())?;
    let row = state
        .repository
        .get_worktree(&worktree_id)
        .await
        .map_err(|_| SafeError {
            code: "worktree_not_found",
            message: "The managed worktree was not found.",
        })?;
    if row.project_id != project_id {
        return Err(input_error());
    }
    let classification = inspect_worktree_file_diff_classification_impl(
        &state,
        worktree_id.clone(),
        selected_path.clone(),
    )
    .await?;
    let textual = matches!(
        classification.staged,
        DiffSectionClassification::TextEligible(_)
    ) || matches!(
        classification.unstaged,
        DiffSectionClassification::TextEligible(_)
    );
    if !textual {
        return Ok(TextualDiffDto {
            project_id: project_id.to_string(),
            worktree_id: worktree_id.to_string(),
            textual_available: false,
            staged_additions: 0,
            staged_deletions: 0,
            unstaged_additions: 0,
            unstaged_deletions: 0,
            hunk_count: 0,
        });
    }
    // E1/E2/E3 are independently captured after B1's matching A/C1/B/C2/C/C3
    // authority. Raw byte equality prevents accepting a transient patch.
    let evidence_one = capture_b2_diff_pair(&row, &selected_path).await?;
    let evidence_two = capture_b2_diff_pair(&row, &selected_path).await?;
    let evidence_three = capture_b2_diff_pair(&row, &selected_path).await?;
    if evidence_one != evidence_two || evidence_two != evidence_three {
        return Err(SafeError {
            code: "change_stale",
            message: "The change changed while it was being inspected.",
        });
    }
    let summary = b2_a::parse_private_diff_pair(
        selected_path.as_str().to_owned(),
        &evidence_three.0,
        &evidence_three.1,
    )
    .map_err(|error| SafeError {
        code: error.safe_code(),
        message: "The textual change could not be safely inspected.",
    })?;
    Ok(TextualDiffDto {
        project_id: project_id.to_string(),
        worktree_id: worktree_id.to_string(),
        textual_available: summary.textual_available,
        staged_additions: summary.staged_additions,
        staged_deletions: summary.staged_deletions,
        unstaged_additions: summary.unstaged_additions,
        unstaged_deletions: summary.unstaged_deletions,
        hunk_count: summary.hunk_count,
    })
}

/// Deprecated Phase 0 command compatibility; delegates to the trusted runtime only.
#[tauri::command]
async fn run_fake_agent(state: State<'_, DesktopState>, app: AppHandle) -> Result<(), SafeError> {
    let run = state
        .orchestrator
        .submit_run(
            TaskRequest {
                task_text: "Phase 0 compatibility fake run".into(),
            },
            FakeAgentScenario::Success,
        )
        .await
        .map_err(safe_error)?;
    let orchestrator = state.orchestrator.clone();
    tauri::async_runtime::spawn(async move {
        let event = match orchestrator.wait_for_run(&run.id).await {
            Ok(run) if run.status == sentinel_core::RunStatus::Completed => {
                serde_json::json!({"type":"completed","text":"Fake run completed."})
            }
            _ => {
                serde_json::json!({"type":"failed","text":"Fake run failed.","error":"Fake run failed."})
            }
        };
        let _ = app.emit("agent-event", event);
    });
    Ok(())
}
/// Deprecated Phase 0 command compatibility; never starts a provider request.
#[tauri::command]
fn record_manual_probe_request(app: AppHandle, agent: String) {
    let _ = app.emit("agent-event", serde_json::json!({"type":"message","text":format!("{agent} probe is manual-only; no model request was started.")}));
}
#[tauri::command]
async fn cancel_run(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<CancellationResultDto, SafeError> {
    let id = RunId::from_str(&id).map_err(|_| input_error())?;
    Ok(CancellationResultDto::from(
        state.orchestrator.cancel_run(&id).await,
    ))
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CancellationResultDto {
    CancellationRequested,
    AlreadyTerminal,
    AlreadyCancelling,
    RunNotActive,
    TerminationFailed,
}
impl From<CancellationResult> for CancellationResultDto {
    fn from(value: CancellationResult) -> Self {
        match value {
            CancellationResult::CancellationRequested => Self::CancellationRequested,
            CancellationResult::AlreadyTerminal => Self::AlreadyTerminal,
            CancellationResult::AlreadyCancelling => Self::AlreadyCancelling,
            CancellationResult::RunNotActive => Self::RunNotActive,
            CancellationResult::TerminationFailed => Self::TerminationFailed,
        }
    }
}

fn format_installation(status: InstallationStatus) -> String {
    match status {
        InstallationStatus::Available { version, .. } => format!("available: {version}"),
        InstallationStatus::NotInstalled => "not_installed".into(),
        InstallationStatus::Unusable { detail } => format!("unusable: {detail}"),
    }
}

fn show_prompt(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window(PROMPT_WINDOW_LABEL)
        .ok_or_else(|| tauri::Error::AssetNotFound(PROMPT_WINDOW_LABEL.into()))?;
    window.show()?;
    window.unminimize()?;
    // Tauri can center the prompt on its current display, but cannot reliably discover the
    // display of another application's focused window on every platform.
    window.center()?;
    window.set_focus()?;
    app.emit("focus-task-input", ())?;
    Ok(())
}

fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    eprintln!("agent-sentinel: tray setup started");
    let icon = match Image::from_bytes(include_bytes!("../icons/tray-template.png")) {
        Ok(icon) => {
            eprintln!("agent-sentinel: tray icon loaded");
            icon
        }
        Err(error) => {
            eprintln!("agent-sentinel: tray icon load failed: {error:?}");
            return Err(error);
        }
    };
    let open = MenuItem::with_id(app, "open-prompt", "Open Prompt", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "show-status", "Show Spike Status", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &status, &quit])?;
    let builder = TrayIconBuilder::with_id("spike-tray")
        .icon(icon)
        .menu(&menu)
        .tooltip("Agent Sentinel")
        .on_menu_event(|app, event| match tray_action(event.id.as_ref()) {
            TrayAction::OpenPrompt | TrayAction::ShowSpikeStatus => {
                let _ = show_prompt(app);
            }
            TrayAction::Quit => app.exit(0),
            TrayAction::Ignore => {}
        });
    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);
    let tray = match builder.build(app) {
        Ok(tray) => {
            eprintln!("agent-sentinel: tray successfully built");
            tray
        }
        Err(error) => {
            eprintln!("agent-sentinel: tray build failed: {error:?}");
            return Err(error);
        }
    };
    app.manage(TrayState(tray));
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(MACOS_ACTIVATION_POLICY);
                app.set_dock_visibility(false);
            }
            if let Err(error) = install_tray(app.handle()) {
                eprintln!("agent-sentinel: tray setup failed: {error:?}");
                return Err(Box::new(error));
            }
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            std::fs::create_dir_all(&data_dir)?;
            let database_path = data_dir.join("phase2.sqlite3");
            let worktree_root = data_dir.join("worktrees");
            let executable = resolve_fake_agent_path(
                std::env::var_os("AGENT_SENTINEL_FAKE_AGENT").map(PathBuf::from),
                tauri::process::current_binary(&app.env())?,
            )
            .map_err(|_| {
                Box::<dyn std::error::Error>::from(
                    "Agent Sentinel fake-agent executable is unavailable",
                )
            })?;
            let repository = tauri::async_runtime::block_on(RunRepository::open(&format!(
                "sqlite:{}",
                database_path.display()
            )))
            .map_err(|_| {
                Box::<dyn std::error::Error>::from(
                    "Agent Sentinel could not initialize its local database",
                )
            })?;
            // A prior desktop process owns no reconnectable child handles.
            // Fail closed before exposing public run queries.
            tauri::async_runtime::block_on(repository.reconcile_codex_run_contexts_on_startup())
                .map_err(|_| {
                    Box::<dyn std::error::Error>::from(
                        "Agent Sentinel could not reconcile interrupted Codex runs",
                    )
                })?;
            // Approval decisions are control-plane records in Phase 7. There
            // is no process waiter to reconstruct, so adapter-bound pending
            // rows become unavailable rather than implying delivery.
            tauri::async_runtime::block_on(repository.reconcile_approval_requests_on_startup())
                .map_err(|_| {
                    Box::<dyn std::error::Error>::from(
                        "Agent Sentinel could not reconcile approval records",
                    )
                })?;
            let protected_application_repository = application_repository_root()
                .map(|root| {
                    tauri::async_runtime::block_on(inspect_repository(&root)).map(|inspection| {
                        ProtectedRepository {
                            identity: inspection.identity,
                            fingerprint: inspection.fingerprint.as_str().to_owned(),
                        }
                    })
                })
                .transpose()
                .map_err(|_| {
                    Box::<dyn std::error::Error>::from(
                        "Agent Sentinel could not validate its protected repository",
                    )
                })?;
            let fake_agent = FakeAgentProgram::from_executable(executable).map_err(|_| {
                Box::<dyn std::error::Error>::from(
                    "Agent Sentinel fake-agent executable is unavailable",
                )
            })?;
            // This private opt-in is intentionally read only at startup.  No
            // public command accepts an executable path; tests may point it at
            // a deterministic fixture executable.
            let codex = std::env::var_os("AGENT_SENTINEL_CODEX_EXECUTABLE")
                .map(PathBuf::from)
                .and_then(|path| CodexExecProgram::from_executable(path).ok());
            let codex_exec_available = codex.is_some();
            app.manage(DesktopState {
                orchestrator: match codex {
                    Some(program) => RunOrchestrator::new(repository.clone(), fake_agent)
                        .with_codex_exec(program),
                    None => RunOrchestrator::new(repository.clone(), fake_agent),
                },
                codex_exec_available,
                repository,
                database_path,
                forwarders: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
                protected_application_repository,
                worktree_root,
                worktree_projects: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
                #[cfg(test)]
                reconciliation_test_hooks: None,
                #[cfg(test)]
                b1_test_hooks: None,
            });
            app.global_shortcut()
                .on_shortcut(DEFAULT_GLOBAL_SHORTCUT, |app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        let _ = show_prompt(app);
                    }
                })?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if should_hide_on_close(window.label()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            spike_diagnostics,
            codex_capability,
            approval_capability,
            list_pending_approvals,
            query_approval_request,
            decide_approval_request,
            start_codex_run,
            query_codex_run,
            cancel_codex_run,
            start_claude_run,
            query_claude_run,
            followup_claude_run,
            cancel_claude_run,
            submit_fake_run,
            list_runs,
            get_run,
            list_run_events,
            cancel_run,
            get_runtime_environment,
            desktop_capabilities,
            register_project,
            list_projects,
            get_project,
            revalidate_project,
            unregister_project,
            create_project_worktree,
            list_project_worktrees,
            remove_project_worktree,
            reconcile_project_worktrees,
            inspect_textual_diff,
            run_fake_agent,
            record_manual_probe_request
        ])
        .run(tauri::generate_context!())
        .expect("Tauri spike failed to run");
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn startup_uses_the_menu_bar_accessory_policy() {
        assert!(matches!(
            MACOS_ACTIVATION_POLICY,
            tauri::ActivationPolicy::Accessory
        ));
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use sentinel_core::{
        CoreError, ProjectId, ProjectRegistration, ProjectValidationState, RunStatus, SafeRunError,
    };
    use sentinel_git::RepositoryFingerprint;
    use sentinel_git::{inspect_staged_index_changes, parse_status_porcelain_v2_z};
    use serde_json::json;
    use std::process::{Command, Stdio};
    use tempfile::tempdir;

    const B1_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    async fn collect_b1_events(
        receiver: &mut tokio::sync::mpsc::Receiver<B1TestEvent>,
        expected_count: usize,
    ) -> Vec<B1TestEvent> {
        let mut events = Vec::with_capacity(expected_count);
        for _ in 0..expected_count {
            let event = tokio::time::timeout(B1_TEST_TIMEOUT, receiver.recv())
                .await
                .expect("B1 observation must arrive before the bounded test timeout")
                .expect("B1 observation channel must remain open during the request");
            events.push(event);
        }
        assert!(
            receiver.try_recv().is_err(),
            "B1 request emitted an unexpected observation"
        );
        events
    }

    async fn await_b1_request(
        task: &mut tokio::task::JoinHandle<Result<FileDiffClassification, SafeError>>,
    ) -> Result<FileDiffClassification, SafeError> {
        match tokio::time::timeout(B1_TEST_TIMEOUT, &mut *task).await {
            Ok(joined) => joined.expect("B1 task must not panic"),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("B1 request did not terminate before the bounded test timeout");
            }
        }
    }

    async fn await_b1_post_numstat_pause(
        reached: &tokio::sync::Notify,
        resume: &tokio::sync::Notify,
        task: &mut tokio::task::JoinHandle<Result<FileDiffClassification, SafeError>>,
    ) {
        if tokio::time::timeout(B1_TEST_TIMEOUT, reached.notified())
            .await
            .is_err()
        {
            resume.notify_one();
            let _ = await_b1_request(task).await;
            panic!("first complete evidence pass did not reach its bounded pause");
        }
    }

    fn fixture_git(directory: &Path, args: &[&str]) {
        let output = Command::new("/usr/bin/git")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture git failed");
    }

    fn fixture_git_output(directory: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("/usr/bin/git")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture git output failed");
        output.stdout
    }

    fn fixture_git_path(directory: &Path, name: &str) -> PathBuf {
        let output = fixture_git_output(directory, &["rev-parse", "--git-path", name]);
        let spelling = String::from_utf8(output).unwrap();
        let path = PathBuf::from(spelling.trim_end());
        if path.is_absolute() {
            path
        } else {
            directory.join(path)
        }
    }

    fn fixture_index_bytes(directory: &Path) -> Vec<u8> {
        std::fs::read(fixture_git_path(directory, "index")).unwrap()
    }

    #[cfg(unix)]
    struct StagedFilterFixture {
        _temporary_root: tempfile::TempDir,
        primary: PathBuf,
        managed: PathBuf,
        base_commit: String,
        managed_index: PathBuf,
        managed_index_lock: PathBuf,
    }

    /// Builds a linked-worktree index fixture whose filter driver is deliberately
    /// configured only after all content-sensitive setup has completed.
    #[cfg(unix)]
    fn staged_filter_fixture() -> StagedFilterFixture {
        let temporary_root = tempdir().unwrap();
        let primary = temporary_root.path().join("primary");
        let managed = temporary_root.path().join("managed");
        std::fs::create_dir(&primary).unwrap();
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        std::fs::write(
            primary.join(".gitattributes"),
            "file.txt filter=sentinel-marker\n",
        )
        .unwrap();
        std::fs::write(primary.join("file.txt"), "before\n").unwrap();
        fixture_git(&primary, &["add", ".gitattributes", "file.txt"]);
        fixture_git(&primary, &["commit", "-m", "filter fixture"]);
        fixture_git(
            &primary,
            &[
                "worktree",
                "add",
                "--detach",
                managed.to_str().unwrap(),
                "HEAD",
            ],
        );
        let base_commit = String::from_utf8(fixture_git_output(&managed, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let managed_index = fixture_git_path(&managed, "index");
        let managed_index_lock = fixture_git_path(&managed, "index.lock");

        // This is intentionally the last content-sensitive setup operation.
        std::fs::write(managed.join("file.txt"), "after\n").unwrap();
        fixture_git(&managed, &["add", "file.txt"]);

        StagedFilterFixture {
            _temporary_root: temporary_root,
            primary,
            managed,
            base_commit,
            managed_index,
            managed_index_lock,
        }
    }

    #[cfg(unix)]
    fn install_filter_marker(fixture: &StagedFilterFixture, kind: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let marker = fixture
            .primary
            .parent()
            .unwrap()
            .join(format!("{kind}-target-marker"));
        let helper = fixture
            .primary
            .parent()
            .unwrap()
            .join(format!("{kind}-target-helper"));
        std::fs::write(
            &helper,
            format!("#!/bin/sh\nprintf launched > {}\ncat\n", marker.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).unwrap();
        fixture_git(
            &fixture.primary,
            &[
                "config",
                &format!("filter.sentinel-marker.{kind}"),
                helper.to_str().unwrap(),
            ],
        );
        assert!(
            !marker.exists(),
            "the unique target marker must not exist before the target command"
        );
        (marker, helper)
    }

    #[cfg(unix)]
    fn direct_staged_diff_index(fixture: &StagedFilterFixture) -> std::process::Output {
        Command::new("/usr/bin/git")
            .args([
                "--no-optional-locks",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "diff-index",
                "--cached",
                "--raw",
                "-z",
                "--no-renames",
                fixture.base_commit.as_str(),
                "--",
            ])
            .current_dir(&fixture.managed)
            .env_clear()
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn fixture_readonly_status(directory: &Path) -> Vec<u8> {
        fixture_git_output(
            directory,
            &[
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
                "--no-renames",
            ],
        )
    }

    /// Test-only legacy control. AH1 production code must never invoke this
    /// stock-status command because it can execute repository filters.
    fn legacy_unsafe_status_control(directory: &Path) -> Vec<ChangedFile> {
        parse_status_porcelain_v2_z(&fixture_readonly_status(directory)).unwrap()
    }

    struct InventorySnapshot {
        head: Option<String>,
        branch: Option<String>,
        index: Vec<u8>,
        status: Vec<u8>,
        readme: String,
        untracked: Option<String>,
        index_lock_exists: bool,
    }

    async fn inventory_snapshot(directory: &Path, untracked_name: &str) -> InventorySnapshot {
        let inspection = inspect_repository(directory).await.unwrap();
        InventorySnapshot {
            head: inspection.head,
            branch: inspection.branch,
            index: fixture_index_bytes(directory),
            status: fixture_readonly_status(directory),
            readme: std::fs::read_to_string(directory.join("README.md")).unwrap(),
            untracked: std::fs::read_to_string(directory.join(untracked_name)).ok(),
            index_lock_exists: fixture_git_path(directory, "index.lock").exists(),
        }
    }

    async fn inventory_fixture(
        state_kind: ManagedWorktreeState,
    ) -> (tempfile::TempDir, DesktopState, ManagedWorktree) {
        let fixture = tempdir().unwrap();
        let primary = fixture.path().join("primary");
        std::fs::create_dir(&primary).unwrap();
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        std::fs::write(primary.join("README.md"), "fixture").unwrap();
        fixture_git(&primary, &["add", "README.md"]);
        fixture_git(&primary, &["commit", "-m", "fixture"]);
        let inspection = inspect_repository(&primary).await.unwrap();
        let database_path = fixture.path().join("inventory.sqlite");
        let repository = RunRepository::open(&format!("sqlite://{}", database_path.display()))
            .await
            .unwrap();
        let project = repository
            .register_project(ProjectRegistration {
                display_name: "Inventory fixture".into(),
                repository_identity: inspection.identity.clone(),
                repository_fingerprint: inspection.fingerprint.as_str().into(),
                fingerprint_scheme: ProjectFingerprintScheme::StrongV1,
                repository_root: inspection.repository_root.to_string_lossy().into_owned(),
                primary_root: inspection.primary_root.to_string_lossy().into_owned(),
                git_common_dir: inspection.common_dir.to_string_lossy().into_owned(),
                branch: inspection.branch.clone(),
                head: inspection.head.clone(),
                validation_state: ProjectValidationState::Valid,
                is_primary_worktree: true,
            })
            .await
            .unwrap();
        let app_data = fixture.path().join("app-data");
        std::fs::create_dir(&app_data).unwrap();
        let state = reconciliation_test_state(
            repository.clone(),
            database_path,
            app_data.join("worktrees"),
            None,
        );
        let id = WorktreeId::new();
        let leaf = worktree_parent(&state.worktree_root, &project.id)
            .unwrap()
            .join(id.to_string());
        let commit = inspection.head.unwrap();
        add_detached_worktree(&primary, &leaf, &commit)
            .await
            .unwrap();
        let row = repository
            .insert_creating_worktree(id, &project, leaf.to_string_lossy().into_owned(), commit)
            .await
            .unwrap();
        let row = if state_kind == ManagedWorktreeState::Ready {
            repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Ready,
                    None,
                )
                .await
                .unwrap()
        } else {
            let ready = repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Ready,
                    None,
                )
                .await
                .unwrap();
            repository
                .transition_worktree(
                    &ready.id,
                    ManagedWorktreeState::Ready,
                    ManagedWorktreeState::RetainedDirty,
                    Some("worktree_dirty"),
                )
                .await
                .unwrap()
        };
        (fixture, state, row)
    }

    #[tokio::test]
    async fn production_inventory_service_is_read_only_for_ready_and_retained_dirty_worktrees() {
        for state_kind in [
            ManagedWorktreeState::Ready,
            ManagedWorktreeState::RetainedDirty,
        ] {
            let (fixture, state, row) = inventory_fixture(state_kind).await;
            let leaf = PathBuf::from(&row.path);
            let primary = fixture.path().join("primary");
            let marker = fixture.path().join("fsmonitor-marker");
            let helper = fixture.path().join("fsmonitor-helper");
            std::fs::write(
                &helper,
                format!("#!/bin/sh\nprintf invoked > {}\n", marker.display()),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(&helper, permissions).unwrap();
            }
            fixture_git(
                &primary,
                &["config", "core.fsmonitor", helper.to_str().unwrap()],
            );
            assert_eq!(
                String::from_utf8(fixture_git_output(
                    &primary,
                    &["config", "--get", "core.fsmonitor"],
                ))
                .unwrap()
                .trim(),
                helper.to_str().unwrap(),
            );
            std::fs::write(leaf.join("README.md"), "managed modification").unwrap();
            std::fs::write(leaf.join("untracked file"), "not returned").unwrap();
            let managed_before = inventory_snapshot(&leaf, "untracked file").await;
            let primary_before = inventory_snapshot(&primary, "untracked file").await;
            assert!(!marker.exists());
            let inventory = inspect_worktree_changes_impl(&state, row.id.clone())
                .await
                .unwrap();
            assert!(!inventory.clean);
            assert!(inventory
                .files
                .iter()
                .any(|file| file.path.as_str() == "README.md"));
            assert!(inventory
                .files
                .iter()
                .any(|file| file.path.as_str() == "untracked file" && file.untracked));
            assert!(!marker.exists());
            let managed_after = inventory_snapshot(&leaf, "untracked file").await;
            let primary_after = inventory_snapshot(&primary, "untracked file").await;
            assert_eq!(managed_after.head, managed_before.head);
            assert_eq!(managed_after.branch, managed_before.branch);
            assert_eq!(managed_after.index, managed_before.index);
            assert_eq!(managed_after.status, managed_before.status);
            assert_eq!(managed_after.readme, managed_before.readme);
            assert_eq!(managed_after.untracked, managed_before.untracked);
            assert!(!managed_after.index_lock_exists);
            assert_eq!(primary_after.head, primary_before.head);
            assert_eq!(primary_after.branch, primary_before.branch);
            assert_eq!(primary_after.index, primary_before.index);
            assert_eq!(primary_after.status, primary_before.status);
            assert_eq!(primary_after.readme, primary_before.readme);
            assert_eq!(primary_after.untracked, primary_before.untracked);
            assert!(!primary_after.index_lock_exists);
            assert_eq!(
                state.repository.get_worktree(&row.id).await.unwrap().state,
                state_kind
            );
            acquire_project_worktree_lock(&state, &row.project_id)
                .await
                .unwrap();
            release_project_worktree_lock(&state, &row.project_id).await;
        }
    }

    #[tokio::test]
    async fn production_b1_classification_uses_fresh_inventory_and_preserves_both_worktrees() {
        let (fixture, state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        let primary = fixture.path().join("primary");
        let marker = fixture.path().join("b1-fsmonitor-marker");
        let helper = fixture.path().join("b1-fsmonitor-helper");
        std::fs::write(
            &helper,
            format!("#!/bin/sh\nprintf invoked > {}\n", marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&helper, permissions).unwrap();
        }
        std::fs::write(leaf.join("README.md"), "staged managed modification").unwrap();
        fixture_git(&leaf, &["add", "README.md"]);
        std::fs::write(leaf.join("README.md"), "unstaged managed modification").unwrap();
        fixture_git(
            &primary,
            &["config", "core.fsmonitor", helper.to_str().unwrap()],
        );
        fixture_git(
            &primary,
            &["config", "diff.external", helper.to_str().unwrap()],
        );
        fixture_git(
            &primary,
            &["config", "diff.fixture.command", helper.to_str().unwrap()],
        );
        fixture_git(
            &primary,
            &["config", "diff.fixture.textconv", helper.to_str().unwrap()],
        );
        std::fs::write(leaf.join(".gitattributes"), "README.md diff=fixture\n").unwrap();
        let managed_before = inventory_snapshot(&leaf, "untracked file").await;
        let primary_before = inventory_snapshot(&primary, "untracked file").await;
        let selected_path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        assert!(!marker.exists());
        let classification = inspect_worktree_file_diff_classification_impl(
            &state,
            row.id.clone(),
            selected_path.clone(),
        )
        .await
        .unwrap();
        assert_eq!(classification.path, selected_path);
        assert_eq!(
            classification.staged,
            DiffSectionClassification::TextEligible(TextEligibleMetadata {
                additions: 1,
                deletions: 1,
            })
        );
        assert_eq!(
            classification.unstaged,
            DiffSectionClassification::TextEligible(TextEligibleMetadata {
                additions: 1,
                deletions: 1,
            })
        );
        assert!(!marker.exists());
        let managed_after = inventory_snapshot(&leaf, "untracked file").await;
        let primary_after = inventory_snapshot(&primary, "untracked file").await;
        assert_eq!(managed_after.head, managed_before.head);
        assert_eq!(managed_after.branch, managed_before.branch);
        assert_eq!(managed_after.index, managed_before.index);
        assert_eq!(managed_after.status, managed_before.status);
        assert_eq!(managed_after.readme, managed_before.readme);
        assert_eq!(managed_after.untracked, managed_before.untracked);
        assert!(!managed_after.index_lock_exists);
        assert_eq!(primary_after.head, primary_before.head);
        assert_eq!(primary_after.branch, primary_before.branch);
        assert_eq!(primary_after.index, primary_before.index);
        assert_eq!(primary_after.status, primary_before.status);
        assert_eq!(primary_after.readme, primary_before.readme);
        assert_eq!(primary_after.untracked, primary_before.untracked);
        assert!(!primary_after.index_lock_exists);
        assert_eq!(
            state.repository.get_worktree(&row.id).await.unwrap().state,
            ManagedWorktreeState::Ready
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            acquire_project_worktree_lock(&state, &row.project_id),
        )
        .await
        .expect("classification must not leak the project lock")
        .unwrap();
        release_project_worktree_lock(&state, &row.project_id).await;
    }

    #[cfg(unix)]
    #[test]
    fn disposable_control_confirms_unprotected_unstaged_diff_executes_a_clean_filter() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempdir().unwrap();
        let repository = fixture.path().join("repository");
        std::fs::create_dir(&repository).unwrap();
        fixture_git(&repository, &["init"]);
        fixture_git(
            &repository,
            &["config", "user.email", "tests@example.invalid"],
        );
        fixture_git(&repository, &["config", "user.name", "Tests"]);
        std::fs::write(repository.join("file.txt"), "before\n").unwrap();
        std::fs::write(
            repository.join(".gitattributes"),
            "file.txt filter=sentinel-marker\n",
        )
        .unwrap();
        fixture_git(&repository, &["add", "file.txt", ".gitattributes"]);
        fixture_git(&repository, &["commit", "-m", "fixture"]);
        let marker = fixture.path().join("clean-filter-marker");
        let helper = fixture.path().join("clean-filter-helper");
        std::fs::write(
            &helper,
            format!("#!/bin/sh\nprintf invoked > {}\ncat\n", marker.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).unwrap();
        std::fs::write(repository.join("file.txt"), "after\n").unwrap();
        // Install the driver only after all content-sensitive fixture setup.
        // The following legacy diff control is therefore solely responsible
        // for any target-marker creation.
        fixture_git(
            &repository,
            &[
                "config",
                "filter.sentinel-marker.clean",
                helper.to_str().unwrap(),
            ],
        );
        assert!(!marker.exists());
        let _ = fixture_git_output(&repository, &["diff", "--numstat", "--", "file.txt"]);
        assert!(
            marker.exists(),
            "control demonstrates Git clean-filter execution"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_b1_defers_clean_and_process_filters_without_launching_helpers() {
        use std::os::unix::fs::PermissionsExt;

        for (kind, config_key) in [
            ("clean", "filter.sentinel-marker.clean"),
            ("process", "filter.sentinel-marker.process"),
        ] {
            let (fixture, state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
            let leaf = PathBuf::from(&row.path);
            let primary = fixture.path().join("primary");
            let marker = fixture.path().join(format!("{kind}-filter-marker"));
            let helper = fixture.path().join(format!("{kind}-filter-helper"));
            std::fs::write(
                &helper,
                format!("#!/bin/sh\nprintf invoked > {}\ncat\n", marker.display()),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&helper, permissions).unwrap();
            std::fs::write(
                leaf.join(".gitattributes"),
                "README.md filter=sentinel-marker\n",
            )
            .unwrap();
            fixture_git(&leaf, &["add", ".gitattributes"]);
            fixture_git(&primary, &["config", config_key, helper.to_str().unwrap()]);
            std::fs::write(leaf.join("README.md"), format!("{kind} filtered change")).unwrap();
            let managed_before = inventory_snapshot(&leaf, "untracked file").await;
            let primary_before = inventory_snapshot(&primary, "untracked file").await;
            let selected_path = inspect_worktree_changes_impl(&state, row.id.clone())
                .await
                .unwrap()
                .files
                .into_iter()
                .find(|file| file.path.as_str() == "README.md")
                .unwrap()
                .path;
            // Phase 3C-A status must not enter the filter conversion boundary.
            assert!(!marker.exists());
            let classification = inspect_worktree_file_diff_classification_impl(
                &state,
                row.id.clone(),
                selected_path,
            )
            .await
            .unwrap();
            assert_eq!(
                classification.staged,
                DiffSectionClassification::NotApplicable
            );
            assert_eq!(
                classification.unstaged,
                DiffSectionClassification::ExternalFilterDeferred
            );
            assert!(!marker.exists(), "{kind} helper must not be launched");
            let managed_after = inventory_snapshot(&leaf, "untracked file").await;
            let primary_after = inventory_snapshot(&primary, "untracked file").await;
            assert_eq!(managed_after.head, managed_before.head);
            assert_eq!(managed_after.branch, managed_before.branch);
            assert_eq!(managed_after.index, managed_before.index);
            assert_eq!(managed_after.status, managed_before.status);
            assert_eq!(managed_after.readme, managed_before.readme);
            assert_eq!(primary_after.head, primary_before.head);
            assert_eq!(primary_after.branch, primary_before.branch);
            assert_eq!(primary_after.index, primary_before.index);
            assert_eq!(primary_after.status, primary_before.status);
            assert!(!managed_after.index_lock_exists);
            assert!(!primary_after.index_lock_exists);
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                acquire_project_worktree_lock(&state, &row.project_id),
            )
            .await
            .expect("deferred filter request must release the project lock")
            .unwrap();
            release_project_worktree_lock(&state, &row.project_id).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_status_control_confirms_clean_filter_execution() {
        use std::os::unix::fs::PermissionsExt;

        let (fixture, _state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        let primary = fixture.path().join("primary");
        let marker = fixture.path().join("staged-filter-marker");
        let helper = fixture.path().join("staged-filter-helper");
        std::fs::write(
            &helper,
            format!("#!/bin/sh\nprintf invoked > {}\ncat\n", marker.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).unwrap();
        std::fs::write(
            leaf.join(".gitattributes"),
            "README.md filter=sentinel-marker\n",
        )
        .unwrap();
        fixture_git(&leaf, &["add", ".gitattributes"]);
        std::fs::write(leaf.join("README.md"), "staged filtered change").unwrap();
        fixture_git(&leaf, &["add", "README.md"]);
        fixture_git(
            &primary,
            &[
                "config",
                "filter.sentinel-marker.clean",
                helper.to_str().unwrap(),
            ],
        );
        let _path = legacy_unsafe_status_control(&leaf)
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        assert!(
            marker.exists(),
            "legacy stock status launches a clean filter"
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_status_control_confirms_process_filter_launchability() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempdir().unwrap();
        let repository = fixture.path().join("repository");
        std::fs::create_dir(&repository).unwrap();
        fixture_git(&repository, &["init"]);
        fixture_git(
            &repository,
            &["config", "user.email", "tests@example.invalid"],
        );
        fixture_git(&repository, &["config", "user.name", "Tests"]);
        std::fs::write(repository.join("file.txt"), "before\n").unwrap();
        std::fs::write(
            repository.join(".gitattributes"),
            "file.txt filter=sentinel-process-marker\n",
        )
        .unwrap();
        fixture_git(&repository, &["add", "file.txt", ".gitattributes"]);
        fixture_git(&repository, &["commit", "-m", "fixture"]);
        let marker = fixture.path().join("process-filter-marker");
        let helper = fixture.path().join("process-filter-helper");
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\nprintf launched > {}\nexit 1\n",
                marker.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).unwrap();
        std::fs::write(repository.join("file.txt"), "after\n").unwrap();
        fixture_git(&repository, &["add", "file.txt"]);
        fixture_git(
            &repository,
            &[
                "config",
                "filter.sentinel-process-marker.process",
                helper.to_str().unwrap(),
            ],
        );
        let _ = Command::new("/usr/bin/git")
            .args([
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
                "--no-renames",
            ])
            .current_dir(&repository)
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "legacy stock status reaches the configured process helper"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_diff_index_filter_markers_are_attributable_to_the_exact_target() {
        for kind in ["clean", "process"] {
            let fixture = staged_filter_fixture();
            let before_index = std::fs::read(&fixture.managed_index).unwrap();
            let before_head = fixture_git_output(&fixture.managed, &["rev-parse", "HEAD"]);
            let (marker, _helper) = install_filter_marker(&fixture, kind);

            let direct = direct_staged_diff_index(&fixture);
            assert!(
                direct.status.success(),
                "direct {kind} control must succeed"
            );
            assert!(
                !direct.stdout.is_empty(),
                "direct {kind} control must report the staged modification"
            );
            assert!(
                !marker.exists(),
                "the direct diff-index control must not launch the {kind} filter"
            );
            assert_eq!(before_index, std::fs::read(&fixture.managed_index).unwrap());
            assert_eq!(
                before_head,
                fixture_git_output(&fixture.managed, &["rev-parse", "HEAD"])
            );
            assert!(!fixture.managed_index_lock.exists());
        }

        for kind in ["clean", "process"] {
            let fixture = staged_filter_fixture();
            let before_index = std::fs::read(&fixture.managed_index).unwrap();
            let before_head = fixture_git_output(&fixture.managed, &["rev-parse", "HEAD"]);
            let (marker, _helper) = install_filter_marker(&fixture, kind);

            let changes = inspect_staged_index_changes(&fixture.managed, &fixture.base_commit)
                .await
                .expect("filter-free staged metadata");
            assert!(changes
                .iter()
                .any(|change| change.path.as_str() == "file.txt"));
            assert!(
                !marker.exists(),
                "the sentinel-git diff-index operation must not launch the {kind} filter"
            );
            assert_eq!(before_index, std::fs::read(&fixture.managed_index).unwrap());
            assert_eq!(
                before_head,
                fixture_git_output(&fixture.managed, &["rev-parse", "HEAD"])
            );
            assert!(!fixture.managed_index_lock.exists());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_inventory_path_does_not_launch_filter_markers() {
        for kind in ["clean", "process"] {
            let (fixture, state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
            let leaf = PathBuf::from(&row.path);
            let primary = fixture.path().join("primary");
            let marker = fixture.path().join(format!("inventory-{kind}-marker"));
            let helper = fixture.path().join(format!("inventory-{kind}-helper"));
            std::fs::write(
                &helper,
                format!("#!/bin/sh\nprintf launched > {}\ncat\n", marker.display()),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&helper, permissions).unwrap();

            // Complete all content-sensitive fixture work before installing the
            // driver. The production request below must be the first operation
            // allowed to observe the configured filter.
            std::fs::write(
                leaf.join(".gitattributes"),
                "README.md filter=sentinel-marker\n",
            )
            .unwrap();
            std::fs::write(leaf.join("README.md"), "staged inventory fixture\n").unwrap();
            fixture_git(&leaf, &["add", ".gitattributes", "README.md"]);
            let managed_index_before = fixture_index_bytes(&leaf);
            let primary_index_before = fixture_index_bytes(&primary);
            let managed_head_before = fixture_git_output(&leaf, &["rev-parse", "HEAD"]);
            let primary_head_before = fixture_git_output(&primary, &["rev-parse", "HEAD"]);
            fixture_git(
                &primary,
                &[
                    "config",
                    &format!("filter.sentinel-marker.{kind}"),
                    helper.to_str().unwrap(),
                ],
            );
            assert!(!marker.exists());

            let inventory = inspect_worktree_changes_impl(&state, row.id.clone())
                .await
                .unwrap();
            assert!(!inventory.clean);
            assert!(
                !marker.exists(),
                "the production inventory path must not launch the {kind} filter"
            );
            assert_eq!(managed_index_before, fixture_index_bytes(&leaf));
            assert_eq!(primary_index_before, fixture_index_bytes(&primary));
            assert_eq!(
                managed_head_before,
                fixture_git_output(&leaf, &["rev-parse", "HEAD"])
            );
            assert_eq!(
                primary_head_before,
                fixture_git_output(&primary, &["rev-parse", "HEAD"])
            );
            assert!(!fixture_git_path(&leaf, "index.lock").exists());
            assert!(!fixture_git_path(&primary, "index.lock").exists());
            acquire_project_worktree_lock(&state, &row.project_id)
                .await
                .unwrap();
            release_project_worktree_lock(&state, &row.project_id).await;
        }
    }

    #[tokio::test]
    async fn production_b1_rejects_a_lifecycle_transition_after_numstat_before_final_validation() {
        let test_timeout = std::time::Duration::from_secs(5);
        let (fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        let primary = fixture.path().join("primary");
        std::fs::write(leaf.join("README.md"), "staged race fixture").unwrap();
        fixture_git(&leaf, &["add", "README.md"]);
        let managed_before = inventory_snapshot(&leaf, "untracked file").await;
        let primary_before = inventory_snapshot(&primary, "untracked file").await;
        let selected_path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .expect("staged path is inventoried")
            .path;

        let post_numstat_reached = Arc::new(tokio::sync::Notify::new());
        let resume_post_numstat = Arc::new(tokio::sync::Notify::new());
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: post_numstat_reached.clone(),
            resume_post_numstat: resume_post_numstat.clone(),
            post_c2_equality_reached: None,
            resume_post_c2_equality: None,
            final_validation_reached: None,
            resume_final_validation: None,
            final_persisted_reload_reached: None,
            resume_final_persisted_reload: None,
            observations: None,
        });
        let reached = post_numstat_reached.notified();
        let service_state = state.clone();
        let worktree_id = row.id.clone();
        let mut classification = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(
                &service_state,
                worktree_id,
                selected_path,
            )
            .await
        });

        if tokio::time::timeout(test_timeout, reached).await.is_err() {
            resume_post_numstat.notify_one();
            if tokio::time::timeout(test_timeout, &mut classification)
                .await
                .is_err()
            {
                classification.abort();
                let _ = classification.await;
            }
            panic!("B1 service did not complete numstat before its final validation");
        }

        // This typed conditional transition intentionally bypasses the in-memory
        // lock: it models a separately authorised persisted lifecycle change.
        let transition = state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Ready,
                ManagedWorktreeState::Removing,
                None,
            )
            .await;
        // Always unblock the state-local hook before asserting transition
        // success, so a failed test setup cannot leave a detached B1 task.
        resume_post_numstat.notify_one();
        let transitioned = match transition {
            Ok(row) => row,
            Err(error) => {
                if tokio::time::timeout(test_timeout, &mut classification)
                    .await
                    .is_err()
                {
                    classification.abort();
                    let _ = classification.await;
                }
                panic!("typed lifecycle transition failed: {error:?}");
            }
        };
        assert_eq!(transitioned.state, ManagedWorktreeState::Removing);

        let result = match tokio::time::timeout(test_timeout, &mut classification).await {
            Ok(joined) => joined.expect("B1 task must not panic"),
            Err(_) => {
                classification.abort();
                let _ = classification.await;
                panic!("B1 service did not resume after the post-numstat pause");
            }
        };
        let error = result.expect_err("a stale classification must not escape");
        assert_eq!(error.code, "worktree_not_ready");
        assert_eq!(
            error.message,
            "This worktree is not available for inspection."
        );
        assert!(!error.message.contains(&row.path));
        assert!(!error.message.contains(&row.base_commit));

        assert_eq!(
            state.repository.get_worktree(&row.id).await.unwrap().state,
            ManagedWorktreeState::Removing
        );
        let managed_after = inventory_snapshot(&leaf, "untracked file").await;
        let primary_after = inventory_snapshot(&primary, "untracked file").await;
        assert_eq!(managed_after.head, managed_before.head);
        assert_eq!(managed_after.branch, managed_before.branch);
        assert_eq!(managed_after.index, managed_before.index);
        assert_eq!(managed_after.status, managed_before.status);
        assert_eq!(managed_after.readme, managed_before.readme);
        assert_eq!(managed_after.untracked, managed_before.untracked);
        assert!(!managed_after.index_lock_exists);
        assert_eq!(primary_after.head, primary_before.head);
        assert_eq!(primary_after.branch, primary_before.branch);
        assert_eq!(primary_after.index, primary_before.index);
        assert_eq!(primary_after.status, primary_before.status);
        assert_eq!(primary_after.readme, primary_before.readme);
        assert_eq!(primary_after.untracked, primary_before.untracked);
        assert!(!primary_after.index_lock_exists);

        tokio::time::timeout(
            test_timeout,
            acquire_project_worktree_lock(&state, &row.project_id),
        )
        .await
        .expect("stale B1 request must release the original project lock")
        .unwrap();
        release_project_worktree_lock(&state, &row.project_id).await;
    }

    #[tokio::test]
    async fn production_b1_rejects_transient_content_change_between_evidence_passes() {
        let (fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        const V1: &str = "v1\nadded-one\n";
        const V2: &str = "v2\nadded-one\nadded-two\nadded-three\n";
        std::fs::write(leaf.join("README.md"), V1).unwrap();
        let selected_path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let (observations, mut receiver) = tokio::sync::mpsc::channel(24);
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: reached.clone(),
            resume_post_numstat: resume.clone(),
            post_c2_equality_reached: None,
            resume_post_c2_equality: None,
            final_validation_reached: None,
            resume_final_validation: None,
            final_persisted_reload_reached: None,
            resume_final_persisted_reload: None,
            observations: Some(observations),
        });
        let service_state = state.clone();
        let mut task = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(
                &service_state,
                row.id.clone(),
                selected_path,
            )
            .await
        });
        await_b1_post_numstat_pause(&reached, &resume, &mut task).await;
        // Both versions are ordinary unstaged modifications; only real numstat
        // counts differ, so inventory A and B remain equal at ChangedFile level.
        std::fs::write(leaf.join("README.md"), V2).unwrap();
        resume.notify_one();
        let error = await_b1_request(&mut task)
            .await
            .expect_err("different C1/C2 numstat evidence must fail closed");
        let events = collect_b1_events(&mut receiver, 9).await;
        let [B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::A,
            observation: inventory_a,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C1,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C1,
            path_key: first_path_key,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
            mode_only: false,
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C1,
            observation: evidence_one,
        }, B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::B,
            observation: inventory_b,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C2,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C2,
            path_key: second_path_key,
            surface: B1TestSurface::Unstaged,
            additions: Some(4),
            deletions: Some(1),
            binary: false,
            mode_only: false,
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C2,
            observation: evidence_two,
        }, B1TestEvent::EvidenceComparisonCompleted { equal: false }] = events.as_slice()
        else {
            panic!("unexpected transient B1 observation sequence: {events:?}");
        };
        assert_eq!(inventory_a, inventory_b, "inventory A must equal B");
        assert_eq!(inventory_a.file_count, 1);
        assert_eq!(first_path_key, second_path_key);
        assert_ne!(
            evidence_one, evidence_two,
            "complete C1/C2 evidence differs"
        );
        assert_eq!(evidence_one.entry_count, 1);
        assert_eq!(
            evidence_one.normalized_path_keys,
            evidence_two.normalized_path_keys
        );
        assert_eq!(evidence_one.staged, evidence_two.staged);
        assert_ne!(evidence_one.unstaged, evidence_two.unstaged);
        assert_eq!(evidence_one.mode_head, evidence_two.mode_head);
        assert_eq!(evidence_one.mode_index, evidence_two.mode_index);
        assert_eq!(evidence_one.mode_worktree, evidence_two.mode_worktree);
        assert_eq!(error.code, "change_stale");
        assert_eq!(std::fs::read_to_string(leaf.join("README.md")).unwrap(), V2);
        drop(fixture);
    }

    #[tokio::test]
    async fn production_b1_stable_content_executes_two_matching_evidence_passes() {
        let (_fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        std::fs::write(leaf.join("README.md"), "stable\nadded-one\n").unwrap();
        let path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let (observations, mut receiver) = tokio::sync::mpsc::channel(24);
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: reached.clone(),
            resume_post_numstat: resume.clone(),
            post_c2_equality_reached: None,
            resume_post_c2_equality: None,
            final_validation_reached: None,
            resume_final_validation: None,
            final_persisted_reload_reached: None,
            resume_final_persisted_reload: None,
            observations: Some(observations),
        });
        let service_state = state.clone();
        let mut task = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(&service_state, row.id.clone(), path)
                .await
        });
        await_b1_post_numstat_pause(&reached, &resume, &mut task).await;
        resume.notify_one();
        let events = collect_b1_events(&mut receiver, 18).await;
        let [B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::A,
            observation: inventory_a,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C1,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C1,
            path_key: first_path_key,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
            mode_only: false,
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C1,
            observation: evidence_one,
        }, B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::B,
            observation: inventory_b,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C2,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C2,
            path_key: second_path_key,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
            mode_only: false,
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C2,
            observation: evidence_two,
        }, B1TestEvent::EvidenceComparisonCompleted { equal: true }, B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::C,
            observation: inventory_c,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C3,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C3,
            path_key: third_path_key,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
            mode_only: false,
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C3,
            observation: evidence_three,
        }, B1TestEvent::FinalEvidenceComparisonCompleted { equal: true }, B1TestEvent::FinalPersistedStateReloadStarted, B1TestEvent::FinalPersistedStateReloadCompleted {
            lifecycle: B1TestLifecycle::Eligible,
            accepted: true,
        }, B1TestEvent::FinalValidationStarted {
            stage: B1TestInventoryStage::C,
        }, B1TestEvent::FinalValidationCompleted {
            stage: B1TestInventoryStage::C,
        }] = events.as_slice()
        else {
            panic!("unexpected stable B1 observation sequence: {events:?}");
        };
        assert_eq!(inventory_a, inventory_b, "inventory A must equal B");
        assert_eq!(inventory_b, inventory_c, "inventory B must equal C");
        assert_eq!(inventory_a, inventory_c, "inventory A must equal C");
        assert_eq!(inventory_a.file_count, 1);
        assert_eq!(first_path_key, second_path_key);
        assert_eq!(second_path_key, third_path_key);
        assert_eq!(
            evidence_one, evidence_two,
            "complete C1/C2 evidence matches"
        );
        assert_eq!(
            evidence_two, evidence_three,
            "complete C2/C3 evidence matches"
        );
        assert_eq!(evidence_one.entry_count, 1);
        assert_eq!(evidence_one.normalized_path_keys, vec![0]);
        assert_eq!(evidence_one.unstaged.additions, Some(2));
        assert_eq!(evidence_one.unstaged.deletions, Some(1));
        let classification = await_b1_request(&mut task)
            .await
            .expect("InventoryCompleted(C) must precede the successful DTO");
        assert_eq!(
            classification.unstaged,
            DiffSectionClassification::TextEligible(TextEligibleMetadata {
                additions: 2,
                deletions: 1
            })
        );
    }

    #[tokio::test]
    async fn production_b1_rejects_content_change_after_c2_before_final_evidence() {
        let (fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        const V1: &str = "v1\nadded-one\n";
        const V2: &str = "v2\nadded-one\nadded-two\nadded-three\n";
        std::fs::write(leaf.join("README.md"), V1).unwrap();
        let path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        let post_numstat_reached = Arc::new(tokio::sync::Notify::new());
        let resume_post_numstat = Arc::new(tokio::sync::Notify::new());
        let post_c2_equality_reached = Arc::new(tokio::sync::Notify::new());
        let resume_post_c2_equality = Arc::new(tokio::sync::Notify::new());
        let (observations, mut receiver) = tokio::sync::mpsc::channel(24);
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: post_numstat_reached.clone(),
            resume_post_numstat: resume_post_numstat.clone(),
            post_c2_equality_reached: Some(post_c2_equality_reached.clone()),
            resume_post_c2_equality: Some(resume_post_c2_equality.clone()),
            final_validation_reached: None,
            resume_final_validation: None,
            final_persisted_reload_reached: None,
            resume_final_persisted_reload: None,
            observations: Some(observations),
        });
        let c2_equality_wait = post_c2_equality_reached.notified();
        let service_state = state.clone();
        let mut task = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(&service_state, row.id.clone(), path)
                .await
        });
        await_b1_post_numstat_pause(&post_numstat_reached, &resume_post_numstat, &mut task).await;
        resume_post_numstat.notify_one();
        tokio::time::timeout(B1_TEST_TIMEOUT, c2_equality_wait)
            .await
            .expect("C1/C2 equality must precede the post-C2 mutation");
        std::fs::write(leaf.join("README.md"), V2).unwrap();
        resume_post_c2_equality.notify_one();
        let error = await_b1_request(&mut task)
            .await
            .expect_err("post-C2 content drift must fail C2/C3 coherence");
        let events = collect_b1_events(&mut receiver, 14).await;
        let [B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::A,
            observation: inventory_a,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C1,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C1,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            ..
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C1,
            observation: evidence_one,
        }, B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::B,
            observation: inventory_b,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C2,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C2,
            surface: B1TestSurface::Unstaged,
            additions: Some(2),
            deletions: Some(1),
            ..
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C2,
            observation: evidence_two,
        }, B1TestEvent::EvidenceComparisonCompleted { equal: true }, B1TestEvent::InventoryCompleted {
            stage: B1TestInventoryStage::C,
            observation: inventory_c,
        }, B1TestEvent::ClassificationPassStarted {
            pass: B1TestPass::C3,
        }, B1TestEvent::NumstatCompleted {
            pass: B1TestPass::C3,
            surface: B1TestSurface::Unstaged,
            additions: Some(4),
            deletions: Some(1),
            ..
        }, B1TestEvent::ClassificationPassCompleted {
            pass: B1TestPass::C3,
            observation: evidence_three,
        }, B1TestEvent::FinalEvidenceComparisonCompleted { equal: false }] = events.as_slice()
        else {
            panic!("unexpected post-C2 drift sequence: {events:?}");
        };
        assert_eq!(inventory_a, inventory_b);
        assert_eq!(inventory_b, inventory_c);
        assert_eq!(evidence_one, evidence_two);
        assert_ne!(evidence_two, evidence_three);
        assert_eq!(error.code, "change_stale");
        assert_eq!(std::fs::read_to_string(leaf.join("README.md")).unwrap(), V2);
        drop(fixture);
    }

    #[tokio::test]
    async fn production_b1_rejects_lifecycle_transition_after_inventory_c_begins() {
        let (_fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        std::fs::write(leaf.join("README.md"), "stable\nadded-one\n").unwrap();
        let path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        let post_numstat_reached = Arc::new(tokio::sync::Notify::new());
        let resume_post_numstat = Arc::new(tokio::sync::Notify::new());
        let final_persisted_reload_reached = Arc::new(tokio::sync::Notify::new());
        let resume_final_persisted_reload = Arc::new(tokio::sync::Notify::new());
        let (observations, mut receiver) = tokio::sync::mpsc::channel(24);
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: post_numstat_reached.clone(),
            resume_post_numstat: resume_post_numstat.clone(),
            post_c2_equality_reached: None,
            resume_post_c2_equality: None,
            final_validation_reached: None,
            resume_final_validation: None,
            final_persisted_reload_reached: Some(final_persisted_reload_reached.clone()),
            resume_final_persisted_reload: Some(resume_final_persisted_reload.clone()),
            observations: Some(observations),
        });
        let final_reload_wait = final_persisted_reload_reached.notified();
        let service_state = state.clone();
        let worktree_id = row.id.clone();
        let mut task = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(&service_state, worktree_id, path).await
        });
        await_b1_post_numstat_pause(&post_numstat_reached, &resume_post_numstat, &mut task).await;
        resume_post_numstat.notify_one();
        tokio::time::timeout(B1_TEST_TIMEOUT, final_reload_wait)
            .await
            .expect("matching C3 must reach the final persisted-state reload");
        let transitioned = state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Ready,
                ManagedWorktreeState::Removing,
                None,
            )
            .await
            .expect("typed disposable lifecycle transition");
        assert_eq!(transitioned.state, ManagedWorktreeState::Removing);
        resume_final_persisted_reload.notify_one();
        let error = await_b1_request(&mut task)
            .await
            .expect_err("late lifecycle revocation must reject the DTO");
        let events = collect_b1_events(&mut receiver, 16).await;
        assert!(matches!(
            events.as_slice(),
            [
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::A,
                    ..
                },
                _,
                _,
                _,
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::B,
                    ..
                },
                _,
                _,
                _,
                B1TestEvent::EvidenceComparisonCompleted { equal: true },
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::C,
                    ..
                },
                B1TestEvent::ClassificationPassStarted {
                    pass: B1TestPass::C3,
                },
                _,
                B1TestEvent::ClassificationPassCompleted {
                    pass: B1TestPass::C3,
                    ..
                },
                B1TestEvent::FinalEvidenceComparisonCompleted { equal: true },
                B1TestEvent::FinalPersistedStateReloadStarted,
                B1TestEvent::FinalPersistedStateReloadCompleted {
                    lifecycle: B1TestLifecycle::Removing,
                    accepted: false,
                },
            ]
        ));
        assert_eq!(error.code, "worktree_not_ready");
        assert_eq!(
            state.repository.get_worktree(&row.id).await.unwrap().state,
            ManagedWorktreeState::Removing
        );
    }

    #[tokio::test]
    async fn production_b1_rejects_repository_validation_failure_after_final_validation_started() {
        let (_fixture, mut state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        std::fs::write(leaf.join("README.md"), "stable\nadded-one\n").unwrap();
        let path = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path.as_str() == "README.md")
            .unwrap()
            .path;
        let post_numstat_reached = Arc::new(tokio::sync::Notify::new());
        let resume_post_numstat = Arc::new(tokio::sync::Notify::new());
        let final_validation_reached = Arc::new(tokio::sync::Notify::new());
        let resume_final_validation = Arc::new(tokio::sync::Notify::new());
        let (observations, mut receiver) = tokio::sync::mpsc::channel(24);
        state.b1_test_hooks = Some(B1TestHooks {
            post_numstat_reached: post_numstat_reached.clone(),
            resume_post_numstat: resume_post_numstat.clone(),
            post_c2_equality_reached: None,
            resume_post_c2_equality: None,
            final_validation_reached: Some(final_validation_reached.clone()),
            resume_final_validation: Some(resume_final_validation.clone()),
            final_persisted_reload_reached: None,
            resume_final_persisted_reload: None,
            observations: Some(observations),
        });
        let final_validation_wait = final_validation_reached.notified();
        let service_state = state.clone();
        let mut task = tokio::spawn(async move {
            inspect_worktree_file_diff_classification_impl(&service_state, row.id.clone(), path)
                .await
        });
        await_b1_post_numstat_pause(&post_numstat_reached, &resume_post_numstat, &mut task).await;
        resume_post_numstat.notify_one();
        tokio::time::timeout(B1_TEST_TIMEOUT, final_validation_wait)
            .await
            .expect("Inventory C must reach its real final validation boundary");

        // The fixture worktree's Git file is removed only while Inventory C is
        // paused immediately before its final repository inspection.
        std::fs::remove_file(leaf.join(".git")).unwrap();
        resume_final_validation.notify_one();
        let error = await_b1_request(&mut task)
            .await
            .expect_err("a failed Inventory C repository validation must reject the DTO");
        let events = collect_b1_events(&mut receiver, 17).await;
        assert!(matches!(
            events.as_slice(),
            [
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::A,
                    ..
                },
                _,
                _,
                _,
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::B,
                    ..
                },
                _,
                _,
                _,
                B1TestEvent::EvidenceComparisonCompleted { equal: true },
                B1TestEvent::InventoryCompleted {
                    stage: B1TestInventoryStage::C,
                    ..
                },
                B1TestEvent::ClassificationPassStarted {
                    pass: B1TestPass::C3,
                },
                _,
                B1TestEvent::ClassificationPassCompleted {
                    pass: B1TestPass::C3,
                    ..
                },
                B1TestEvent::FinalEvidenceComparisonCompleted { equal: true },
                B1TestEvent::FinalPersistedStateReloadStarted,
                B1TestEvent::FinalPersistedStateReloadCompleted {
                    lifecycle: B1TestLifecycle::Eligible,
                    accepted: true,
                },
                B1TestEvent::FinalValidationStarted {
                    stage: B1TestInventoryStage::C,
                },
            ]
        ));
        assert_eq!(error.code, "git_command_failed");
    }

    #[test]
    fn b1_metadata_only_policies_defer_untracked_conflict_symlink_and_gitlink_content() {
        const OID: &str = "0123456789abcdef0123456789abcdef01234567";
        let untracked = sentinel_git::parse_status_porcelain_v2_z(b"? untracked\0")
            .unwrap()
            .remove(0);
        assert_eq!(
            classify_without_numstat(&untracked, true),
            Some(DiffSectionClassification::NotApplicable)
        );
        assert_eq!(
            classify_without_numstat(&untracked, false),
            Some(DiffSectionClassification::UntrackedContentDeferred)
        );
        let conflict = sentinel_git::parse_status_porcelain_v2_z(
            format!("u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} conflict\0")
                .as_bytes(),
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            classify_without_numstat(&conflict, true),
            Some(DiffSectionClassification::ConflictContentDeferred)
        );
        assert_eq!(
            classify_without_numstat(&conflict, false),
            Some(DiffSectionClassification::ConflictContentDeferred)
        );
        let symlink = sentinel_git::parse_status_porcelain_v2_z(
            format!("1 M. N... 120000 120000 120000 {OID} {OID} link\0").as_bytes(),
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            classify_without_numstat(&symlink, true),
            Some(DiffSectionClassification::SymlinkMetadataOnly)
        );
        let gitlink = sentinel_git::parse_status_porcelain_v2_z(
            format!("1 M. SC.. 160000 160000 160000 {OID} {OID} module\0").as_bytes(),
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            classify_without_numstat(&gitlink, true),
            Some(DiffSectionClassification::SubmoduleMetadataOnly)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_b1_service_classifies_binary_mode_symlink_and_untracked_without_content() {
        use std::os::unix::fs::PermissionsExt;

        let (fixture, state, row) = inventory_fixture(ManagedWorktreeState::Ready).await;
        let leaf = PathBuf::from(&row.path);
        let external_target = fixture.path().join("external-target");
        std::fs::write(&external_target, "outside managed worktree").unwrap();
        let mut permissions = std::fs::metadata(leaf.join("README.md"))
            .unwrap()
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(leaf.join("README.md"), permissions).unwrap();
        std::os::unix::fs::symlink(&external_target, leaf.join("managed-link")).unwrap();
        std::fs::write(leaf.join("binary.bin"), [0_u8, 0x9f, 0x92, 0x96]).unwrap();
        std::fs::write(leaf.join("untracked.txt"), "deferred").unwrap();
        fixture_git(&leaf, &["add", "README.md", "managed-link", "binary.bin"]);
        let inventory = inspect_worktree_changes_impl(&state, row.id.clone())
            .await
            .unwrap()
            .files;
        for (name, staged, unstaged) in [
            (
                "README.md",
                DiffSectionClassification::ModeOnly,
                DiffSectionClassification::NotApplicable,
            ),
            (
                "managed-link",
                DiffSectionClassification::SymlinkMetadataOnly,
                DiffSectionClassification::SymlinkMetadataOnly,
            ),
            (
                "binary.bin",
                DiffSectionClassification::Binary,
                DiffSectionClassification::NotApplicable,
            ),
            (
                "untracked.txt",
                DiffSectionClassification::NotApplicable,
                DiffSectionClassification::UntrackedContentDeferred,
            ),
        ] {
            let path = inventory
                .iter()
                .find(|file| file.path.as_str() == name)
                .unwrap()
                .path
                .clone();
            let classification =
                inspect_worktree_file_diff_classification_impl(&state, row.id.clone(), path)
                    .await
                    .unwrap();
            assert_eq!(classification.staged, staged);
            assert_eq!(classification.unstaged, unstaged);
        }
        assert_eq!(
            state.repository.get_worktree(&row.id).await.unwrap().state,
            ManagedWorktreeState::Ready
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            acquire_project_worktree_lock(&state, &row.project_id),
        )
        .await
        .expect("all metadata-only outcomes release the project lock")
        .unwrap();
        release_project_worktree_lock(&state, &row.project_id).await;
    }

    #[cfg(unix)]
    #[test]
    fn managed_root_rejects_a_configured_symlink_before_canonicalization() {
        let app_data = tempdir().unwrap();
        let external = tempdir().unwrap();
        let root = app_data.path().join("worktrees");
        std::os::unix::fs::symlink(external.path(), &root).unwrap();
        assert!(worktree_parent(&root, &ProjectId::new()).is_err());
        assert!(std::fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn managed_leaf_replacement_symlink_is_rejected_without_following_target() {
        let app_data = tempdir().unwrap();
        let target = tempdir().unwrap();
        let project = ProjectId::new();
        let worktree = WorktreeId::new();
        let root = app_data.path().join("worktrees");
        let parent = worktree_parent(&root, &project).unwrap();
        let leaf = parent.join(worktree.to_string());
        std::os::unix::fs::symlink(target.path(), &leaf).unwrap();
        assert!(validate_managed_leaf(&root, &project, &worktree, &leaf).is_err());
        assert!(target.path().exists());
    }

    #[test]
    fn post_remove_requires_proven_missing_leaf_and_exact_metadata_absence() {
        assert_eq!(
            post_remove_state(
                Ok(ManagedLeafState::Missing),
                Ok(WorktreeMetadataLookup::Absent)
            ),
            ManagedWorktreeState::Removed
        );
        for leaf in [
            ManagedLeafState::RealDirectory,
            ManagedLeafState::UnsafeLinkOrReparse,
            ManagedLeafState::NonDirectory,
            ManagedLeafState::MetadataUnavailable,
        ] {
            assert_eq!(
                post_remove_state(Ok(leaf), Ok(WorktreeMetadataLookup::Absent)),
                ManagedWorktreeState::Failed
            );
        }
        assert_eq!(
            post_remove_state(
                Ok(ManagedLeafState::Missing),
                Ok(WorktreeMetadataLookup::Present)
            ),
            ManagedWorktreeState::Failed
        );
        assert_eq!(
            post_remove_state(
                Ok(ManagedLeafState::Missing),
                Err(GitError::MetadataInvalid)
            ),
            ManagedWorktreeState::Failed
        );
    }

    #[test]
    fn reconciliation_validation_failure_state_mapping_is_limited_to_active_states() {
        for state in [ManagedWorktreeState::Creating, ManagedWorktreeState::Ready] {
            assert_eq!(
                reconciliation_validation_failure_state(state),
                Some(ManagedWorktreeState::Failed)
            );
        }
        assert_eq!(
            reconciliation_validation_failure_state(ManagedWorktreeState::Removing),
            None
        );
    }

    fn reconciliation_registration(
        directory: &std::path::Path,
        identity: &str,
    ) -> ProjectRegistration {
        let repository_root = directory.join(format!("unavailable-repository-{identity}"));
        ProjectRegistration {
            display_name: format!("Fixture {identity}"),
            repository_identity: format!("fixture-identity-{identity}"),
            repository_fingerprint: format!("strong_v1:fixture:{identity}"),
            fingerprint_scheme: ProjectFingerprintScheme::StrongV1,
            repository_root: repository_root.to_string_lossy().into_owned(),
            primary_root: repository_root.to_string_lossy().into_owned(),
            git_common_dir: repository_root.join(".git").to_string_lossy().into_owned(),
            branch: Some("main".into()),
            head: Some("0123456789abcdef0123456789abcdef01234567".into()),
            validation_state: ProjectValidationState::Valid,
            is_primary_worktree: true,
        }
    }

    fn reconciliation_test_state(
        repository: RunRepository,
        database_path: PathBuf,
        worktree_root: PathBuf,
        hooks: Option<ReconciliationTestHooks>,
    ) -> DesktopState {
        let executable = std::env::current_exe().expect("test executable");
        DesktopState {
            orchestrator: RunOrchestrator::new(
                repository.clone(),
                FakeAgentProgram::from_executable(executable).expect("test executable program"),
            ),
            codex_exec_available: false,
            repository,
            database_path,
            forwarders: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            protected_application_repository: None,
            worktree_root,
            worktree_projects: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            reconciliation_test_hooks: hooks,
            b1_test_hooks: None,
        }
    }

    async fn insert_reconciliation_row(
        repository: &RunRepository,
        root: &Path,
        project: &Project,
        state: ManagedWorktreeState,
        create_leaf: bool,
    ) -> (ManagedWorktree, PathBuf) {
        let id = WorktreeId::new();
        let leaf = worktree_parent(root, &project.id)
            .expect("managed parent")
            .join(id.to_string());
        if create_leaf {
            std::fs::create_dir(&leaf).expect("real managed leaf");
        }
        let row = repository
            .insert_creating_worktree(
                id,
                project,
                leaf.to_string_lossy().into_owned(),
                "0123456789abcdef0123456789abcdef01234567".into(),
            )
            .await
            .expect("creating row");
        let row = if state == ManagedWorktreeState::Ready {
            repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Creating,
                    ManagedWorktreeState::Ready,
                    None,
                )
                .await
                .expect("ready row")
        } else {
            row
        };
        (row, leaf)
    }

    async fn reconciliation_fixture(
        initial_state: ManagedWorktreeState,
        hooks: Option<ReconciliationTestHooks>,
    ) -> (
        tempfile::TempDir,
        String,
        DesktopState,
        Project,
        ManagedWorktree,
        PathBuf,
    ) {
        let directory = tempdir().expect("temporary reconciliation fixture");
        let database_path = directory.path().join("desktop.sqlite");
        let url = format!("sqlite://{}", database_path.display());
        let repository = RunRepository::open(&url).await.expect("open repository");
        let project = repository
            .register_project(reconciliation_registration(directory.path(), "primary"))
            .await
            .expect("project");
        let root = directory.path().join("managed-worktrees");
        let (row, leaf) =
            insert_reconciliation_row(&repository, &root, &project, initial_state, true).await;
        let state = reconciliation_test_state(repository, database_path, root, hooks);
        (directory, url, state, project, row, leaf)
    }

    async fn assert_reconciliation_validation_failure(initial_state: ManagedWorktreeState) {
        let (directory, url, state, project, row, leaf) = reconciliation_fixture(
            initial_state,
            Some(ReconciliationTestHooks {
                fail_leaf_canonicalization: true,
                failure_reached: None,
                resume_failure: None,
            }),
        )
        .await;
        let marker = leaf.join("must-remain");
        std::fs::write(&marker, "unchanged").expect("managed fixture marker");
        let missing_repository = PathBuf::from(&project.repository_root);

        let reconciled = reconcile_project_worktrees_impl(&state)
            .await
            .expect("reconcile");
        assert!(reconciled.iter().any(|worktree| {
            worktree.id == row.id.to_string()
                && worktree.state == ManagedWorktreeState::Failed
                && worktree.error_category.as_deref() == Some("recovery_required")
        }));
        let failed = state
            .repository
            .get_worktree(&row.id)
            .await
            .expect("failed row");
        assert_eq!(failed.state, ManagedWorktreeState::Failed);
        assert_eq!(failed.error_category.as_deref(), Some("recovery_required"));
        assert!(leaf.is_dir());
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker remains"),
            "unchanged"
        );
        assert!(!missing_repository.exists());
        assert!(matches!(
            state.repository.unregister_project(&project.id).await,
            Err(CoreError::ProjectHasWorktrees)
        ));

        drop(state);
        let reopened = RunRepository::open(&url).await.expect("reopen repository");
        let persisted = reopened.get_worktree(&row.id).await.expect("persisted row");
        assert_eq!(persisted.state, ManagedWorktreeState::Failed);
        assert_eq!(
            persisted.error_category.as_deref(),
            Some("recovery_required")
        );
        assert!(matches!(
            reopened.unregister_project(&project.id).await,
            Err(CoreError::ProjectHasWorktrees)
        ));
        drop(directory);
    }

    #[tokio::test]
    async fn creating_reconciliation_validation_failure_is_a_durable_recovery_failure() {
        assert_reconciliation_validation_failure(ManagedWorktreeState::Creating).await;
    }

    #[tokio::test]
    async fn ready_reconciliation_validation_failure_is_a_durable_recovery_failure() {
        assert_reconciliation_validation_failure(ManagedWorktreeState::Ready).await;
    }

    #[tokio::test]
    async fn reconciliation_validation_failure_does_not_overwrite_a_newer_state() {
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let (_directory, _url, state, _project, row, leaf) = reconciliation_fixture(
            ManagedWorktreeState::Ready,
            Some(ReconciliationTestHooks {
                fail_leaf_canonicalization: true,
                failure_reached: Some(reached.clone()),
                resume_failure: Some(resume.clone()),
            }),
        )
        .await;
        let reconciliation_state = state.clone();
        let reconciliation =
            tokio::spawn(
                async move { reconcile_project_worktrees_impl(&reconciliation_state).await },
            );
        reached.notified().await;
        state
            .repository
            .transition_worktree(
                &row.id,
                ManagedWorktreeState::Ready,
                ManagedWorktreeState::Removing,
                None,
            )
            .await
            .expect("newer transition");
        resume.notify_one();
        reconciliation
            .await
            .expect("reconciliation task")
            .expect("reconciliation result");
        assert_eq!(
            state
                .repository
                .get_worktree(&row.id)
                .await
                .expect("row")
                .state,
            ManagedWorktreeState::Removing
        );
        assert!(leaf.is_dir());
    }

    #[tokio::test]
    async fn reconciliation_continues_after_validation_failure_for_an_independent_row() {
        let directory = tempdir().expect("temporary reconciliation fixture");
        let database_path = directory.path().join("desktop.sqlite");
        let url = format!("sqlite://{}", database_path.display());
        let repository = RunRepository::open(&url).await.expect("open repository");
        let root = directory.path().join("managed-worktrees");
        let project_a = repository
            .register_project(reconciliation_registration(directory.path(), "first"))
            .await
            .expect("first project");
        let (first, first_leaf) = insert_reconciliation_row(
            &repository,
            &root,
            &project_a,
            ManagedWorktreeState::Creating,
            true,
        )
        .await;
        let project_b = repository
            .register_project(reconciliation_registration(directory.path(), "second"))
            .await
            .expect("second project");
        let (second, second_leaf) = insert_reconciliation_row(
            &repository,
            &root,
            &project_b,
            ManagedWorktreeState::Ready,
            false,
        )
        .await;
        let pool = sqlx::SqlitePool::connect(&url).await.expect("fixture pool");
        sqlx::query("UPDATE managed_worktrees SET created_at_ms = ? WHERE id = ?")
            .bind(1_i64)
            .bind(first.id.to_string())
            .execute(&pool)
            .await
            .expect("first ordering");
        sqlx::query("UPDATE managed_worktrees SET created_at_ms = ? WHERE id = ?")
            .bind(2_i64)
            .bind(second.id.to_string())
            .execute(&pool)
            .await
            .expect("second ordering");
        drop(pool);
        let state = reconciliation_test_state(
            repository,
            database_path,
            root,
            Some(ReconciliationTestHooks {
                fail_leaf_canonicalization: true,
                failure_reached: None,
                resume_failure: None,
            }),
        );

        reconcile_project_worktrees_impl(&state)
            .await
            .expect("reconcile all rows");
        let first_after = state
            .repository
            .get_worktree(&first.id)
            .await
            .expect("first row");
        let second_after = state
            .repository
            .get_worktree(&second.id)
            .await
            .expect("second row");
        assert_eq!(first_after.state, ManagedWorktreeState::Failed);
        assert_eq!(
            first_after.error_category.as_deref(),
            Some("recovery_required")
        );
        assert_eq!(second_after.state, ManagedWorktreeState::Missing);
        assert!(first_leaf.is_dir());
        assert!(!second_leaf.exists());
    }

    #[tokio::test]
    async fn reconciliation_releases_a_failed_projects_lock_for_a_later_same_project_row() {
        let directory = tempdir().expect("temporary reconciliation fixture");
        let database_path = directory.path().join("desktop.sqlite");
        let url = format!("sqlite://{}", database_path.display());
        let repository = RunRepository::open(&url).await.expect("open repository");
        let root = directory.path().join("managed-worktrees");
        let project = repository
            .register_project(reconciliation_registration(
                directory.path(),
                "same-project",
            ))
            .await
            .expect("project");
        let (first, first_leaf) = insert_reconciliation_row(
            &repository,
            &root,
            &project,
            ManagedWorktreeState::Creating,
            true,
        )
        .await;
        let (second, second_leaf) = insert_reconciliation_row(
            &repository,
            &root,
            &project,
            ManagedWorktreeState::Ready,
            false,
        )
        .await;
        let pool = sqlx::SqlitePool::connect(&url).await.expect("fixture pool");
        sqlx::query("UPDATE managed_worktrees SET created_at_ms = ? WHERE id = ?")
            .bind(1_i64)
            .bind(first.id.to_string())
            .execute(&pool)
            .await
            .expect("first ordering");
        sqlx::query("UPDATE managed_worktrees SET created_at_ms = ? WHERE id = ?")
            .bind(2_i64)
            .bind(second.id.to_string())
            .execute(&pool)
            .await
            .expect("second ordering");
        drop(pool);
        let queued = repository
            .list_reconcilable_worktrees()
            .await
            .expect("ordered rows");
        assert_eq!(
            queued.iter().map(|row| &row.id).collect::<Vec<_>>(),
            vec![&first.id, &second.id]
        );
        assert_eq!(first.project_id, second.project_id);
        assert_eq!(first.project_id, project.id);
        let marker = first_leaf.join("must-remain");
        std::fs::write(&marker, "unchanged").expect("managed fixture marker");
        let unavailable_repository = PathBuf::from(&project.repository_root);
        let state = reconciliation_test_state(
            repository,
            database_path,
            root,
            Some(ReconciliationTestHooks {
                fail_leaf_canonicalization: true,
                failure_reached: None,
                resume_failure: None,
            }),
        );

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reconcile_project_worktrees_impl(&state),
        )
        .await
        .expect("same-project reconciliation did not complete")
        .expect("same-project reconciliation");
        let first_after = state
            .repository
            .get_worktree(&first.id)
            .await
            .expect("first row");
        let second_after = state
            .repository
            .get_worktree(&second.id)
            .await
            .expect("second row");
        assert_eq!(first_after.state, ManagedWorktreeState::Failed);
        assert_eq!(
            first_after.error_category.as_deref(),
            Some("recovery_required")
        );
        assert_eq!(second_after.state, ManagedWorktreeState::Missing);
        assert!(first_leaf.is_dir());
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker remains"),
            "unchanged"
        );
        assert!(!second_leaf.exists());
        assert!(!unavailable_repository.exists());

        acquire_project_worktree_lock(&state, &project.id)
            .await
            .expect("failed project's lock was released");
        release_project_worktree_lock(&state, &project.id).await;
    }

    #[cfg(unix)]
    fn executable(path: &std::path::Path) {
        std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn terminal_events_end_a_forwarder() {
        assert!(terminal_event("run_completed"));
        assert!(terminal_event("run_failed"));
        assert!(terminal_event("run_cancelled"));
        assert!(!terminal_event("message"));
    }
    #[test]
    fn event_dto_preserves_the_persisted_contract() {
        let id = RunId::new();
        let dto = event_dto(NormalizedAgentEvent {
            schema_version: 1,
            run_id: id.clone(),
            sequence_number: 7,
            event_type: "message".into(),
            occurred_at_ms: 42,
            payload: json!({"text":"safe"}),
        });
        assert_eq!(dto.run_id, id.to_string());
        assert_eq!(dto.sequence_number, 7);
        assert_eq!(dto.timestamp_ms, 42);
    }
    fn run(status: RunStatus, error: Option<SafeRunError>) -> Run {
        Run {
            id: RunId::new(),
            task_text: "inspect fixture".into(),
            agent: AgentKind::Fake,
            status,
            schema_version: 1,
            created_at_ms: 10,
            started_at_ms: Some(20),
            finished_at_ms: None,
            exit_code: None,
            error,
        }
    }
    #[test]
    fn run_dto_serializes_running_shape_exactly() {
        let run = run(RunStatus::Running, None);
        assert_eq!(
            serde_json::to_value(run_dto_with_capability(run.clone(), true)).unwrap(),
            json!({"id":run.id.to_string(),"task_text":"inspect fixture","agent_kind":"fake","status":"running","schema_version":1,"created_at_ms":10,"started_at_ms":20,"finished_at_ms":null,"exit_code":null,"error_category":null,"error_message":null,"cancellable":true})
        );
    }
    #[test]
    fn run_dto_flattens_failed_error_without_internal_fields() {
        let mut run = run(
            RunStatus::Failed,
            Some(SafeRunError {
                category: "agent_failed".into(),
                message: "safe failure".into(),
            }),
        );
        run.finished_at_ms = Some(30);
        run.exit_code = Some(1);
        let value = serde_json::to_value(run_dto_with_capability(run.clone(), false)).unwrap();
        assert_eq!(
            value,
            json!({"id":run.id.to_string(),"task_text":"inspect fixture","agent_kind":"fake","status":"failed","schema_version":1,"created_at_ms":10,"started_at_ms":20,"finished_at_ms":30,"exit_code":1,"error_category":"agent_failed","error_message":"safe failure","cancellable":false})
        );
        assert!(value.get("error").is_none());
        assert!(value.get("executable").is_none());
        assert!(value.get("stderr").is_none());
    }
    #[test]
    fn detached_nonterminal_run_uses_the_same_shape_without_capability() {
        let run = run(RunStatus::Running, None);
        let value = serde_json::to_value(run_dto_with_capability(run, false)).unwrap();
        assert_eq!(value.get("status"), Some(&json!("running")));
        assert_eq!(value.get("cancellable"), Some(&json!(false)));
        assert_eq!(value.as_object().map(|object| object.len()), Some(12));
    }
    #[test]
    fn input_errors_are_stable_and_safe() {
        let error = input_error();
        assert_eq!(error.code, "invalid_input");
        assert!(!error.message.contains('/'));
    }
    #[test]
    fn project_dto_has_an_explicit_safe_shape() {
        let project = Project {
            id: ProjectId::new(),
            display_name: "Fixture project".into(),
            repository_identity: "/private/secret/.git".into(),
            repository_fingerprint: "unix:private-device:private-inode".into(),
            fingerprint_scheme: ProjectFingerprintScheme::StrongV1,
            repository_root: "/private/secret".into(),
            primary_root: "/private/secret".into(),
            git_common_dir: "/private/secret/.git".into(),
            branch: Some("main".into()),
            head: Some("abc123".into()),
            validation_state: ProjectValidationState::Valid,
            is_primary_worktree: true,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_validated_at_ms: 3,
        };
        let value = serde_json::to_value(project_dto(project)).expect("serialize project DTO");
        assert_eq!(value["display_name"], "Fixture project");
        assert!(value.get("repository_root").is_none());
        assert!(value.get("git_common_dir").is_none());
        assert!(value.get("repository_fingerprint").is_none());
        assert!(!value.to_string().contains("/private/secret"));
    }
    #[test]
    fn validation_errors_do_not_expose_git_diagnostics() {
        let error = project_error(&GitError::Command(
            "fatal: /private/private-repository token=secret".into(),
        ));
        assert_eq!(error.code, "validation_failed");
        assert!(!error.message.contains("private-repository"));
        assert!(!error.message.contains("secret"));
    }
    #[test]
    fn repository_identity_change_has_a_safe_typed_bridge_error() {
        let error = project_storage_error(CoreError::RepositoryIdentityChanged);
        assert_eq!(error.code, "repository_identity_changed");
        assert!(!error.message.contains('/'));
        assert!(!error.message.contains("inode"));
    }
    #[test]
    fn application_repository_cannot_be_registered() {
        let root = application_repository_root().expect("application repository root");
        let inspection = RepositoryInspection {
            repository_root: root.join("linked-worktree-fixture"),
            primary_root: root,
            common_dir: PathBuf::from("/private/ignored/.git"),
            identity: "fixture".into(),
            fingerprint: RepositoryFingerprint::from_stored("unix:1:2".into()).unwrap(),
            branch: Some("main".into()),
            head: Some("abc123".into()),
            state: RepositoryState::LinkedWorktree,
            is_primary: false,
        };
        let protected = ProtectedRepository {
            identity: "fixture".into(),
            fingerprint: "unix:1:2".into(),
        };
        let error = registration_from_inspection(inspection, None, Some(&protected))
            .expect_err("protected root");
        assert_eq!(error.code, "repository_unsupported");
        assert!(!error.message.contains('/'));
    }
    #[tokio::test]
    async fn missing_project_directory_has_a_safe_validation_result() {
        let missing = tempdir()
            .expect("temporary parent")
            .path()
            .join("missing repository");
        let error = validate_project_registration(missing, Some("Fixture".into()), None)
            .await
            .expect_err("missing path");
        assert_eq!(error.code, "path_not_found");
        assert!(!error.message.contains("missing repository"));
    }
    #[cfg(unix)]
    #[test]
    fn trusted_override_wins_and_returns_a_canonical_path() {
        let directory = tempdir().unwrap();
        let override_path = directory.path().join("agent with 空格");
        executable(&override_path);
        let current_binary = directory.path().join("Agent Sentinel");
        let resolved =
            resolve_fake_agent_path(Some(override_path.clone()), current_binary).unwrap();
        assert_eq!(resolved, override_path.canonicalize().unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn packaged_candidate_requires_an_executable_file() {
        let directory = tempdir().unwrap();
        let current_binary = directory.path().join("Agent Sentinel");
        let binary = bundled_sidecar_candidate(&current_binary).unwrap();
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "not executable").unwrap();
        let error = resolve_fake_agent_path(None, current_binary).unwrap_err();
        assert_eq!(error.code, "fake_agent_unavailable");
        assert!(!error.message.contains(&binary.display().to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn packaged_runtime_candidate_has_no_binaries_prefix() {
        let directory = tempdir().unwrap();
        let current_binary = directory.path().join("Agent Sentinel");
        let binary = bundled_sidecar_candidate(&current_binary).unwrap();
        std::fs::create_dir_all(directory.path()).unwrap();
        executable(&binary);

        assert_eq!(binary, directory.path().join(fake_agent_filename()));
        assert!(!binary
            .components()
            .any(|component| component.as_os_str() == "binaries"));
        assert_eq!(
            resolve_fake_agent_path(None, current_binary).unwrap(),
            binary.canonicalize().unwrap()
        );
    }

    #[test]
    fn external_bin_source_and_runtime_filenames_are_distinct() {
        let target = "aarch64-apple-darwin";
        let source = format!("binaries/sentinel-fake-agent-{target}");
        assert_eq!(source, "binaries/sentinel-fake-agent-aarch64-apple-darwin");
        assert_eq!(fake_agent_filename(), "sentinel-fake-agent");
    }

    #[test]
    fn bundled_sidecar_is_a_sibling_in_macos_and_development_layouts() {
        assert_eq!(
            bundled_sidecar_candidate(std::path::Path::new(
                "/Applications/Agent Sentinel.app/Contents/MacOS/agent-sentinel-desktop"
            ))
            .unwrap(),
            PathBuf::from("/Applications/Agent Sentinel.app/Contents/MacOS")
                .join(fake_agent_filename())
        );
        assert_eq!(
            bundled_sidecar_candidate(std::path::Path::new(
                "/workspace/target/debug/agent-sentinel-desktop"
            ))
            .unwrap(),
            PathBuf::from("/workspace/target/debug").join(fake_agent_filename())
        );
    }

    #[test]
    fn bundled_sidecar_requires_a_binary_parent() {
        let error = bundled_sidecar_candidate(std::path::Path::new("")).unwrap_err();
        assert_eq!(error.code, "fake_agent_unavailable");
    }

    #[tokio::test]
    async fn phase3_two_fake_tasks_are_worktree_isolated() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main-repository");
        std::fs::create_dir(&main).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&main)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Phase Three"]);
        git(&["config", "user.email", "phase3@example.invalid"]);
        std::fs::write(main.join("baseline.txt"), b"baseline\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "baseline"]);
        let base = resolve_exact_head(&main).await.unwrap();
        let task_one = temp.path().join("managed-task-one");
        let task_two = temp.path().join("managed-task-two");
        add_detached_worktree(&main, &task_one, &base)
            .await
            .unwrap();
        add_detached_worktree(&main, &task_two, &base)
            .await
            .unwrap();
        let main_before = std::fs::read(main.join("baseline.txt")).unwrap();
        let one = tokio::spawn({
            let task_one = task_one.clone();
            async move {
                std::fs::write(task_one.join("task-one.txt"), b"fake one\n").unwrap();
                Ok::<(), ()>(())
            }
        });
        let two = tokio::spawn({
            let task_two = task_two.clone();
            async move {
                std::fs::write(task_two.join("task-two.txt"), b"fake two\n").unwrap();
                Err::<(), ()>(())
            }
        });
        assert_eq!(one.await.unwrap(), Ok(()));
        assert_eq!(two.await.unwrap(), Err(()));
        assert_eq!(
            std::fs::read(main.join("baseline.txt")).unwrap(),
            main_before
        );
        assert!(!main.join("task-one.txt").exists() && !main.join("task-two.txt").exists());
        assert!(task_one.join("task-one.txt").exists() && !task_one.join("task-two.txt").exists());
        assert!(task_two.join("task-two.txt").exists() && !task_two.join("task-one.txt").exists());
        // Cross-use of a task's selected filesystem identity is rejected by
        // managed-leaf validation before it can become a worktree authority.
        assert!(validate_managed_leaf(
            temp.path(),
            &ProjectId::new(),
            &WorktreeId::new(),
            &task_one
        )
        .is_err());
    }
}
