mod windowing;

use sentinel_agent_api::{detect_installation, AgentKind, InstallationStatus};
use sentinel_core::{
    CoreError, ManagedWorktree, ManagedWorktreeState, NormalizedAgentEvent, Project,
    ProjectFingerprintScheme, ProjectId, ProjectRegistration, ProjectValidationState, Run, RunId,
    RunRepository, TaskRequest, WorktreeId,
};
use sentinel_fake_agent::FakeAgentScenario;
use sentinel_git::{
    add_detached_worktree, inspect_repository, inspect_worktree_destination_no_follow,
    remove_detached_worktree, resolve_exact_head, worktree_is_clean, worktree_metadata_lookup,
    GitError, RepositoryInspection, RepositoryState, WorktreeDestinationState,
    WorktreeMetadataLookup,
};
use sentinel_runtime::{CancellationResult, FakeAgentProgram, RunOrchestrator};
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
#[derive(Clone)]
struct DesktopState {
    repository: RunRepository,
    orchestrator: RunOrchestrator,
    database_path: PathBuf,
    forwarders: Arc<tokio::sync::Mutex<HashSet<RunId>>>,
    protected_application_repository: Option<ProtectedRepository>,
    worktree_root: PathBuf,
    worktree_projects: Arc<tokio::sync::Mutex<HashSet<ProjectId>>>,
    #[cfg(test)]
    reconciliation_test_hooks: Option<ReconciliationTestHooks>,
}
#[cfg(test)]
#[derive(Clone)]
struct ReconciliationTestHooks {
    fail_leaf_canonicalization: bool,
    failure_reached: Option<Arc<tokio::sync::Notify>>,
    resume_failure: Option<Arc<tokio::sync::Notify>>,
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
        // Validate before each path-following operation and again immediately
        // before the mutating Git invocation.
        let leaf = validate_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path)?;
        if !worktree_is_clean(&leaf)
            .await
            .map_err(|error| project_error(&error))?
        {
            return state
                .repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Removing,
                    ManagedWorktreeState::RetainedDirty,
                    Some("worktree_dirty"),
                )
                .await
                .map(worktree_dto)
                .map_err(worktree_error);
        }
        let leaf = validate_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path)?;
        match remove_detached_worktree(Path::new(&project.repository_root), &leaf).await {
            Ok(()) => {
                let metadata =
                    worktree_metadata_lookup(Path::new(&project.repository_root), &path).await;
                let next = post_remove_state(
                    inspect_managed_leaf(&state.worktree_root, &row.project_id, &row.id, &path),
                    metadata,
                );
                let error = (next != ManagedWorktreeState::Removed).then_some("recovery_required");
                state
                    .repository
                    .transition_worktree(&row.id, ManagedWorktreeState::Removing, next, error)
                    .await
                    .map(worktree_dto)
                    .map_err(worktree_error)
            }
            Err(GitError::DirtyWorktree(_)) => state
                .repository
                .transition_worktree(
                    &row.id,
                    ManagedWorktreeState::Removing,
                    ManagedWorktreeState::RetainedDirty,
                    Some("worktree_dirty"),
                )
                .await
                .map(worktree_dto)
                .map_err(worktree_error),
            Err(error) => {
                let _ = state
                    .repository
                    .transition_worktree(
                        &row.id,
                        ManagedWorktreeState::Removing,
                        ManagedWorktreeState::Failed,
                        Some("worktree_remove_failed"),
                    )
                    .await;
                Err(project_error(&error))
            }
        }
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
            app.manage(DesktopState {
                orchestrator: RunOrchestrator::new(repository.clone(), fake_agent),
                repository,
                database_path,
                forwarders: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
                protected_application_repository,
                worktree_root,
                worktree_projects: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
                #[cfg(test)]
                reconciliation_test_hooks: None,
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
            submit_fake_run,
            list_runs,
            get_run,
            list_run_events,
            cancel_run,
            get_runtime_environment,
            register_project,
            list_projects,
            get_project,
            revalidate_project,
            unregister_project,
            create_project_worktree,
            list_project_worktrees,
            remove_project_worktree,
            reconcile_project_worktrees,
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
    use serde_json::json;
    use tempfile::tempdir;

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
            repository,
            database_path,
            forwarders: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            protected_application_repository: None,
            worktree_root,
            worktree_projects: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            reconciliation_test_hooks: hooks,
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
}
