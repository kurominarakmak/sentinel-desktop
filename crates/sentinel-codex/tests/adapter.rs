use sentinel_codex::{
    detect_installation, CodexAppServer, CodexError, CodexInstallation, CodexModelDiscovery,
    CodexProgram, CodexTimeouts, CodexTurn,
};
use sentinel_core::{
    v3::{CreateTask, EventKind},
    RunRepository,
};
use sentinel_provider_api::{ProviderError, ProviderModelDiscovery};
use std::time::Duration;
use std::{fs, process::Command, sync::Mutex};
use tempfile::TempDir;

static FAKE_SERVER_ENV: Mutex<()> = Mutex::new(());

fn fake_server_env_lock() -> std::sync::MutexGuard<'static, ()> {
    FAKE_SERVER_ENV
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn url(dir: &TempDir) -> String {
    format!("sqlite://{}", dir.path().join("db.sqlite").display())
}
fn fake() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex-app-server"))
}
async fn fixture() -> (TempDir, RunRepository, sentinel_core::v3::Task) {
    let d = tempfile::tempdir().unwrap();
    let r = RunRepository::open(&url(&d)).await.unwrap();
    let t = r
        .v3()
        .create_task(
            CreateTask {
                project_id: None,
                workflow_id: "default".into(),
                summary: "fixture".into(),
            },
            1,
        )
        .await
        .unwrap();
    (d, r, t)
}

#[tokio::test]
async fn detects_a_supported_explicit_executable_and_missing_file() {
    assert!(matches!(
        detect_installation(fake()).await,
        CodexInstallation::Available { .. }
    ));
    assert!(matches!(
        detect_installation("/definitely/missing/codex").await,
        CodexInstallation::Missing
    ));
    assert!(CodexProgram::from_executable("/definitely/missing/codex").is_err());
}

#[tokio::test]
async fn app_server_model_discovery_preserves_models_and_reasoning_capabilities() {
    let _lock = fake_server_env_lock();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
    let directory = tempfile::tempdir().unwrap();
    let repository = RunRepository::open(&url(&directory)).await.unwrap();
    let discovery = CodexModelDiscovery::new(
        CodexProgram::from_executable(fake()).unwrap(),
        repository,
        directory.path(),
    );
    let catalog = discovery.discover_models().await.unwrap();
    assert_eq!(
        catalog
            .models
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>(),
        ["fixture-primary", "fixture-fast"]
    );
    assert_eq!(
        catalog.models[0].supported_reasoning_efforts,
        ["low", "medium"]
    );
    assert_eq!(
        catalog.models[0].default_reasoning_effort.as_deref(),
        Some("medium")
    );
}

#[tokio::test]
async fn app_server_model_discovery_distinguishes_empty_and_failed_results() {
    let _lock = fake_server_env_lock();
    let directory = tempfile::tempdir().unwrap();
    let repository = RunRepository::open(&url(&directory)).await.unwrap();
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "empty-models");
    let empty = CodexModelDiscovery::new(
        CodexProgram::from_executable(fake()).unwrap(),
        repository.clone(),
        directory.path(),
    );
    assert!(empty.discover_models().await.unwrap().models.is_empty());
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "unsupported-model-list");
    let failed = CodexModelDiscovery::new(
        CodexProgram::from_executable(fake()).unwrap(),
        repository,
        directory.path(),
    );
    assert!(matches!(
        failed.discover_models().await,
        Err(ProviderError::Unavailable(_))
    ));
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test]
async fn refuses_an_executable_replaced_after_configuration() {
    let (directory, repository, task) = fixture().await;
    let executable = directory.path().join("codex-copy");
    fs::copy(fake(), &executable).unwrap();
    let program = CodexProgram::from_executable(&executable).unwrap();
    fs::write(&executable, b"replaced after validation").unwrap();
    assert!(matches!(
        CodexAppServer::start(program, repository, task.id, directory.path()).await,
        Err(CodexError::MissingExecutable)
    ));
}

/// Opt-in local integration check. It never runs in CI and only uses a fresh
/// temporary Git repository; it does not issue account or reset operations.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an explicitly authorized local Codex account"]
async fn real_local_app_server_smoke_uses_only_a_temporary_git_repository() {
    assert_eq!(
        std::env::var("SENTINEL_REAL_CODEX_SMOKE").as_deref(),
        Ok("1")
    );
    let executable = std::env::var_os("SENTINEL_REAL_CODEX_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .expect("set SENTINEL_REAL_CODEX_EXECUTABLE to the local codex binary");
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
                workflow_id: "real-smoke".into(),
                summary: "temporary Codex App Server smoke".into(),
            },
            1,
        )
        .await
        .unwrap();
    let program = CodexProgram::from_executable(executable).unwrap();
    let mut server = CodexAppServer::start(
        program.clone(),
        repository.clone(),
        task.id.clone(),
        directory.path(),
    )
    .await
    .unwrap();
    let session = server.start_thread(directory.path()).await.unwrap();
    let turn = server
        .start_turn(
            &session,
            "For this temporary smoke test, begin waiting without changing any files.",
        )
        .await
        .unwrap();
    // A minimal smoke turn may complete before the interrupt reaches Codex.
    // Both outcomes prove the supported request path without pretending that
    // a completed turn was cancelled in flight.
    assert!(matches!(
        server.interrupt_turn(&session, &turn).await,
        Ok(()) | Err(CodexError::RpcError)
    ));
    server.shutdown().await.unwrap();
    let mut replacement = CodexAppServer::start(
        program,
        repository.clone(),
        task.id.clone(),
        directory.path(),
    )
    .await
    .unwrap();
    let resumed = replacement.resume_thread(&session.thread_id).await.unwrap();
    assert_eq!(resumed.thread_id, session.thread_id);
    assert!(!repository
        .v3()
        .list_events(&task.id)
        .await
        .unwrap()
        .is_empty());
    replacement.shutdown().await.unwrap();
}

/// Opt-in evidence for a provider-confirmed in-flight interrupt. The prompt is
/// deliberately harmless and asks Codex to wait without inspecting or editing
/// the disposable repository.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an explicitly authorized local Codex account"]
async fn real_local_app_server_interrupts_an_in_flight_turn() {
    assert_eq!(
        std::env::var("SENTINEL_REAL_CODEX_SMOKE").as_deref(),
        Ok("1")
    );
    let executable = std::env::var_os("SENTINEL_REAL_CODEX_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .expect("set SENTINEL_REAL_CODEX_EXECUTABLE to the local codex binary");
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
                workflow_id: "real-interrupt".into(),
                summary: "temporary interrupt smoke".into(),
            },
            1,
        )
        .await
        .unwrap();
    let mut server = CodexAppServer::start(
        CodexProgram::from_executable(executable).unwrap(),
        repository.clone(),
        task.id.clone(),
        directory.path(),
    )
    .await
    .unwrap();
    let session = server.start_thread(directory.path()).await.unwrap();
    {
        let start = server.start_turn(&session, "For this temporary smoke test, use your shell tool to run `sleep 30`. Do not inspect or modify files, and do not respond until that command completes; this test will interrupt the turn first.");
        tokio::pin!(start);
        let turn = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    result = &mut start => panic!("long turn completed before it could be interrupted: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {
                        let events = repository.v3().list_events(&task.id).await.unwrap();
                        if let Some(turn_id) = events.iter().find_map(|event| {
                            (event.payload.get("event").and_then(|value| value.as_str()) == Some("turn_started"))
                                .then(|| event.payload.get("turn_id").and_then(|value| value.as_str()))
                                .flatten()
                                .map(str::to_owned)
                        }) {
                            break CodexTurn { turn_id };
                        }
                    }
                }
            }
        })
        .await
        .expect("provider did not announce a long-running turn");
        server.interrupt_turn(&session, &turn).await.unwrap();
        assert_eq!(start.await.unwrap(), turn);
    }
    let events = repository.v3().list_events(&task.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionCancelled)));
    server.shutdown().await.unwrap();
}

/// Opt-in evidence that only the owned child is terminated and a fresh server
/// resumes the one durable provider thread without adding another session.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an explicitly authorized local Codex account"]
async fn real_local_app_server_recovers_after_owned_child_death() {
    assert_eq!(
        std::env::var("SENTINEL_REAL_CODEX_SMOKE").as_deref(),
        Ok("1")
    );
    let executable = std::env::var_os("SENTINEL_REAL_CODEX_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .unwrap();
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
                workflow_id: "real-recovery".into(),
                summary: "temporary death recovery smoke".into(),
            },
            1,
        )
        .await
        .unwrap();
    let program = CodexProgram::from_executable(executable).unwrap();
    let mut server = CodexAppServer::start(
        program.clone(),
        repository.clone(),
        task.id.clone(),
        directory.path(),
    )
    .await
    .unwrap();
    let session = server.start_thread(directory.path()).await.unwrap();
    server
        .start_turn(
            &session,
            "For this temporary smoke test, reply with exactly `ready` and do not inspect or modify files.",
        )
        .await
        .unwrap();
    let pid = server.pid().unwrap();
    assert!(Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    for _ in 0..50 {
        if !server.is_alive() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!server.is_alive());
    let mut replacement = CodexAppServer::start(
        program,
        repository.clone(),
        task.id.clone(),
        directory.path(),
    )
    .await
    .unwrap();
    let resumed = replacement.resume_thread(&session.thread_id).await.unwrap();
    assert_eq!(resumed.session_id, session.session_id);
    let sessions = repository
        .v3()
        .list_sessions_for_task(&task.id)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    let events = repository.v3().list_events(&task.id).await.unwrap();
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence_number < pair[1].sequence_number));
    let _ = server.shutdown().await;
    replacement.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn app_server_starts_threads_turns_interrupts_and_persists_normalized_events() {
    let _guard = fake_server_env_lock();
    let (d, r, t) = fixture().await;
    let p = CodexProgram::from_executable(fake()).unwrap();
    let mut s = CodexAppServer::start(p, r.clone(), t.id.clone(), d.path())
        .await
        .unwrap();
    let thread = s.start_thread(d.path()).await.unwrap();
    let resumed = s.resume_thread(&thread.thread_id).await.unwrap();
    let turn = s.start_turn(&resumed, "safe fixture prompt").await.unwrap();
    s.interrupt_turn(&resumed, &turn).await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionStarted)));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::ToolCompleted)));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::TurnCompleted)));
    assert!(events.iter().any(|event| {
        event.payload.get("turn_id").is_some() && event.payload.get("item_id").is_some()
    }));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionCancelled)));
    assert_eq!(
        r.v3()
            .get_session(&thread.session_id)
            .await
            .unwrap()
            .provider_session_ref,
        "thread-test"
    );
    s.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn implementation_turn_disables_interactive_approval_in_the_owned_worktree() {
    let _guard = fake_server_env_lock();
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "implementation-policy");
    let (d, r, t) = fixture().await;
    let p = CodexProgram::from_executable(fake()).unwrap();
    let mut server = CodexAppServer::start(p, r, t.id, d.path()).await.unwrap();
    let session = server.start_thread(d.path()).await.unwrap();
    server
        .start_turn(&session, "make the owned fixture edit")
        .await
        .unwrap();
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_frame_isolated_from_following_valid_response() {
    let _guard = fake_server_env_lock();
    let (d, r, t, mut server) =
        start_with_scenario("malformed-followed-valid", CodexTimeouts::default()).await;
    server.start_thread(d.path()).await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(&event.kind, EventKind::Unknown { discriminator } if discriminator == "transport/malformed_frame")
    }));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

async fn start_with_scenario(
    scenario: &str,
    timeouts: CodexTimeouts,
) -> (
    TempDir,
    RunRepository,
    sentinel_core::v3::Task,
    CodexAppServer,
) {
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", scenario);
    let (d, r, t) = fixture().await;
    let p = CodexProgram::from_executable(fake()).unwrap();
    let server =
        CodexAppServer::start_with_timeouts(p, r.clone(), t.id.clone(), d.path(), timeouts)
            .await
            .unwrap();
    (d, r, t, server)
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_requests_are_dispatched_by_id_when_responses_are_reversed() {
    let _guard = fake_server_env_lock();
    let (d, _r, _t, mut server) = start_with_scenario("reverse", CodexTimeouts::default()).await;
    let (first, second) =
        tokio::join!(server.start_thread(d.path()), server.start_thread(d.path()));
    assert_eq!(first.unwrap().thread_id, "thread-first");
    assert_eq!(second.unwrap().thread_id, "thread-second");
    assert_eq!(server.pending_request_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn one_timeout_does_not_complete_another_request() {
    let _guard = fake_server_env_lock();
    let timeouts = CodexTimeouts {
        request: Duration::from_millis(50),
        ..CodexTimeouts::default()
    };
    let (d, _r, _t, mut server) = start_with_scenario("timeout-one", timeouts).await;
    assert_eq!(
        server.start_thread(d.path()).await,
        Err(CodexError::Timeout)
    );
    assert_eq!(
        server.start_thread(d.path()).await.unwrap().thread_id,
        "thread-test"
    );
    assert_eq!(server.pending_request_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn initial_rate_limit_read_and_live_update_are_retained() {
    let _guard = fake_server_env_lock();
    let (d, _r, _t, mut server) =
        start_with_scenario("rate-limit-update", CodexTimeouts::default()).await;
    assert_eq!(
        server.latest_rate_limits().unwrap().0["primary"]["usedPercent"],
        42
    );
    let mut updates = server.subscribe_rate_limits();
    server.start_thread(d.path()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), updates.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        server.latest_rate_limits().unwrap().0["primary"]["usedPercent"],
        77
    );
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_versions_payloads_and_provider_methods_remain_safe_diagnostics() {
    let _guard = fake_server_env_lock();
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "unsupported-version");
    let (d, r, t) = fixture().await;
    let p = CodexProgram::from_executable(fake()).unwrap();
    assert!(matches!(
        CodexAppServer::start(p, r, t.id, d.path()).await,
        Err(CodexError::Unsupported)
    ));
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "unsupported-payload");
    let (d, _r, _t, mut server) =
        start_with_scenario("unsupported-payload", CodexTimeouts::default()).await;
    assert_eq!(
        server.start_thread(d.path()).await,
        Err(CodexError::MissingIdentifier)
    );
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    let (d, r, t, mut server) =
        start_with_scenario("unsupported-notification", CodexTimeouts::default()).await;
    server.start_thread(d.path()).await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::Unknown { discriminator } if discriminator == "future/unsupported")));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn protocol_negotiation_accepts_legacy_and_v2_and_rejects_older_and_newer_shapes() {
    let _guard = fake_server_env_lock();
    for unsupported in ["older-version", "newer-version"] {
        std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", unsupported);
        let (d, r, t) = fixture().await;
        let program = CodexProgram::from_executable(fake()).unwrap();
        assert!(matches!(
            CodexAppServer::start(program, r, t.id, d.path()).await,
            Err(CodexError::Unsupported)
        ));
    }
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "protocol-v2");
    let (d, r, t) = fixture().await;
    let program = CodexProgram::from_executable(fake()).unwrap();
    let mut server = CodexAppServer::start(program, r, t.id, d.path())
        .await
        .unwrap();
    server.shutdown().await.unwrap();
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "missing-version");
    let (d, r, t) = fixture().await;
    let program = CodexProgram::from_executable(fake()).unwrap();
    let mut server = CodexAppServer::start(program, r, t.id, d.path())
        .await
        .unwrap();
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn child_exit_fans_out_to_one_and_many_pending_requests() {
    let _guard = fake_server_env_lock();
    let (d, _r, _t, mut server) =
        start_with_scenario("exit-during-request", CodexTimeouts::default()).await;
    assert_eq!(
        server.start_thread(d.path()).await,
        Err(CodexError::UnexpectedExit)
    );
    assert_eq!(server.pending_request_count().await, 0);
    let _ = server.shutdown().await;
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");

    let (d, _r, _t, mut server) =
        start_with_scenario("exit-during-request", CodexTimeouts::default()).await;
    let (one, two) = tokio::join!(server.start_thread(d.path()), server.start_thread(d.path()));
    assert_eq!(one, Err(CodexError::UnexpectedExit));
    assert_eq!(two, Err(CodexError::UnexpectedExit));
    assert_eq!(server.pending_request_count().await, 0);
    let _ = server.shutdown().await;
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_response_is_ignored_after_remove_before_complete() {
    let _guard = fake_server_env_lock();
    let (d, _r, _t, mut server) =
        start_with_scenario("duplicate-response", CodexTimeouts::default()).await;
    assert_eq!(
        server.start_thread(d.path()).await.unwrap().thread_id,
        "thread-test"
    );
    assert_eq!(server.pending_request_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn notification_worker_preserves_provider_order_and_shutdown_reaps_tasks() {
    let _guard = fake_server_env_lock();
    let (d, r, t, mut server) =
        start_with_scenario("notification-order", CodexTimeouts::default()).await;
    let session = server.start_thread(d.path()).await.unwrap();
    server
        .start_turn(&session, "ordered notifications")
        .await
        .unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    let ids: Vec<_> = events
        .iter()
        .filter_map(|event| event.payload.get("item_id").and_then(|id| id.as_str()))
        .collect();
    let first = ids.iter().position(|id| *id == "first").unwrap();
    let second = ids.iter().position(|id| *id == "second").unwrap();
    assert!(first < second);
    server.shutdown().await.unwrap();
    assert_eq!(server.pending_request_count().await, 0);
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn jsonl_frames_survive_split_and_coalesced_writes() {
    let _guard = fake_server_env_lock();
    let (d, _r, _t, mut server) =
        start_with_scenario("split-frame", CodexTimeouts::default()).await;
    assert_eq!(
        server.start_thread(d.path()).await.unwrap().thread_id,
        "thread-test"
    );
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");

    let (d, r, t, mut server) =
        start_with_scenario("multiple-frames", CodexTimeouts::default()).await;
    server.start_thread(d.path()).await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| {
        event
            .payload
            .get("provider_event_id")
            .and_then(|id| id.as_str())
            == Some("coalesced-notification")
            && event
                .payload
                .get("arrival_sequence")
                .and_then(|sequence| sequence.as_u64())
                == Some(1)
    }));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_frame_isolated_from_following_valid_response() {
    let _guard = fake_server_env_lock();
    let (d, r, t, mut server) =
        start_with_scenario("oversized-followed-valid", CodexTimeouts::default()).await;
    server.start_thread(d.path()).await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(&event.kind, EventKind::Unknown { discriminator } if discriminator == "transport/oversized_frame")
    }));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_notifications_are_idempotent_and_order_anomalies_are_diagnostic() {
    let _guard = fake_server_env_lock();
    let (d, r, t, mut server) =
        start_with_scenario("duplicate-notification", CodexTimeouts::default()).await;
    let session = server.start_thread(d.path()).await.unwrap();
    server.start_turn(&session, "dedupe").await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.payload.get("item_id").and_then(|id| id.as_str())
                    == Some("duplicate-item")
            )
            .count(),
        1
    );
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");

    let (d, r, t, mut server) =
        start_with_scenario("late-notification", CodexTimeouts::default()).await;
    let session = server.start_thread(d.path()).await.unwrap();
    server.start_turn(&session, "late").await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| event
        .payload
        .get("transport_diagnostic")
        .and_then(|value| value.as_str())
        == Some("late_notification")));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");

    let (d, r, t, mut server) =
        start_with_scenario("out-of-order-item", CodexTimeouts::default()).await;
    let session = server.start_thread(d.path()).await.unwrap();
    server.start_turn(&session, "out of order").await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    assert!(events.iter().any(|event| event
        .payload
        .get("transport_diagnostic")
        .and_then(|value| value.as_str())
        == Some("out_of_order_item_notification")));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}

#[tokio::test(flavor = "current_thread")]
async fn diagnostic_payloads_are_bounded() {
    let _guard = fake_server_env_lock();
    let (d, r, t, mut server) =
        start_with_scenario("bounded-diagnostic", CodexTimeouts::default()).await;
    let session = server.start_thread(d.path()).await.unwrap();
    server.start_turn(&session, "bounded").await.unwrap();
    let events = r.v3().list_events(&t.id).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.payload.get("item_id").and_then(|id| id.as_str()) == Some("large-item"))
        .unwrap();
    assert!(event
        .payload
        .get("params")
        .and_then(|params| params.get("truncated"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        serde_json::to_vec(&event.payload).unwrap().len() <= sentinel_codex::MAX_DIAGNOSTIC_BYTES
    );
    assert!(event
        .raw_diagnostic_payload
        .as_ref()
        .map(|raw| serde_json::to_vec(raw).unwrap().len() <= sentinel_codex::MAX_DIAGNOSTIC_BYTES)
        .unwrap_or(true));
    assert_eq!(server.pending_request_count().await, 0);
    assert_eq!(server.correlation_buffer_count().await, 0);
    server.shutdown().await.unwrap();
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
}
