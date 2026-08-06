use sentinel_codex::{
    detect_installation, CodexAppServer, CodexError, CodexInstallation, CodexProgram,
};
use sentinel_core::{
    v3::{CreateTask, EventKind},
    RunRepository,
};
use std::sync::Mutex;
use tempfile::TempDir;

static FAKE_SERVER_ENV: Mutex<()> = Mutex::new(());

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
    let _guard = FAKE_SERVER_ENV.lock().unwrap();
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
        .any(|event| matches!(event.kind, EventKind::Message)));
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
async fn malformed_protocol_fails_closed() {
    let _guard = FAKE_SERVER_ENV.lock().unwrap();
    std::env::set_var("SENTINEL_FAKE_CODEX_SCENARIO", "malformed");
    let (d, r, t) = fixture().await;
    let p = CodexProgram::from_executable(fake()).unwrap();
    let result = CodexAppServer::start(p, r, t.id, d.path()).await;
    std::env::remove_var("SENTINEL_FAKE_CODEX_SCENARIO");
    assert!(matches!(result, Err(CodexError::MalformedMessage)));
}
