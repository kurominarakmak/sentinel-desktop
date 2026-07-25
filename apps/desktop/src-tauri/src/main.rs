mod windowing;

use sentinel_agent_api::{detect_installation, AgentKind, InstallationStatus};
use sentinel_core::{NormalizedAgentEvent, Run, RunId, RunRepository, TaskRequest};
use sentinel_fake_agent::FakeAgentScenario;
use sentinel_runtime::{CancellationResult, FakeAgentProgram, RunOrchestrator};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{collections::HashSet, path::PathBuf, str::FromStr, sync::Arc};
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
    use sentinel_core::{RunStatus, SafeRunError};
    use serde_json::json;
    use tempfile::tempdir;

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
