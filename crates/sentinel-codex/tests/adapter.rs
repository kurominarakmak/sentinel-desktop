use sentinel_codex::{
    detect_installation, CodexAppServer, CodexError, CodexInstallation, CodexProgram, CodexTimeouts,
};
use sentinel_core::{
    v3::{CreateTask, EventKind},
    RunRepository,
};
use std::sync::Mutex;
use std::time::Duration;
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
