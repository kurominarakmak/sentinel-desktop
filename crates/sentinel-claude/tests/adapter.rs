use sentinel_claude::{
    detect_installation, ClaudeInstallation, ClaudeProcess, ClaudeProgram, PROVIDER,
};
use sentinel_core::{
    v3::{
        CreateTask, EventId, EventKind, NormalizedEventEnvelope, SessionLifecycle, TaskLifecycle,
    },
    RunRepository,
};
use std::{fs, path::PathBuf, process::Command, sync::Mutex};
use tempfile::TempDir;

static ENV: Mutex<()> = Mutex::new(());
fn guard() -> std::sync::MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|error| error.into_inner())
}
fn fake() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-claude"))
}
fn url(dir: &TempDir) -> String {
    format!("sqlite://{}", dir.path().join("db.sqlite").display())
}
async fn fixture() -> (TempDir, RunRepository, sentinel_core::v3::Task) {
    let dir = tempfile::tempdir().unwrap();
    let repository = RunRepository::open(&url(&dir)).await.unwrap();
    let task = repository
        .v3()
        .create_task(
            CreateTask {
                project_id: None,
                workflow_id: "claude".into(),
                summary: "fixture".into(),
            },
            1,
        )
        .await
        .unwrap();
    (dir, repository, task)
}
async fn start(
    scenario: &str,
) -> (
    TempDir,
    RunRepository,
    sentinel_core::v3::Task,
    ClaudeProcess,
    sentinel_claude::ClaudeSession,
) {
    let _ = scenario;
    let (dir, repository, task) = fixture().await;
    let (process, session) = ClaudeProcess::start(
        ClaudeProgram::from_executable(fake()).unwrap(),
        repository.clone(),
        task.id.clone(),
        dir.path(),
        "safe fixture prompt",
    )
    .await
    .unwrap();
    (dir, repository, task, process, session)
}

#[tokio::test]
async fn detects_installation_and_missing_program() {
    assert!(matches!(
        detect_installation(fake()).await,
        ClaudeInstallation::Available { .. }
    ));
    assert!(matches!(
        detect_installation("/definitely/missing/claude").await,
        ClaudeInstallation::Missing
    ));
}

#[tokio::test]
async fn refuses_an_executable_replaced_after_configuration() {
    let (directory, repository, task) = fixture().await;
    let executable = directory.path().join("claude-copy");
    fs::copy(fake(), &executable).unwrap();
    let program = ClaudeProgram::from_executable(&executable).unwrap();
    fs::write(&executable, b"replaced after validation").unwrap();
    assert!(matches!(
        ClaudeProcess::start(
            program,
            repository,
            task.id,
            directory.path(),
            "safe prompt"
        )
        .await,
        Err(sentinel_claude::ClaudeError::MissingExecutable)
    ));
}

#[tokio::test]
async fn starts_streams_and_persists_the_claude_session() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "stream");
    let (_dir, repository, task, mut process, session) = start("stream").await;
    process.wait_for_exit().await.unwrap();
    let sessions = repository
        .v3()
        .list_sessions_for_task(&task.id)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider, PROVIDER);
    assert_eq!(sessions[0].id, session.session_id);
    let events = repository.v3().list_events(&task.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::Message)));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::TurnCompleted)));
    assert_eq!(
        repository.v3().get_task(&task.id).await.unwrap().lifecycle,
        TaskLifecycle::Draft,
        "provider completion must not complete the Sentinel task"
    );
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

#[tokio::test]
async fn cancellation_kills_only_the_owned_claude_process() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "cancel");
    let (_dir, repository, task, mut process, _) = start("cancel").await;
    assert!(process.id().is_some());
    process.cancel().await.unwrap();
    let events = repository.v3().list_events(&task.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionCancelled)));
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

#[tokio::test]
async fn resumes_an_existing_persisted_session_without_duplication() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "stream");
    let (dir, repository, task) = fixture().await;
    let program = ClaudeProgram::from_executable(fake()).unwrap();
    let (mut first, session) = ClaudeProcess::start(
        program.clone(),
        repository.clone(),
        task.id.clone(),
        dir.path(),
        "first",
    )
    .await
    .unwrap();
    first.wait_for_exit().await.unwrap();
    let (mut resumed, recovered) = ClaudeProcess::resume(
        program,
        repository.clone(),
        task.id.clone(),
        dir.path(),
        &session.provider_session_id,
        "resume",
    )
    .await
    .unwrap();
    resumed.wait_for_exit().await.unwrap();
    assert_eq!(recovered, session);
    assert_eq!(
        repository
            .v3()
            .list_sessions_for_task(&task.id)
            .await
            .unwrap()
            .len(),
        1
    );
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

#[tokio::test]
async fn malformed_output_is_diagnostic_not_authoritative() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "malformed");
    let (_dir, repository, task, mut process, _) = start("malformed").await;
    process.wait_for_exit().await.unwrap();
    assert!(repository.v3().list_events(&task.id).await.unwrap().iter().any(|event| matches!(&event.kind, EventKind::Unknown { discriminator } if discriminator == "claude/malformed_json")));
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

#[tokio::test]
async fn exit_is_recorded_without_completing_the_sentinel_task() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "exit");
    let (_dir, repository, task, mut process, _) = start("exit").await;
    process.wait_for_exit().await.unwrap();
    assert!(repository
        .v3()
        .list_events(&task.id)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionFailed)));
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

#[tokio::test]
async fn restart_reconciliation_returns_only_sentinel_owned_recovery_sessions() {
    let _guard = guard();
    std::env::set_var("SENTINEL_FAKE_CLAUDE_SCENARIO", "stream");
    let (dir, repository, task, mut process, session) = start("stream").await;
    process.wait_for_exit().await.unwrap();
    let transition = NormalizedEventEnvelope {
        event_id: EventId::new(),
        task_id: task.id.clone(),
        session_id: None,
        provider: PROVIDER.into(),
        kind: EventKind::Unknown {
            discriminator: "test/restart".into(),
        },
        schema_version: 1,
        occurred_at_ms: 2,
        sequence_number: 100,
        causation_id: None,
        correlation_id: None,
        payload: serde_json::json!({}),
        raw_diagnostic_payload: None,
    };
    let task = repository
        .v3()
        .transition_task_with_event(&task, TaskLifecycle::Preparing, &transition, 2)
        .await
        .unwrap();
    repository.v3().restore_unfinished_tasks(3).await.unwrap();
    let sessions = ClaudeProcess::reconcile_persisted_sessions(&repository, &task.id)
        .await
        .unwrap();
    assert_eq!(sessions, vec![session]);
    assert_eq!(
        repository
            .v3()
            .list_sessions_for_task(&task.id)
            .await
            .unwrap()[0]
            .lifecycle,
        SessionLifecycle::RecoveryRequired
    );
    drop(dir);
    std::env::remove_var("SENTINEL_FAKE_CLAUDE_SCENARIO");
}

/// Explicitly opt-in provider validation. It uses a disposable Git repository
/// and never changes Claude configuration, installs a status bridge, or touches
/// the Sentinel checkout.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an explicitly authorized local Claude Code account"]
async fn real_local_claude_stream_json_start_resume_and_restart_reconciliation() {
    assert_eq!(
        std::env::var("SENTINEL_REAL_CLAUDE_SMOKE").as_deref(),
        Ok("1")
    );
    let executable = std::env::var_os("SENTINEL_REAL_CLAUDE_EXECUTABLE")
        .map(PathBuf::from)
        .expect("set SENTINEL_REAL_CLAUDE_EXECUTABLE to the local claude binary");
    assert!(matches!(
        detect_installation(executable.clone()).await,
        ClaudeInstallation::Available { .. }
    ));
    let directory = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(directory.path())
        .status()
        .unwrap()
        .success());
    let repository = RunRepository::open(&url(&directory)).await.unwrap();
    let task = repository
        .v3()
        .create_task(
            CreateTask {
                project_id: None,
                workflow_id: "real-claude".into(),
                summary: "temporary Claude stream-json smoke".into(),
            },
            1,
        )
        .await
        .unwrap();
    let program = ClaudeProgram::from_executable(executable).unwrap();
    let (mut process, session) = ClaudeProcess::start(
        program.clone(),
        repository.clone(),
        task.id.clone(),
        directory.path(),
        "Reply with exactly `sentinel-start-ok`. Do not inspect or modify any files.",
    )
    .await
    .unwrap();
    process.wait_for_exit().await.unwrap();
    let events = repository.v3().list_events(&task.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::Message)));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionCompleted)));
    let (mut resumed, resumed_session) = ClaudeProcess::resume(
        program,
        repository.clone(),
        task.id.clone(),
        directory.path(),
        &session.provider_session_id,
        "Reply with exactly `sentinel-resume-ok`. Do not inspect or modify any files.",
    )
    .await
    .unwrap();
    assert_eq!(resumed_session, session);
    resumed.wait_for_exit().await.unwrap();
    let transition = NormalizedEventEnvelope {
        event_id: EventId::new(),
        task_id: task.id.clone(),
        session_id: None,
        provider: PROVIDER.into(),
        kind: EventKind::Unknown {
            discriminator: "real/restart".into(),
        },
        schema_version: 1,
        occurred_at_ms: 2,
        sequence_number: 10_000,
        causation_id: None,
        correlation_id: None,
        payload: serde_json::json!({}),
        raw_diagnostic_payload: None,
    };
    let task = repository
        .v3()
        .transition_task_with_event(&task, TaskLifecycle::Preparing, &transition, 2)
        .await
        .unwrap();
    repository.v3().restore_unfinished_tasks(3).await.unwrap();
    assert_eq!(
        ClaudeProcess::reconcile_persisted_sessions(&repository, &task.id)
            .await
            .unwrap(),
        vec![session]
    );
}
