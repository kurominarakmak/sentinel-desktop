use sentinel_agent_api::AgentEvent;
use sentinel_core::{
    redact, CoreError, NormalizedAgentEvent, Run, RunId, RunRepository, RunStatus, SafeRunError,
    TaskRequest, EVENT_SCHEMA_VERSION,
};
use sentinel_fake_agent::FakeAgentScenario;
use sentinel_process::{ProcessEvent, SupervisedProcess};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{broadcast, mpsc, watch, Mutex},
    time,
};

const LIVE_EVENT_CAPACITY: usize = 64;
const STDERR_LIMIT: usize = 4_096;
const CANCEL_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeAgentProgram {
    executable: PathBuf,
    prefix_args: Vec<String>,
}

impl FakeAgentProgram {
    /// Configures one known local fake-agent executable; no PATH lookup occurs.
    pub fn from_executable(executable: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        let executable = executable.into();
        if !executable.is_file() {
            return Err(RuntimeError::LaunchTargetMissing);
        }
        Ok(Self {
            executable,
            prefix_args: vec!["--scenario".into()],
        })
    }

    /// Explicit developer-only helper. Production callers must pass a bundled executable path.
    pub fn developer_cargo_workspace(cargo: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let executable = cargo.as_ref().to_path_buf();
        if !executable.is_file() {
            return Err(RuntimeError::LaunchTargetMissing);
        }
        Ok(Self {
            executable,
            prefix_args: vec![
                "run".into(),
                "--quiet".into(),
                "-p".into(),
                "sentinel-fake-agent".into(),
                "--".into(),
                "--scenario".into(),
            ],
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancellationResult {
    CancellationRequested,
    AlreadyTerminal,
    AlreadyCancelling,
    RunNotActive,
    TerminationFailed,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("run is not active")]
    RunNotActive,
    #[error("runtime storage error")]
    Storage,
    #[error("configured fake-agent executable is unavailable")]
    LaunchTargetMissing,
    #[error("runtime launch error")]
    Launch,
    #[error("runtime process termination error")]
    Termination,
}

pub trait RunStorage: Send + Sync {
    fn create_run<'a>(&'a self, request: TaskRequest) -> StorageFuture<'a, Run>;
    fn get_run<'a>(&'a self, id: &'a RunId) -> StorageFuture<'a, Run>;
    fn transition_with_event<'a>(
        &'a self,
        id: &'a RunId,
        status: RunStatus,
        error: Option<SafeRunError>,
        event: &'a NormalizedAgentEvent,
    ) -> StorageFuture<'a, Run>;
    fn append_event<'a>(&'a self, event: &'a NormalizedAgentEvent) -> StorageFuture<'a, ()>;
    fn finish_run<'a>(
        &'a self,
        id: &'a RunId,
        status: RunStatus,
        exit_code: Option<i32>,
        error: Option<SafeRunError>,
        event: Option<&'a NormalizedAgentEvent>,
    ) -> StorageFuture<'a, Run>;
}

pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

impl RunStorage for RunRepository {
    fn create_run<'a>(&'a self, request: TaskRequest) -> StorageFuture<'a, Run> {
        Box::pin(RunRepository::create_run(self, request))
    }
    fn get_run<'a>(&'a self, id: &'a RunId) -> StorageFuture<'a, Run> {
        Box::pin(RunRepository::get_run(self, id))
    }
    fn transition_with_event<'a>(
        &'a self,
        id: &'a RunId,
        status: RunStatus,
        error: Option<SafeRunError>,
        event: &'a NormalizedAgentEvent,
    ) -> StorageFuture<'a, Run> {
        Box::pin(RunRepository::transition_with_event(
            self, id, status, error, event,
        ))
    }
    fn append_event<'a>(&'a self, event: &'a NormalizedAgentEvent) -> StorageFuture<'a, ()> {
        Box::pin(RunRepository::append_event(self, event))
    }
    fn finish_run<'a>(
        &'a self,
        id: &'a RunId,
        status: RunStatus,
        exit_code: Option<i32>,
        error: Option<SafeRunError>,
        event: Option<&'a NormalizedAgentEvent>,
    ) -> StorageFuture<'a, Run> {
        Box::pin(RunRepository::finish_run(
            self, id, status, exit_code, error, event,
        ))
    }
}

#[derive(Clone)]
struct ActiveRun {
    cancel: mpsc::Sender<RunnerCommand>,
    events: broadcast::Sender<NormalizedAgentEvent>,
    completion: watch::Sender<Option<Result<Run, RuntimeError>>>,
    cancellation_requested: Arc<AtomicBool>,
}

enum RunnerCommand {
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalRaceWinner {
    AgentFailure,
    Cancellation,
    ParentExit,
}

#[derive(Default)]
struct RunOutcomeEvidence {
    agent_failure: Option<SafeRunError>,
    process_exit_code: Option<i32>,
    parent_exit_observed: bool,
    race_winner: Option<TerminalRaceWinner>,
    fatal_runtime_error: Option<SafeRunError>,
    storage_error: Option<SafeRunError>,
    termination_error: Option<SafeRunError>,
    lingering_process_group: bool,
    malformed_protocol: bool,
    stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalOutcome {
    status: RunStatus,
    exit_code: Option<i32>,
    error: Option<SafeRunError>,
    lifecycle_event_type: &'static str,
    completion_error: Option<RuntimeError>,
}

#[derive(Clone)]
pub struct RunOrchestrator {
    repository: Arc<dyn RunStorage>,
    fake_agent: FakeAgentProgram,
    active: Arc<Mutex<HashMap<RunId, ActiveRun>>>,
}

impl RunOrchestrator {
    pub fn new(repository: RunRepository, fake_agent: FakeAgentProgram) -> Self {
        Self::with_storage(Arc::new(repository), fake_agent)
    }

    pub fn with_storage(repository: Arc<dyn RunStorage>, fake_agent: FakeAgentProgram) -> Self {
        Self {
            repository,
            fake_agent,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn submit_run(
        &self,
        request: TaskRequest,
        scenario: FakeAgentScenario,
    ) -> Result<Run, RuntimeError> {
        let run = self
            .repository
            .create_run(request)
            .await
            .map_err(|_| RuntimeError::Storage)?;
        let (cancel_sender, cancel_receiver) = mpsc::channel(1);
        let (event_sender, _) = broadcast::channel(LIVE_EVENT_CAPACITY);
        let (completion_sender, _) = watch::channel(None);
        let active = ActiveRun {
            cancel: cancel_sender,
            events: event_sender,
            completion: completion_sender,
            cancellation_requested: Arc::new(AtomicBool::new(false)),
        };
        self.active
            .lock()
            .await
            .insert(run.id.clone(), active.clone());
        let runner = self.clone();
        let id = run.id.clone();
        tokio::spawn(async move {
            runner.run(id, scenario, active, cancel_receiver).await;
        });
        Ok(run)
    }

    pub async fn get_active_run(&self, id: &RunId) -> Result<Option<Run>, RuntimeError> {
        if self.active.lock().await.contains_key(id) {
            self.repository
                .get_run(id)
                .await
                .map(Some)
                .map_err(|_| RuntimeError::Storage)
        } else {
            Ok(None)
        }
    }

    pub async fn subscribe_to_run_events(
        &self,
        id: &RunId,
    ) -> Result<broadcast::Receiver<NormalizedAgentEvent>, RuntimeError> {
        self.active
            .lock()
            .await
            .get(id)
            .map(|active| active.events.subscribe())
            .ok_or(RuntimeError::RunNotActive)
    }

    pub async fn cancel_run(&self, id: &RunId) -> CancellationResult {
        let active = self.active.lock().await.get(id).cloned();
        let Some(active) = active else {
            return match self.repository.get_run(id).await {
                Ok(run) if run.status.terminal() => CancellationResult::AlreadyTerminal,
                _ => CancellationResult::RunNotActive,
            };
        };
        if active.cancellation_requested.swap(true, Ordering::AcqRel) {
            return CancellationResult::AlreadyCancelling;
        }
        match active.cancel.try_send(RunnerCommand::Cancel) {
            Ok(()) => CancellationResult::CancellationRequested,
            Err(mpsc::error::TrySendError::Full(_)) => CancellationResult::AlreadyCancelling,
            Err(mpsc::error::TrySendError::Closed(_)) => CancellationResult::TerminationFailed,
        }
    }

    pub async fn wait_for_run(&self, id: &RunId) -> Result<Run, RuntimeError> {
        let completion = self
            .active
            .lock()
            .await
            .get(id)
            .map(|active| active.completion.subscribe());
        let Some(mut completion) = completion else {
            return match self.repository.get_run(id).await {
                Ok(run) if run.status.terminal() => Ok(run),
                _ => Err(RuntimeError::RunNotActive),
            };
        };
        loop {
            if let Some(result) = completion.borrow().clone() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| RuntimeError::RunNotActive)?;
        }
    }

    async fn run(
        &self,
        id: RunId,
        scenario: FakeAgentScenario,
        active: ActiveRun,
        mut commands: mpsc::Receiver<RunnerCommand>,
    ) {
        let mut sequence = 0;
        let mut evidence = RunOutcomeEvidence::default();
        let preparing = self.lifecycle_event(&id, &mut sequence, "run_preparing");
        if self
            .persist_and_publish(&id, RunStatus::Preparing, None, preparing, &active)
            .await
            .is_err()
        {
            evidence.storage_error = Some(runtime_error(
                "runtime_storage",
                "unable to persist run lifecycle update",
            ));
            self.finalize(&id, &mut sequence, evidence, false, &active)
                .await;
            return;
        }
        if commands.try_recv().is_ok() {
            evidence.race_winner = Some(TerminalRaceWinner::Cancellation);
            self.finalize(&id, &mut sequence, evidence, true, &active)
                .await;
            return;
        }

        let mut arguments = self.fake_agent.prefix_args.clone();
        arguments.push(scenario.as_str().into());
        let references: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let started =
            SupervisedProcess::start(&self.fake_agent.executable.to_string_lossy(), &references)
                .await;
        let (mut process, mut output) = match started {
            Ok(value) => value,
            Err(_) => {
                evidence.fatal_runtime_error = Some(runtime_error(
                    "fake_agent_start",
                    "unable to start fake agent",
                ));
                self.finalize(&id, &mut sequence, evidence, true, &active)
                    .await;
                return;
            }
        };
        let running = self.lifecycle_event(&id, &mut sequence, "run_running");
        if self
            .persist_and_publish(&id, RunStatus::Running, None, running, &active)
            .await
            .is_err()
        {
            evidence.storage_error = Some(runtime_error(
                "runtime_storage",
                "unable to persist run lifecycle update",
            ));
            if process.cancel(CANCEL_TIMEOUT).await.is_err() {
                evidence.termination_error = Some(runtime_error(
                    "termination_failed",
                    "fake-agent process cleanup failed",
                ));
            }
            self.finalize(&id, &mut sequence, evidence, true, &active)
                .await;
            return;
        }

        let mut output_open = true;
        'execution: loop {
            tokio::select! {
                command = commands.recv() => if command.is_some() {
                    let cancelling = self.lifecycle_event(&id, &mut sequence, "run_cancelling");
                    if self.persist_and_publish(&id, RunStatus::Cancelling, None, cancelling, &active).await.is_err() {
                        evidence.storage_error = Some(runtime_error(
                            "runtime_storage",
                            "unable to persist run lifecycle update",
                        ));
                    } else {
                        evidence.race_winner.get_or_insert(TerminalRaceWinner::Cancellation);
                    }
                    match process.cancel(CANCEL_TIMEOUT).await {
                        Ok(code) => evidence.process_exit_code = code,
                        Err(_) => evidence.termination_error = Some(runtime_error(
                            "termination_failed",
                            "fake-agent process cleanup failed",
                        )),
                    }
                    break 'execution;
                },
                event = output.recv(), if output_open => match event {
                Some(ProcessEvent::Stdout(line)) => match serde_json::from_str::<AgentEvent>(&line) {
                        Ok(agent_event) => {
                            if self.record_agent_event(&id, &mut sequence, agent_event, &mut evidence, &active).await.is_err() {
                                self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                                break 'execution;
                            }
                        }
                        Err(_) => if self.record_parser_error(&id, &mut sequence, &mut evidence, &active).await.is_err() {
                            self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                            break 'execution;
                        }
                    },
                    Some(ProcessEvent::Stderr(line)) => append_bounded_redacted(&mut evidence.stderr, &line, STDERR_LIMIT),
                    Some(ProcessEvent::OutputError) => if self.record_parser_error(&id, &mut sequence, &mut evidence, &active).await.is_err() {
                        self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                        break 'execution;
                    },
                    Some(ProcessEvent::Exited(_)) => {},
                    None => output_open = false,
                },
                _ = time::sleep(Duration::from_millis(10)) => match process.try_wait() {
                    Ok(Some(code)) => {
                        evidence.process_exit_code = code;
                        evidence.parent_exit_observed = true;
                        evidence.race_winner.get_or_insert(TerminalRaceWinner::ParentExit);
                        break 'execution;
                    }
                    Ok(None) => {},
                    Err(_) => {
                        evidence.fatal_runtime_error = Some(runtime_error(
                            "process_wait",
                            "unable to observe fake-agent process exit",
                        ));
                        break 'execution;
                    },
                }
            }
        }
        // Reaping the direct child does not prove that its owned process group
        // has exited: a descendant can retain an inherited output pipe. Treat
        // that as an abnormal lifecycle outcome, terminate the owned group,
        // and keep draining only for a bounded period.
        if evidence.parent_exit_observed && process_group_is_alive(&process) {
            evidence.lingering_process_group = true;
            active.cancellation_requested.store(true, Ordering::Release);
            if process.cancel(CANCEL_TIMEOUT).await.is_err() {
                evidence.termination_error = Some(runtime_error(
                    "termination_failed",
                    "fake-agent process cleanup failed",
                ));
            }
        }

        // A child that has exited normally closes the output channel promptly.
        // Do not let an inherited pipe turn that expectation into an unbounded
        // wait; cancellation remains observable while the bounded drain runs.
        let drain_deadline = time::Instant::now() + CANCEL_TIMEOUT;
        while evidence.parent_exit_observed && output_open && time::Instant::now() < drain_deadline
        {
            tokio::select! {
                command = commands.recv() => {
                    if command.is_some() {
                        // Parent exit was already observed. The exit-derived
                        // result wins this race, but consuming the request keeps
                        // the command channel from becoming a shutdown hazard.
                        active.cancellation_requested.store(true, Ordering::Release);
                    }
                }
                _ = time::sleep_until(drain_deadline) => break,
                event = output.recv() => match event {
                Some(ProcessEvent::Stdout(line)) => match serde_json::from_str::<AgentEvent>(&line)
                {
                    Ok(agent_event) => {
                        if self.record_agent_event(&id, &mut sequence, agent_event, &mut evidence, &active).await.is_err() {
                            self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                            break;
                        }
                    }
                    Err(_) => {
                        if self.record_parser_error(&id, &mut sequence, &mut evidence, &active).await.is_err() {
                            self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                            break;
                        }
                    }
                },
                Some(ProcessEvent::Stderr(line)) => {
                    append_bounded_redacted(&mut evidence.stderr, &line, STDERR_LIMIT);
                }
                Some(ProcessEvent::OutputError) => {
                    if self.record_parser_error(&id, &mut sequence, &mut evidence, &active).await.is_err() {
                        self.record_event_storage_failure(&mut evidence, &mut process, &active).await;
                        break;
                    }
                }
                Some(ProcessEvent::Exited(_)) => {}
                None => output_open = false,
                },
            }
        }
        self.finalize(&id, &mut sequence, evidence, true, &active)
            .await;
    }

    fn lifecycle_event(
        &self,
        id: &RunId,
        sequence: &mut u64,
        event_type: &str,
    ) -> NormalizedAgentEvent {
        self.event(id, sequence, event_type, json!({}))
    }
    fn parser_error_event(&self, id: &RunId, sequence: &mut u64) -> NormalizedAgentEvent {
        self.event(
            id,
            sequence,
            "parser_error",
            json!({"category":"malformed_agent_event"}),
        )
    }
    fn event(
        &self,
        id: &RunId,
        sequence: &mut u64,
        event_type: &str,
        payload: Value,
    ) -> NormalizedAgentEvent {
        *sequence += 1;
        NormalizedAgentEvent {
            run_id: id.clone(),
            sequence_number: *sequence,
            event_type: event_type.into(),
            schema_version: EVENT_SCHEMA_VERSION,
            occurred_at_ms: now_ms(),
            payload,
        }
    }
    fn normalize_event(
        &self,
        id: &RunId,
        sequence: &mut u64,
        agent_event: AgentEvent,
    ) -> NormalizedAgentEvent {
        let event_type = match &agent_event {
            AgentEvent::SessionStarted { .. } => "session_started",
            AgentEvent::PhaseChanged { .. } => "phase_changed",
            AgentEvent::Message { .. } => "message",
            AgentEvent::CommandStarted { .. } => "command_started",
            AgentEvent::CommandCompleted { .. } => "command_completed",
            AgentEvent::FileChanged { .. } => "file_changed",
            AgentEvent::WaitingForInput => "waiting_for_input",
            AgentEvent::Completed => "agent_completed",
            AgentEvent::Failed { .. } => "agent_failed",
        };
        let payload = redact_value(
            serde_json::to_value(agent_event)
                .unwrap_or_else(|_| json!({"type":"serialization_error"})),
        );
        self.event(id, sequence, event_type, payload)
    }
    async fn record_agent_event(
        &self,
        id: &RunId,
        sequence: &mut u64,
        agent_event: AgentEvent,
        evidence: &mut RunOutcomeEvidence,
        active: &ActiveRun,
    ) -> Result<(), CoreError> {
        if let AgentEvent::Failed { error } = &agent_event {
            evidence.agent_failure.get_or_insert_with(|| SafeRunError {
                category: "agent_failed".into(),
                message: bounded_redacted_text(error, STDERR_LIMIT),
            });
            evidence
                .race_winner
                .get_or_insert(TerminalRaceWinner::AgentFailure);
        }
        let event = self.normalize_event(id, sequence, agent_event);
        self.append_and_publish(event, active).await
    }
    async fn record_parser_error(
        &self,
        id: &RunId,
        sequence: &mut u64,
        evidence: &mut RunOutcomeEvidence,
        active: &ActiveRun,
    ) -> Result<(), CoreError> {
        evidence.malformed_protocol = true;
        let event = self.parser_error_event(id, sequence);
        self.append_and_publish(event, active).await
    }
    async fn record_event_storage_failure(
        &self,
        evidence: &mut RunOutcomeEvidence,
        process: &mut SupervisedProcess,
        active: &ActiveRun,
    ) {
        active.cancellation_requested.store(true, Ordering::Release);
        evidence.storage_error = Some(runtime_error(
            "event_persistence",
            "unable to persist normalized agent event",
        ));
        match process.cancel(CANCEL_TIMEOUT).await {
            Ok(code) => evidence.process_exit_code = code,
            Err(_) => {
                evidence.termination_error = Some(runtime_error(
                    "termination_failed",
                    "fake-agent process cleanup failed",
                ));
            }
        }
    }
    async fn persist_and_publish(
        &self,
        id: &RunId,
        status: RunStatus,
        error: Option<SafeRunError>,
        event: NormalizedAgentEvent,
        active: &ActiveRun,
    ) -> Result<(), CoreError> {
        self.repository
            .transition_with_event(id, status, error, &event)
            .await?;
        let _ = active.events.send(event);
        Ok(())
    }
    async fn append_and_publish(
        &self,
        event: NormalizedAgentEvent,
        active: &ActiveRun,
    ) -> Result<(), CoreError> {
        self.repository.append_event(&event).await?;
        let _ = active.events.send(event);
        Ok(())
    }
    /// The only normal terminal persistence path. All runner branches collect
    /// evidence and funnel through this method exactly once.
    async fn finalize(
        &self,
        id: &RunId,
        sequence: &mut u64,
        evidence: RunOutcomeEvidence,
        persist_terminal: bool,
        active: &ActiveRun,
    ) {
        let terminal = resolve_terminal_outcome(&evidence);
        let outcome = if persist_terminal {
            let event = self.lifecycle_event(id, sequence, terminal.lifecycle_event_type);
            match self
                .repository
                .finish_run(
                    id,
                    terminal.status,
                    terminal.exit_code,
                    terminal.error,
                    Some(&event),
                )
                .await
            {
                Ok(run) => {
                    let _ = active.events.send(event);
                    terminal.completion_error.map_or(Ok(run), Err)
                }
                Err(_) => Err(RuntimeError::Storage),
            }
        } else {
            Err(terminal.completion_error.unwrap_or(RuntimeError::Storage))
        };
        self.complete_active(id, active, outcome).await;
    }

    async fn complete_active(
        &self,
        id: &RunId,
        active: &ActiveRun,
        outcome: Result<Run, RuntimeError>,
    ) {
        let _ = active.completion.send(Some(outcome));
        self.active.lock().await.remove(id);
    }
}

fn runtime_error(category: &str, message: &str) -> SafeRunError {
    SafeRunError {
        category: category.into(),
        message: message.into(),
    }
}

fn failed_outcome(
    evidence: &RunOutcomeEvidence,
    error: SafeRunError,
    completion_error: Option<RuntimeError>,
) -> TerminalOutcome {
    TerminalOutcome {
        status: RunStatus::Failed,
        exit_code: evidence.process_exit_code,
        error: Some(error),
        lifecycle_event_type: "run_failed",
        completion_error,
    }
}

/// Resolves collected facts without performing I/O. A winner records which
/// terminal observation arrived first, so a later cancellation cannot rewrite
/// a fixed parent-exit result and a confirmed cancellation cannot be rewritten
/// by its own termination exit code.
fn resolve_terminal_outcome(evidence: &RunOutcomeEvidence) -> TerminalOutcome {
    if let Some(error) = &evidence.storage_error {
        return failed_outcome(evidence, error.clone(), Some(RuntimeError::Storage));
    }
    if let Some(error) = &evidence.termination_error {
        return failed_outcome(evidence, error.clone(), Some(RuntimeError::Termination));
    }
    if let Some(error) = &evidence.fatal_runtime_error {
        return failed_outcome(evidence, error.clone(), None);
    }
    if evidence.lingering_process_group {
        return failed_outcome(
            evidence,
            runtime_error(
                "lingering_process_group",
                "owned descendant remained active after parent exit",
            ),
            None,
        );
    }
    if evidence.race_winner == Some(TerminalRaceWinner::Cancellation) {
        return TerminalOutcome {
            status: RunStatus::Cancelled,
            exit_code: evidence.process_exit_code,
            error: None,
            lifecycle_event_type: "run_cancelled",
            completion_error: None,
        };
    }
    if let Some(error) = &evidence.agent_failure {
        return failed_outcome(evidence, error.clone(), None);
    }
    if evidence.malformed_protocol {
        return failed_outcome(
            evidence,
            runtime_error(
                "malformed_agent_event",
                "fake agent emitted malformed structured output",
            ),
            None,
        );
    }
    if evidence.process_exit_code != Some(0) {
        return failed_outcome(
            evidence,
            runtime_error(
                "fake_agent_exit",
                if evidence.stderr.is_empty() {
                    "fake agent exited unexpectedly"
                } else {
                    &evidence.stderr
                },
            ),
            None,
        );
    }
    if evidence.race_winner == Some(TerminalRaceWinner::ParentExit) {
        return TerminalOutcome {
            status: RunStatus::Completed,
            exit_code: evidence.process_exit_code,
            error: None,
            lifecycle_event_type: "run_completed",
            completion_error: None,
        };
    }
    failed_outcome(
        evidence,
        runtime_error(
            "insufficient_terminal_evidence",
            "unable to determine fake-agent terminal outcome",
        ),
        None,
    )
}

#[cfg(unix)]
fn process_group_is_alive(process: &SupervisedProcess) -> bool {
    process.process_group_is_alive()
}

#[cfg(not(unix))]
fn process_group_is_alive(_: &SupervisedProcess) -> bool {
    false
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(i64::MAX)
        })
}
fn redact_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(redact(&value)),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, redact_value(value)))
                .collect(),
        ),
        other => other,
    }
}

/// Redacts first, then keeps a valid UTF-8 prefix within `byte_limit` bytes.
/// The ellipsis marks truncation whenever its UTF-8 representation fits.
pub fn bounded_redacted_text(input: &str, byte_limit: usize) -> String {
    let redacted = redact(input);
    if redacted.len() <= byte_limit {
        return redacted;
    }
    if byte_limit == 0 {
        return String::new();
    }
    const ELLIPSIS: &str = "…";
    if byte_limit < ELLIPSIS.len() {
        return redacted[..utf8_prefix_end(&redacted, byte_limit)].to_owned();
    }
    let content_limit = byte_limit.saturating_sub(ELLIPSIS.len());
    let end = utf8_prefix_end(&redacted, content_limit);
    let mut result = redacted[..end].to_owned();
    result.push_str(ELLIPSIS);
    result
}

fn append_bounded_redacted(destination: &mut String, input: &str, byte_limit: usize) {
    let remaining = byte_limit.saturating_sub(destination.len());
    if remaining != 0 {
        destination.push_str(&bounded_redacted_text(input, remaining));
    }
}

fn utf8_prefix_end(value: &str, byte_limit: usize) -> usize {
    let mut end = value.len().min(byte_limit);
    while end != 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn terminal_outcome_precedence_is_deterministic() {
        struct Case {
            name: &'static str,
            agent_failed: bool,
            exit_code: Option<i32>,
            cancellation_won: bool,
            runtime_failed: bool,
            lingering_group: bool,
            expected: RunStatus,
        }
        let cases = [
            Case {
                name: "clean zero exit",
                agent_failed: false,
                exit_code: Some(0),
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Completed,
            },
            Case {
                name: "agent failure with zero exit",
                agent_failed: true,
                exit_code: Some(0),
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Failed,
            },
            Case {
                name: "agent failure with nonzero exit",
                agent_failed: true,
                exit_code: Some(1),
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Failed,
            },
            Case {
                name: "nonzero exit",
                agent_failed: false,
                exit_code: Some(1),
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Failed,
            },
            Case {
                name: "cancellation wins",
                agent_failed: false,
                exit_code: Some(0),
                cancellation_won: true,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Cancelled,
            },
            // The explicit cancellation winner records that cleanup began before
            // later agent output, so it wins this otherwise contradictory race.
            Case {
                name: "cancellation wins over later agent failure",
                agent_failed: true,
                exit_code: Some(0),
                cancellation_won: true,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Cancelled,
            },
            Case {
                name: "runtime failure",
                agent_failed: false,
                exit_code: Some(0),
                cancellation_won: false,
                runtime_failed: true,
                lingering_group: false,
                expected: RunStatus::Failed,
            },
            Case {
                name: "lingering group",
                agent_failed: false,
                exit_code: Some(0),
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: true,
                expected: RunStatus::Failed,
            },
            Case {
                name: "no exit evidence",
                agent_failed: false,
                exit_code: None,
                cancellation_won: false,
                runtime_failed: false,
                lingering_group: false,
                expected: RunStatus::Failed,
            },
        ];

        for case in cases {
            let mut evidence = RunOutcomeEvidence {
                process_exit_code: case.exit_code,
                race_winner: case
                    .cancellation_won
                    .then_some(TerminalRaceWinner::Cancellation)
                    .or_else(|| case.exit_code.map(|_| TerminalRaceWinner::ParentExit)),
                lingering_process_group: case.lingering_group,
                ..RunOutcomeEvidence::default()
            };
            if case.agent_failed {
                evidence.agent_failure = Some(runtime_error("agent_failed", "reported failure"));
            }
            if case.runtime_failed {
                evidence.fatal_runtime_error = Some(runtime_error("runtime", "failed"));
            }
            assert_eq!(
                resolve_terminal_outcome(&evidence).status,
                case.expected,
                "{}",
                case.name
            );
        }
    }
}
