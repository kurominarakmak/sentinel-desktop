use sentinel_core::{
    CoreError, NormalizedAgentEvent, Run, RunRepository, RunStatus, SafeRunError, TaskRequest,
};
use sentinel_fake_agent::FakeAgentScenario;
use sentinel_runtime::{
    bounded_redacted_text, CancellationResult, FakeAgentProgram, RunOrchestrator, RunStorage,
    RuntimeError, StorageFuture,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    sync::{broadcast, Mutex as TokioMutex},
    time,
};

static CURRENT_DIRECTORY_LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();

async fn runtime() -> (TempDir, RunRepository, RunOrchestrator) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = format!(
        "sqlite://{}",
        directory.path().join("runtime.sqlite").display()
    );
    let repository = RunRepository::open(&url).await.expect("repository");
    let runtime = RunOrchestrator::new(repository.clone(), fake_agent_program());
    (directory, repository, runtime)
}

fn fake_agent_program() -> FakeAgentProgram {
    FakeAgentProgram::from_executable(fake_agent_executable()).expect("known fake agent executable")
}

fn fake_agent_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(format!(
            "sentinel-fake-agent{}",
            std::env::consts::EXE_SUFFIX
        ))
}

struct FailingStorage {
    inner: RunRepository,
    status: Option<RunStatus>,
    fail_append: bool,
    fail_finish: bool,
    transition_failed: AtomicBool,
    append_failed: AtomicBool,
    finish_failed: AtomicBool,
}

impl RunStorage for FailingStorage {
    fn create_run<'a>(&'a self, request: TaskRequest) -> StorageFuture<'a, Run> {
        Box::pin(self.inner.create_run(request))
    }
    fn get_run<'a>(&'a self, id: &'a sentinel_core::RunId) -> StorageFuture<'a, Run> {
        Box::pin(self.inner.get_run(id))
    }
    fn transition_with_event<'a>(
        &'a self,
        id: &'a sentinel_core::RunId,
        status: RunStatus,
        error: Option<SafeRunError>,
        event: &'a NormalizedAgentEvent,
    ) -> StorageFuture<'a, Run> {
        Box::pin(async move {
            if self.status == Some(status) && !self.transition_failed.swap(true, Ordering::AcqRel) {
                return Err(CoreError::Storage);
            }
            self.inner
                .transition_with_event(id, status, error, event)
                .await
        })
    }
    fn append_event<'a>(&'a self, event: &'a NormalizedAgentEvent) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            if self.fail_append && !self.append_failed.swap(true, Ordering::AcqRel) {
                return Err(CoreError::Storage);
            }
            self.inner.append_event(event).await
        })
    }
    fn finish_run<'a>(
        &'a self,
        id: &'a sentinel_core::RunId,
        status: RunStatus,
        exit_code: Option<i32>,
        error: Option<SafeRunError>,
        event: Option<&'a NormalizedAgentEvent>,
    ) -> StorageFuture<'a, Run> {
        Box::pin(async move {
            if self.fail_finish && !self.finish_failed.swap(true, Ordering::AcqRel) {
                return Err(CoreError::Storage);
            }
            self.inner
                .finish_run(id, status, exit_code, error, event)
                .await
        })
    }
}

async fn fault_runtime(status: RunStatus) -> (TempDir, RunRepository, RunOrchestrator) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = format!(
        "sqlite://{}",
        directory.path().join("runtime.sqlite").display()
    );
    let repository = RunRepository::open(&url).await.expect("repository");
    let storage: Arc<dyn RunStorage> = Arc::new(FailingStorage {
        inner: repository.clone(),
        status: Some(status),
        fail_append: false,
        fail_finish: false,
        transition_failed: AtomicBool::new(false),
        append_failed: AtomicBool::new(false),
        finish_failed: AtomicBool::new(false),
    });
    (
        directory,
        repository,
        RunOrchestrator::with_storage(storage, fake_agent_program()),
    )
}

async fn event_fault_runtime(fail_finish: bool) -> (TempDir, RunRepository, RunOrchestrator) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = format!(
        "sqlite://{}",
        directory.path().join("runtime.sqlite").display()
    );
    let repository = RunRepository::open(&url).await.expect("repository");
    let storage: Arc<dyn RunStorage> = Arc::new(FailingStorage {
        inner: repository.clone(),
        status: None,
        fail_append: true,
        fail_finish,
        transition_failed: AtomicBool::new(false),
        append_failed: AtomicBool::new(false),
        finish_failed: AtomicBool::new(false),
    });
    (
        directory,
        repository,
        RunOrchestrator::with_storage(storage, fake_agent_program()),
    )
}

async fn submit(runtime: &RunOrchestrator, scenario: FakeAgentScenario) -> sentinel_core::Run {
    runtime
        .submit_run(
            TaskRequest {
                task_text: "deterministic fake task".into(),
            },
            scenario,
        )
        .await
        .expect("submit run")
}

async fn observe_status(
    repository: &RunRepository,
    id: &sentinel_core::RunId,
    expected: RunStatus,
) {
    time::timeout(Duration::from_secs(10), async {
        loop {
            if repository.get_run(id).await.expect("run").status == expected {
                return;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected live status");
}

#[tokio::test]
async fn successful_fake_run_persists_ordered_events_and_delivers_live_events() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::Success).await;
    let mut live = runtime
        .subscribe_to_run_events(&run.id)
        .await
        .expect("subscription");
    let completed = runtime.wait_for_run(&run.id).await.expect("completed run");
    assert_eq!(completed.status, RunStatus::Completed);
    assert_eq!(completed.exit_code, Some(0));
    let events = repository
        .list_events(&run.id)
        .await
        .expect("persisted events");
    assert!(events.len() >= 10);
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence_number < pair[1].sequence_number));
    let first_live = time::timeout(Duration::from_secs(2), live.recv())
        .await
        .expect("live event")
        .expect("event");
    assert_eq!(first_live.run_id, run.id);
}

#[tokio::test]
async fn failure_and_partial_output_are_persisted_as_failed_runs() {
    let (_directory, repository, runtime) = runtime().await;
    let failed = submit(&runtime, FakeAgentScenario::Failure).await;
    let failed_run = runtime.wait_for_run(&failed.id).await.expect("failed run");
    assert_eq!(failed_run.status, RunStatus::Failed);
    assert_eq!(failed_run.exit_code, Some(42));
    assert!(failed_run.error.is_some());
    let partial = submit(&runtime, FakeAgentScenario::Partial).await;
    assert_eq!(
        runtime
            .wait_for_run(&partial.id)
            .await
            .expect("partial run")
            .status,
        RunStatus::Failed
    );
    assert!(repository
        .list_events(&partial.id)
        .await
        .expect("partial events")
        .iter()
        .any(|event| event.event_type == "message"));
}

#[tokio::test]
async fn agent_failure_event_with_zero_exit_resolves_as_failed_without_completion_event() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::FailureExitZero).await;
    let terminal = runtime.wait_for_run(&run.id).await.expect("failed run");
    assert_eq!(terminal.status, RunStatus::Failed);
    assert_eq!(terminal.exit_code, Some(0));
    let error = terminal.error.expect("agent failure detail");
    assert_eq!(error.category, "agent_failed");
    assert!(!error.message.contains("not-a-real-secret"));
    let events = repository.list_events(&run.id).await.expect("events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "agent_failed"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_failed")
            .count(),
        1
    );
    assert!(events
        .iter()
        .all(|event| event.event_type != "run_completed"));
}

#[tokio::test]
async fn delayed_run_exposes_running_before_completion() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::Delayed).await;
    observe_status(&repository, &run.id, RunStatus::Running).await;
    assert_eq!(
        runtime
            .wait_for_run(&run.id)
            .await
            .expect("completion")
            .status,
        RunStatus::Completed
    );
}

#[tokio::test]
async fn malformed_output_is_not_persisted_as_agent_event_and_fails_safely() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::Malformed).await;
    assert_eq!(
        runtime.wait_for_run(&run.id).await.expect("failure").status,
        RunStatus::Failed
    );
    let events = repository.list_events(&run.id).await.expect("events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "parser_error"));
    assert!(!events
        .iter()
        .any(|event| event.payload.to_string().contains("not-json")));
}

#[tokio::test]
async fn cancellation_is_idempotent_and_terminal() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::CancellationChild).await;
    observe_status(&repository, &run.id, RunStatus::Running).await;
    assert_eq!(
        runtime.cancel_run(&run.id).await,
        CancellationResult::CancellationRequested
    );
    assert_eq!(
        runtime.cancel_run(&run.id).await,
        CancellationResult::AlreadyCancelling
    );
    let cancelled = runtime.wait_for_run(&run.id).await.expect("cancelled run");
    assert_eq!(cancelled.status, RunStatus::Cancelled);
    assert_ne!(cancelled.exit_code, Some(0));
    assert!(repository
        .list_events(&run.id)
        .await
        .expect("events")
        .iter()
        .all(|event| event.event_type != "run_completed"));
}

#[tokio::test]
async fn cancellation_before_process_start_and_exit_race_have_one_terminal_result() {
    let (_directory, repository, runtime) = runtime().await;
    let before_start = submit(&runtime, FakeAgentScenario::Delayed).await;
    let _ = runtime.cancel_run(&before_start.id).await;
    let result = runtime
        .wait_for_run(&before_start.id)
        .await
        .expect("terminal run");
    assert!(matches!(
        result.status,
        RunStatus::Cancelled | RunStatus::Completed
    ));
    let race = submit(&runtime, FakeAgentScenario::Success).await;
    let _ = runtime.cancel_run(&race.id).await;
    let terminal = runtime
        .wait_for_run(&race.id)
        .await
        .expect("terminal race run");
    assert!(terminal.status.terminal());
    assert_eq!(
        repository
            .get_run(&race.id)
            .await
            .expect("persisted terminal")
            .status,
        terminal.status
    );
}

#[tokio::test]
async fn lagged_subscriber_does_not_interrupt_persistence() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::Burst).await;
    let mut subscriber = runtime
        .subscribe_to_run_events(&run.id)
        .await
        .expect("subscription");
    assert_eq!(
        runtime
            .wait_for_run(&run.id)
            .await
            .expect("completion")
            .status,
        RunStatus::Completed
    );
    assert!(matches!(
        subscriber.recv().await,
        Err(broadcast::error::RecvError::Lagged(_))
            | Err(broadcast::error::RecvError::Closed)
            | Ok(_)
    ));
    assert!(
        repository
            .list_events(&run.id)
            .await
            .expect("persisted events")
            .len()
            >= 104
    );
}

#[tokio::test]
async fn oversized_event_fails_without_false_completion_and_stderr_is_redacted() {
    let (_directory, repository, runtime) = runtime().await;
    let oversized = submit(&runtime, FakeAgentScenario::OversizedEvent).await;
    assert_eq!(
        runtime.wait_for_run(&oversized.id).await,
        Err(RuntimeError::Storage)
    );
    let stderr = submit(&runtime, FakeAgentScenario::StderrSecrets).await;
    let error = runtime
        .wait_for_run(&stderr.id)
        .await
        .expect("failure")
        .error
        .expect("safe error");
    assert!(
        !error.message.contains("hunter2")
            && !error.message.contains("bearer-token")
            && !error.message.contains("sk-live-secret")
    );
    assert!(
        repository
            .list_events(&stderr.id)
            .await
            .expect("events")
            .len()
            >= 3
    );
}

#[tokio::test]
async fn explicit_executable_path_with_spaces_and_non_ascii_is_used_without_cwd_dependency() {
    let _cwd_guard = CURRENT_DIRECTORY_LOCK
        .get_or_init(|| TokioMutex::new(()))
        .lock()
        .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let executable = directory
        .path()
        .join("fake agent ไทย")
        .join(format!("runner{}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(executable.parent().expect("parent")).expect("program directory");
    std::fs::copy(fake_agent_executable(), &executable).expect("copy fake executable");
    let program = FakeAgentProgram::from_executable(&executable).expect("explicit program");
    let previous = std::env::current_dir().expect("current directory");
    std::env::set_current_dir(directory.path()).expect("unrelated cwd");
    let url = format!(
        "sqlite://{}",
        directory.path().join("runtime.sqlite").display()
    );
    let repository = RunRepository::open(&url).await.expect("repository");
    let runtime = RunOrchestrator::new(repository, program);
    let run = submit(&runtime, FakeAgentScenario::Success).await;
    let result = runtime
        .wait_for_run(&run.id)
        .await
        .expect("successful explicit executable");
    std::env::set_current_dir(previous).expect("restore cwd");
    assert_eq!(result.status, RunStatus::Completed);
}

#[test]
fn missing_executable_is_a_typed_launch_error_and_cargo_is_not_implicit() {
    let missing = PathBuf::from("definitely-missing-fake-agent");
    assert_eq!(
        FakeAgentProgram::from_executable(missing),
        Err(RuntimeError::LaunchTargetMissing)
    );
}

#[tokio::test]
async fn transition_persistence_failures_complete_waiters_and_remove_active_runs() {
    for status in [RunStatus::Preparing, RunStatus::Running] {
        let (_directory, repository, runtime) = fault_runtime(status).await;
        let run = submit(&runtime, FakeAgentScenario::CancellationChild).await;
        let first = time::timeout(Duration::from_secs(2), runtime.wait_for_run(&run.id))
            .await
            .expect("bounded waiter");
        assert_eq!(first, Err(RuntimeError::Storage));
        let second = runtime.wait_for_run(&run.id).await;
        assert!(matches!(
            second,
            Err(RuntimeError::RunNotActive)
                | Ok(Run {
                    status: RunStatus::Failed,
                    ..
                })
        ));
        assert_eq!(
            runtime
                .get_active_run(&run.id)
                .await
                .expect("active lookup"),
            None
        );
        assert!(matches!(
            runtime.cancel_run(&run.id).await,
            CancellationResult::RunNotActive | CancellationResult::AlreadyTerminal
        ));
        let persisted = repository.get_run(&run.id).await.expect("persisted run");
        assert_ne!(persisted.status, RunStatus::Completed);
    }
}

#[tokio::test]
async fn event_persistence_failure_cancels_owned_process_and_never_strands_waiters() {
    for fail_finish in [false, true] {
        let (_directory, repository, runtime) = event_fault_runtime(fail_finish).await;
        let run = submit(&runtime, FakeAgentScenario::CancellationChild).await;
        let result = time::timeout(Duration::from_secs(2), runtime.wait_for_run(&run.id))
            .await
            .expect("bounded failure waiter");
        assert_eq!(result, Err(RuntimeError::Storage));
        assert_eq!(
            runtime
                .get_active_run(&run.id)
                .await
                .expect("active lookup"),
            None
        );
        assert!(matches!(
            runtime.cancel_run(&run.id).await,
            CancellationResult::RunNotActive | CancellationResult::AlreadyTerminal
        ));
        assert!(repository
            .list_events(&run.id)
            .await
            .expect("events")
            .iter()
            .all(|event| event.event_type != "run_completed"));
        assert!(matches!(
            runtime.wait_for_run(&run.id).await,
            Err(RuntimeError::RunNotActive)
                | Ok(Run {
                    status: RunStatus::Failed,
                    ..
                })
        ));
    }
}

#[tokio::test]
async fn lingering_descendant_output_pipes_are_terminated_without_stranding_runs() {
    for scenario in [
        FakeAgentScenario::LingeringStdoutChild,
        FakeAgentScenario::LingeringStderrChild,
    ] {
        let (_directory, repository, runtime) = runtime().await;
        let run = submit(&runtime, scenario).await;
        let completed = time::timeout(Duration::from_secs(2), runtime.wait_for_run(&run.id))
            .await
            .expect("bounded descendant cleanup")
            .expect("persisted abnormal lifecycle failure");
        assert_eq!(completed.status, RunStatus::Failed);
        assert_eq!(completed.exit_code, Some(0));
        assert_eq!(
            completed.error.expect("lifecycle error").category,
            "lingering_process_group"
        );
        assert_eq!(
            runtime
                .get_active_run(&run.id)
                .await
                .expect("active lookup"),
            None
        );
        assert!(repository
            .list_events(&run.id)
            .await
            .expect("events")
            .iter()
            .all(|event| event.event_type != "run_completed"));
    }
}

#[tokio::test]
async fn cancellation_during_lingering_descendant_cleanup_is_idempotent() {
    let (_directory, repository, runtime) = runtime().await;
    let run = submit(&runtime, FakeAgentScenario::LingeringStdoutChild).await;
    observe_status(&repository, &run.id, RunStatus::Running).await;
    let first = runtime.cancel_run(&run.id).await;
    let second = runtime.cancel_run(&run.id).await;
    assert!(matches!(
        first,
        CancellationResult::CancellationRequested
            | CancellationResult::AlreadyCancelling
            | CancellationResult::RunNotActive
            | CancellationResult::AlreadyTerminal
    ));
    assert!(matches!(
        second,
        CancellationResult::AlreadyCancelling
            | CancellationResult::RunNotActive
            | CancellationResult::AlreadyTerminal
    ));
    let completed = time::timeout(Duration::from_secs(2), runtime.wait_for_run(&run.id))
        .await
        .expect("bounded completion")
        .expect("persisted terminal result");
    assert!(matches!(
        completed.status,
        RunStatus::Failed | RunStatus::Cancelled
    ));
    assert_eq!(
        runtime
            .get_active_run(&run.id)
            .await
            .expect("active lookup"),
        None
    );
}

#[test]
fn bounded_redaction_is_utf8_safe_and_respects_byte_limits() {
    let cases = [
        ("abcd", 4),
        ("abcdef", 4),
        ("ภาษาไทยภาษาไทย", 10),
        ("日本語日本語", 8),
        ("😀😀😀", 7),
        ("ascii-日本😀", 9),
        ("", 1),
        ("e\u{301}e\u{301}", 4),
    ];
    for (input, limit) in cases {
        let output = bounded_redacted_text(input, limit);
        assert!(output.len() <= limit, "{input:?} exceeded {limit}");
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
    }
    assert_eq!(bounded_redacted_text("abcdef", 1), "a");
    assert_eq!(bounded_redacted_text("abcdef", 0), "");
    assert_eq!(bounded_redacted_text("abcd", 4), "abcd");
    let secret = bounded_redacted_text("prefix token=secret-value suffix", 16);
    assert!(!secret.contains("secret-value"));
    assert!(secret.len() <= 16);
    let runtime_stderr =
        bounded_redacted_text(&("😀".repeat(2_000) + " password=secret-value"), 4_096);
    assert!(runtime_stderr.len() <= 4_096);
    assert!(!runtime_stderr.contains("secret-value"));
}
