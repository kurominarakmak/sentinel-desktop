//! Supported Codex App Server transport owned by the Sentinel supervisor.
//!
//! The transport has one owner for each direction: a locked writer owns child
//! stdin and one dispatcher owns stdout.  Requests never read stdout directly.

use sentinel_core::{
    redact,
    v3::{CreateSession, EventId, EventKind, NormalizedEventEnvelope, SessionId, TaskId},
    CoreError, RunRepository,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot, watch, Mutex},
    task::JoinHandle,
    time,
};

pub const PROVIDER: &str = "openai.codex.app_server";
pub const MAX_JSON_LINE_BYTES: usize = 16 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;
pub const MAX_PENDING_REQUESTS: usize = 64;
pub const START_TIMEOUT: Duration = Duration::from_secs(5);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodexTimeouts {
    pub startup: Duration,
    pub request: Duration,
    pub interrupt: Duration,
    pub shutdown: Duration,
}
impl Default for CodexTimeouts {
    fn default() -> Self {
        Self {
            startup: START_TIMEOUT,
            request: REQUEST_TIMEOUT,
            interrupt: REQUEST_TIMEOUT,
            shutdown: SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexInstallation {
    Available {
        executable: PathBuf,
        version: String,
    },
    Missing,
    Unsupported {
        detail: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexProgram {
    executable: PathBuf,
}
impl CodexProgram {
    pub fn from_executable(executable: impl Into<PathBuf>) -> Result<Self, CodexError> {
        let executable = executable.into();
        executable
            .is_file()
            .then_some(Self { executable })
            .ok_or(CodexError::MissingExecutable)
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
}
pub async fn detect_installation(executable: impl Into<PathBuf>) -> CodexInstallation {
    let executable = executable.into();
    if !executable.is_file() {
        return CodexInstallation::Missing;
    }
    match time::timeout(
        START_TIMEOUT,
        Command::new(&executable).arg("--version").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            let version = bounded_text(&output.stdout);
            if version.is_empty() {
                CodexInstallation::Unsupported {
                    detail: "Codex did not report a version.".into(),
                }
            } else {
                CodexInstallation::Available {
                    executable,
                    version,
                }
            }
        }
        Ok(Ok(output)) => CodexInstallation::Unsupported {
            detail: bounded_text(&output.stderr),
        },
        Ok(Err(error)) => CodexInstallation::Unsupported {
            detail: redact(&error.to_string()),
        },
        Err(_) => CodexInstallation::Unsupported {
            detail: "Codex version check timed out.".into(),
        },
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CodexError {
    #[error("Codex executable is missing or not a regular file")]
    MissingExecutable,
    #[error("Codex App Server failed to start")]
    StartFailed,
    #[error("Codex App Server request timed out")]
    Timeout,
    #[error("Codex App Server exited unexpectedly")]
    UnexpectedExit,
    #[error("malformed or oversized Codex JSON-RPC message")]
    MalformedMessage,
    #[error("unexpected JSON-RPC response")]
    UnexpectedResponse,
    #[error("Codex App Server returned an error")]
    RpcError,
    #[error("Codex App Server does not support this protocol version or method")]
    Unsupported,
    #[error("Codex thread or turn identifier is missing")]
    MissingIdentifier,
    #[error("invalid adapter input")]
    InvalidInput,
    #[error("too many in-flight Codex requests")]
    RequestCapacity,
    #[error("persistence failure")]
    Storage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexSession {
    pub session_id: SessionId,
    pub thread_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexTurn {
    pub turn_id: String,
}

/// The supported account-rate-limit payload, retained verbatim only after its
/// two documented windows have passed basic shape validation.
#[derive(Clone, Debug, PartialEq)]
pub struct CodexRateLimits(pub Value);

fn rate_limits(value: &Value) -> Option<CodexRateLimits> {
    let limits = value.get("rateLimits").unwrap_or(value);
    let object = limits.as_object()?;
    let valid_window = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_object)
            .is_some_and(|window| {
                window
                    .get("usedPercent")
                    .and_then(Value::as_f64)
                    .is_some_and(|used| (0.0..=100.0).contains(&used))
                    && window
                        .get("resetsAt")
                        .and_then(Value::as_str)
                        .is_some_and(|reset| !reset.is_empty())
            })
    };
    (valid_window("primary") || valid_window("secondary")).then(|| CodexRateLimits(limits.clone()))
}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, CodexError>>>;
struct Shared {
    stdin: Mutex<ChildStdin>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    notifications: mpsc::Sender<NotificationCommand>,
    alive: std::sync::atomic::AtomicBool,
}
struct EventState {
    repository: RunRepository,
    task_id: TaskId,
    sequence: u64,
    arrival_sequence: u64,
    seen: HashSet<String>,
    sessions: HashMap<String, SessionId>,
    pending_turn: Option<String>,
    buffered: Vec<BufferedNotification>,
    completed_turns: HashSet<String>,
    started_items: HashSet<(String, String)>,
    rate_limits: watch::Sender<Option<CodexRateLimits>>,
}
struct BufferedNotification {
    value: Value,
    arrival_sequence: u64,
}
enum NotificationCommand {
    Event(Value),
    Diagnostic {
        kind: &'static str,
        frame_bytes: usize,
    },
    Activate(String),
    Flush {
        thread: String,
        done: oneshot::Sender<Result<(), CodexError>>,
    },
    Clear,
    Stop(oneshot::Sender<()>),
}
enum ExitCommand {
    Shutdown {
        timeout: Duration,
        done: oneshot::Sender<Result<(), CodexError>>,
    },
}

/// A direct JSONL connection to exactly one child started by this object.
pub struct CodexAppServer {
    shared: Arc<Shared>,
    state: Arc<Mutex<EventState>>,
    exit: mpsc::Sender<ExitCommand>,
    dispatcher: Option<JoinHandle<()>>,
    notification_worker: Option<JoinHandle<()>>,
    exit_watcher: Option<JoinHandle<()>>,
    timeouts: CodexTimeouts,
    pid: Option<u32>,
    rate_limits: watch::Receiver<Option<CodexRateLimits>>,
    rate_limits_tx: watch::Sender<Option<CodexRateLimits>>,
}

impl CodexAppServer {
    pub async fn start(
        program: CodexProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
    ) -> Result<Self, CodexError> {
        Self::start_with_timeouts(program, repository, task_id, cwd, CodexTimeouts::default()).await
    }
    pub async fn start_with_timeouts(
        program: CodexProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        timeouts: CodexTimeouts,
    ) -> Result<Self, CodexError> {
        if !cwd.is_dir() {
            return Err(CodexError::InvalidInput);
        }
        // A recreated desktop-owned connection continues the durable event
        // stream for this Sentinel task. It must never reuse sequence zero.
        let sequence = repository
            .v3()
            .list_events(&task_id)
            .await
            .map_err(|_| CodexError::Storage)?
            .last()
            .map(|event| event.sequence_number)
            .unwrap_or(0);
        let mut command = Command::new(program.executable());
        command
            .arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| CodexError::StartFailed)?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or(CodexError::StartFailed)?;
        let stdout = child.stdout.take().ok_or(CodexError::StartFailed)?;
        let (notification_tx, notification_rx) = mpsc::channel(128);
        let shared = Arc::new(Shared {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            notifications: notification_tx.clone(),
            alive: std::sync::atomic::AtomicBool::new(true),
        });
        let (rate_limits_tx, rate_limits_rx) = watch::channel(None);
        let state = Arc::new(Mutex::new(EventState {
            repository,
            task_id,
            sequence,
            arrival_sequence: 0,
            seen: HashSet::new(),
            sessions: HashMap::new(),
            pending_turn: None,
            buffered: Vec::new(),
            completed_turns: HashSet::new(),
            started_items: HashSet::new(),
            rate_limits: rate_limits_tx.clone(),
        }));
        let notification_worker = tokio::spawn(notification_worker(notification_rx, state.clone()));
        let dispatcher = tokio::spawn(dispatch_stdout(stdout, shared.clone()));
        let (exit_tx, exit_rx) = mpsc::channel(1);
        let exit_watcher = tokio::spawn(watch_child(child, exit_rx, shared.clone()));
        let mut server = Self {
            shared,
            state,
            exit: exit_tx,
            dispatcher: Some(dispatcher),
            notification_worker: Some(notification_worker),
            exit_watcher: Some(exit_watcher),
            timeouts,
            pid,
            rate_limits: rate_limits_rx,
            rate_limits_tx,
        };
        let initialize = json!({"clientInfo":{"name":"agent-sentinel","title":"Agent Sentinel","version":"3"},"capabilities":{}});
        let initialized = match time::timeout(
            server.timeouts.startup,
            server.request("initialize", initialize),
        )
        .await
        .map_err(|_| CodexError::Timeout)
        .and_then(|v| v)
        {
            Ok(value) => value,
            Err(error) => {
                let _ = server.shutdown().await;
                return Err(error);
            }
        };
        server.notify("initialized", json!({})).await?;
        if initialized.get("protocolVersion").and_then(Value::as_str) != Some("1") {
            let _ = server.shutdown().await;
            return Err(CodexError::Unsupported);
        }
        // Unsupported and unavailable accounts must not manufacture a value.
        // A later update remains authoritative if this read races startup.
        let _ = server.read_rate_limits().await;
        Ok(server)
    }
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire)
    }
    /// Exposed for deterministic transport diagnostics and tests.
    pub async fn pending_request_count(&self) -> usize {
        self.shared.pending.lock().await.len()
    }
    /// Exposed for deterministic transport diagnostics and tests.
    pub async fn correlation_buffer_count(&self) -> usize {
        self.state.lock().await.buffered.len()
    }
    pub fn subscribe_rate_limits(&self) -> watch::Receiver<Option<CodexRateLimits>> {
        self.rate_limits.clone()
    }
    pub fn latest_rate_limits(&self) -> Option<CodexRateLimits> {
        self.rate_limits.borrow().clone()
    }
    pub async fn read_rate_limits(&self) -> Result<Option<CodexRateLimits>, CodexError> {
        let result = self.request("account/rateLimits/read", json!({})).await?;
        let snapshot = rate_limits(&result);
        if let Some(snapshot) = &snapshot {
            let _ = self.rate_limits_tx.send(Some(snapshot.clone()));
        }
        Ok(snapshot)
    }

    pub async fn start_thread(&self, cwd: &Path) -> Result<CodexSession, CodexError> {
        let result = self.request("thread/start", json!({"cwd":cwd})).await?;
        self.session_from_result(result, EventKind::SessionStarted)
            .await
    }
    pub async fn resume_thread(&self, thread_id: &str) -> Result<CodexSession, CodexError> {
        valid_id(thread_id)?;
        let result = self
            .request("thread/resume", json!({"threadId":thread_id}))
            .await?;
        self.session_from_result_with_fallback(result, thread_id, EventKind::SessionResumed)
            .await
    }
    async fn session_from_result(
        &self,
        result: Value,
        kind: EventKind,
    ) -> Result<CodexSession, CodexError> {
        let thread = identifier(&result, &["thread.id", "thread_id", "id"])?;
        self.session_for_thread(thread, kind).await
    }
    async fn session_from_result_with_fallback(
        &self,
        result: Value,
        fallback: &str,
        kind: EventKind,
    ) -> Result<CodexSession, CodexError> {
        let thread = identifier(&result, &["thread.id", "thread_id", "id"])
            .unwrap_or_else(|_| fallback.into());
        self.session_for_thread(thread, kind).await
    }
    async fn session_for_thread(
        &self,
        thread_id: String,
        kind: EventKind,
    ) -> Result<CodexSession, CodexError> {
        let (repository, task_id) = {
            let state = self.state.lock().await;
            (state.repository.clone(), state.task_id.clone())
        };
        let session = match repository
            .v3()
            .get_session_by_provider_reference(&task_id, PROVIDER, &thread_id)
            .await
        {
            Ok(s) => s,
            Err(CoreError::NotFound) => repository
                .v3()
                .create_session(
                    CreateSession {
                        task_id,
                        provider: PROVIDER.into(),
                        provider_session_ref: thread_id.clone(),
                    },
                    now_ms(),
                )
                .await
                .map_err(|_| CodexError::Storage)?,
            Err(_) => return Err(CodexError::Storage),
        };
        self.emit(
            kind,
            Some(session.id.clone()),
            json!({"thread_id":thread_id}),
            None,
        )
        .await?;
        self.state
            .lock()
            .await
            .sessions
            .insert(thread_id.clone(), session.id.clone());
        Ok(CodexSession {
            session_id: session.id,
            thread_id,
        })
    }
    pub async fn start_turn(
        &self,
        session: &CodexSession,
        prompt: &str,
    ) -> Result<CodexTurn, CodexError> {
        if prompt.trim().is_empty() || prompt.len() > 8000 || prompt.contains('\0') {
            return Err(CodexError::InvalidInput);
        }
        self.shared
            .notifications
            .send(NotificationCommand::Activate(session.thread_id.clone()))
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        let result = self
            .request(
                "turn/start",
                json!({"threadId":session.thread_id,"input":[{"type":"text","text":prompt}]}),
            )
            .await;
        let result = match result {
            Ok(v) => v,
            Err(e) => {
                self.clear_turn().await;
                return Err(e);
            }
        };
        let turn_id = match identifier(&result, &["turn.id", "turn_id", "id"]) {
            Ok(v) => v,
            Err(e) => {
                self.clear_turn().await;
                return Err(e);
            }
        };
        self.emit(
            EventKind::ToolStarted,
            Some(session.session_id.clone()),
            json!({"thread_id":session.thread_id,"turn_id":turn_id,"event":"turn_started"}),
            None,
        )
        .await?;
        let (done_tx, done_rx) = oneshot::channel();
        self.shared
            .notifications
            .send(NotificationCommand::Flush {
                thread: session.thread_id.clone(),
                done: done_tx,
            })
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        done_rx.await.map_err(|_| CodexError::UnexpectedExit)??;
        Ok(CodexTurn { turn_id })
    }
    pub async fn interrupt_turn(
        &self,
        session: &CodexSession,
        turn: &CodexTurn,
    ) -> Result<(), CodexError> {
        valid_id(&turn.turn_id)?;
        time::timeout(
            self.timeouts.interrupt,
            self.request(
                "turn/interrupt",
                json!({"threadId":session.thread_id,"turnId":turn.turn_id}),
            ),
        )
        .await
        .map_err(|_| CodexError::Timeout)??;
        self.emit(
            EventKind::SessionCancelled,
            Some(session.session_id.clone()),
            json!({"turn_id":turn.turn_id}),
            None,
        )
        .await
    }
    pub async fn shutdown(&mut self) -> Result<(), CodexError> {
        self.clear_turn().await;
        let (done_tx, done_rx) = oneshot::channel();
        let _ = self
            .exit
            .send(ExitCommand::Shutdown {
                timeout: self.timeouts.shutdown,
                done: done_tx,
            })
            .await;
        let result = done_rx.await.unwrap_or(Err(CodexError::UnexpectedExit));
        if let Some(task) = self.exit_watcher.take() {
            let _ = task.await;
        }
        if let Some(task) = self.dispatcher.take() {
            let _ = task.await;
        }
        let (stop_tx, stop_rx) = oneshot::channel();
        let _ = self
            .shared
            .notifications
            .send(NotificationCommand::Stop(stop_tx))
            .await;
        let _ = stop_rx.await;
        if let Some(task) = self.notification_worker.take() {
            let _ = task.await;
        }
        result
    }
    async fn clear_turn(&self) {
        let _ = self
            .shared
            .notifications
            .send(NotificationCommand::Clear)
            .await;
    }
    async fn notify(&self, method: &str, params: Value) -> Result<(), CodexError> {
        self.write(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    async fn request(&self, method: &str, params: Value) -> Result<Value, CodexError> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.shared.pending.lock().await;
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(CodexError::RequestCapacity);
            }
            pending.insert(id, tx);
        }
        if let Err(error) = self
            .write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
        {
            self.shared.pending.lock().await.remove(&id);
            return Err(error);
        }
        match time::timeout(self.timeouts.request, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CodexError::UnexpectedExit),
            Err(_) => {
                self.shared.pending.lock().await.remove(&id);
                Err(CodexError::Timeout)
            }
        }
    }
    async fn write(&self, value: Value) -> Result<(), CodexError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| CodexError::MalformedMessage)?;
        if encoded.len() > MAX_JSON_LINE_BYTES {
            return Err(CodexError::InvalidInput);
        }
        let mut stdin = self.shared.stdin.lock().await;
        stdin
            .write_all(&encoded)
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        stdin.flush().await.map_err(|_| CodexError::UnexpectedExit)
    }
    async fn emit(
        &self,
        kind: EventKind,
        session_id: Option<SessionId>,
        payload: Value,
        raw: Option<Value>,
    ) -> Result<(), CodexError> {
        emit_state(
            &mut *self.state.lock().await,
            kind,
            session_id,
            payload,
            raw,
        )
        .await
    }
}
impl Drop for CodexAppServer {
    fn drop(&mut self) {
        // `Child` does not kill on drop. Hand shutdown to its sole owner before
        // detaching; the completion receiver can be dropped safely.
        let (done, _) = oneshot::channel();
        let _ = self.exit.try_send(ExitCommand::Shutdown {
            timeout: self.timeouts.shutdown,
            done,
        });
        if let Some(task) = self.dispatcher.take() {
            task.abort();
        }
        if let Some(task) = self.notification_worker.take() {
            task.abort();
        }
        // Keep the watcher alive: it owns and reaps the child.
        let _ = self.exit_watcher.take();
    }
}

async fn dispatch_stdout(stdout: tokio::process::ChildStdout, shared: Arc<Shared>) {
    let mut stdout = stdout;
    let mut chunk = [0_u8; 4096];
    let mut frame = Vec::new();
    let mut discarding_oversized_frame = false;
    loop {
        match stdout.read(&mut chunk).await {
            Ok(0) | Err(_) => {
                shared.alive.store(false, Ordering::Release);
                complete_all(&shared.pending, CodexError::UnexpectedExit).await;
                let _ = shared.notifications.send(NotificationCommand::Clear).await;
                break;
            }
            Ok(read) => {
                for byte in &chunk[..read] {
                    if discarding_oversized_frame {
                        if *byte == b'\n' {
                            discarding_oversized_frame = false;
                        }
                        continue;
                    }
                    if *byte == b'\n' {
                        process_frame(&frame, &shared).await;
                        frame.clear();
                    } else {
                        frame.push(*byte);
                        if frame.len() > MAX_JSON_LINE_BYTES {
                            frame.clear();
                            discarding_oversized_frame = true;
                            let _ = shared
                                .notifications
                                .send(NotificationCommand::Diagnostic {
                                    kind: "oversized_frame",
                                    frame_bytes: MAX_JSON_LINE_BYTES + 1,
                                })
                                .await;
                        }
                    }
                }
            }
        }
    }
}
async fn process_frame(frame: &[u8], shared: &Arc<Shared>) {
    let frame = if frame.last() == Some(&b'\r') {
        &frame[..frame.len() - 1]
    } else {
        frame
    };
    if frame.is_empty() {
        return;
    }
    let value = match parse_message(frame) {
        Ok(value) => value,
        Err(_) => {
            let _ = shared
                .notifications
                .send(NotificationCommand::Diagnostic {
                    kind: "malformed_frame",
                    frame_bytes: frame.len(),
                })
                .await;
            return;
        }
    };
    if value.get("method").is_some() {
        let _ = shared
            .notifications
            .send(NotificationCommand::Event(value))
            .await;
        return;
    }
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        let _ = shared
            .notifications
            .send(NotificationCommand::Diagnostic {
                kind: "invalid_frame_shape",
                frame_bytes: frame.len(),
            })
            .await;
        return;
    };
    let outcome = if value.get("error").is_some() {
        Err(CodexError::RpcError)
    } else {
        value
            .get("result")
            .cloned()
            .ok_or(CodexError::MalformedMessage)
    };
    // Remove before completing: duplicate or stale responses cannot settle another request.
    if let Some(sender) = shared.pending.lock().await.remove(&id) {
        let _ = sender.send(outcome);
    } else {
        let _ = shared
            .notifications
            .send(NotificationCommand::Diagnostic {
                kind: "unmatched_response",
                frame_bytes: frame.len(),
            })
            .await;
    }
}
async fn watch_child(
    mut child: Child,
    mut commands: mpsc::Receiver<ExitCommand>,
    shared: Arc<Shared>,
) {
    let result = tokio::select! {
        status = child.wait() => status.map(|_| ()).map_err(|_| CodexError::UnexpectedExit),
        command = commands.recv() => match command {
            Some(ExitCommand::Shutdown { timeout, done }) => {
                let _ = shared.stdin.lock().await.shutdown().await;
                let outcome = match time::timeout(timeout, child.wait()).await { Ok(Ok(_)) => Ok(()), Ok(Err(_)) => Err(CodexError::UnexpectedExit), Err(_) => match child.kill().await { Ok(()) => child.wait().await.map(|_| ()).map_err(|_| CodexError::UnexpectedExit), Err(_) => Err(CodexError::UnexpectedExit) } };
                let _ = done.send(outcome.clone()); outcome
            }
            None => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(CodexError::UnexpectedExit)
            }
        }
    };
    complete_all(&shared.pending, CodexError::UnexpectedExit).await;
    shared.alive.store(false, Ordering::Release);
    let _ = shared.notifications.send(NotificationCommand::Clear).await;
    let _ = result;
}
async fn complete_all(pending: &Mutex<Pending>, error: CodexError) {
    let drained = std::mem::take(&mut *pending.lock().await);
    for (_, sender) in drained {
        let _ = sender.send(Err(error.clone()));
    }
}

async fn notification_worker(
    mut receiver: mpsc::Receiver<NotificationCommand>,
    state: Arc<Mutex<EventState>>,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            NotificationCommand::Event(value) => {
                let mut state = state.lock().await;
                state.arrival_sequence += 1;
                let arrival_sequence = state.arrival_sequence;
                let _ = handle_notification(&mut state, &value, arrival_sequence).await;
            }
            NotificationCommand::Diagnostic { kind, frame_bytes } => {
                let mut state = state.lock().await;
                state.arrival_sequence += 1;
                let arrival_sequence = state.arrival_sequence;
                let _ = emit_state(
                    &mut state,
                    EventKind::Unknown {
                        discriminator: format!("transport/{kind}"),
                    },
                    None,
                    json!({"transport_diagnostic":kind,"arrival_sequence":arrival_sequence,"frame_bytes":frame_bytes.min(MAX_JSON_LINE_BYTES)}),
                    None,
                )
                .await;
            }
            NotificationCommand::Activate(thread) => {
                let mut state = state.lock().await;
                state.pending_turn = Some(thread);
                state.buffered.clear();
            }
            NotificationCommand::Flush { thread, done } => {
                let result = flush_notifications(&mut *state.lock().await, &thread).await;
                let _ = done.send(result);
            }
            NotificationCommand::Clear => {
                let mut state = state.lock().await;
                state.pending_turn = None;
                state.buffered.clear();
            }
            NotificationCommand::Stop(done) => {
                let _ = done.send(());
                break;
            }
        }
    }
}
async fn flush_notifications(state: &mut EventState, thread: &str) -> Result<(), CodexError> {
    let buffered = if state.pending_turn.as_deref() == Some(thread) {
        state.pending_turn = None;
        std::mem::take(&mut state.buffered)
    } else {
        Vec::new()
    };
    for notification in buffered {
        handle_notification(state, &notification.value, notification.arrival_sequence).await?;
    }
    Ok(())
}
async fn handle_notification(
    state: &mut EventState,
    value: &Value,
    arrival_sequence: u64,
) -> Result<(), CodexError> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or(CodexError::MalformedMessage)?;
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    if method == "account/rateLimits/updated" {
        if let Some(snapshot) = rate_limits(&params) {
            let _ = state.rate_limits.send(Some(snapshot));
        }
    }
    let thread_id = params.get("threadId").and_then(Value::as_str).or_else(|| {
        params
            .get("thread")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
    });
    if state.pending_turn.as_deref() == thread_id && !matches!(method, "turn/started") {
        state.buffered.push(BufferedNotification {
            value: value.clone(),
            arrival_sequence,
        });
        return Ok(());
    }
    let fingerprint = serde_json::to_string(value).unwrap_or_default();
    if !state.seen.insert(fingerprint) {
        return Ok(());
    }
    let provider_id = provider_notification_id(value, &params, method);
    let turn_id = notification_turn_id(&params);
    let item_id = params
        .get("item")
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let item_key = turn_id.clone().zip(item_id.clone());
    let transport_diagnostic = if turn_id
        .as_ref()
        .is_some_and(|turn_id| state.completed_turns.contains(turn_id))
        && method != "turn/completed"
    {
        Some("late_notification")
    } else if method == "item/completed"
        && item_key
            .as_ref()
            .is_some_and(|item| !state.started_items.contains(item))
    {
        Some("out_of_order_item_notification")
    } else {
        None
    };
    if method == "item/started" {
        if let Some(item) = item_key.clone() {
            state.started_items.insert(item);
        }
    }
    if method == "turn/completed" {
        if let Some(turn_id) = &turn_id {
            state.completed_turns.insert(turn_id.clone());
        }
    }
    let kind = match method {
        "turn/started" => EventKind::ToolStarted,
        "item/agentMessage/delta" => EventKind::Message,
        "item/started" | "item/updated" => EventKind::ToolStarted,
        "item/completed" | "turn/completed" => EventKind::ToolCompleted,
        "thread/updated" => EventKind::FileChanged,
        "turn/failed" => EventKind::SessionFailed,
        "turn/cancelled" => EventKind::SessionCancelled,
        "approval/requested" => EventKind::ApprovalRequested,
        _ => EventKind::Unknown {
            discriminator: method.into(),
        },
    };
    emit_state(state, kind, thread_id.and_then(|id| state.sessions.get(id).cloned()), json!({"provider_event_id":provider_id,"arrival_sequence":arrival_sequence,"method":method,"thread_id":thread_id,"turn_id":turn_id,"item_id":item_id,"transport_diagnostic":transport_diagnostic,"params":bounded_value(params)}), Some(bounded_value(json!({"method":method,"provider_event_id":provider_id,"arrival_sequence":arrival_sequence,"transport_diagnostic":transport_diagnostic})))).await
}
fn provider_notification_id(value: &Value, params: &Value, method: &str) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| params.get("id").and_then(Value::as_str))
        .or_else(|| {
            params
                .get("item")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{method}:{}",
                serde_json::to_string(params).unwrap_or_default()
            )
        })
}
fn notification_turn_id(params: &Value) -> Option<String> {
    params
        .get("turnId")
        .and_then(Value::as_str)
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
}
async fn emit_state(
    state: &mut EventState,
    kind: EventKind,
    session_id: Option<SessionId>,
    payload: Value,
    raw: Option<Value>,
) -> Result<(), CodexError> {
    state.sequence += 1;
    state
        .repository
        .v3()
        .append_event(&NormalizedEventEnvelope {
            event_id: EventId::new(),
            task_id: state.task_id.clone(),
            session_id,
            provider: PROVIDER.into(),
            kind,
            schema_version: 1,
            occurred_at_ms: now_ms(),
            sequence_number: state.sequence,
            causation_id: None,
            correlation_id: None,
            payload: bounded_value(payload),
            raw_diagnostic_payload: raw.map(bounded_value),
        })
        .await
        .map_err(|_| CodexError::Storage)
}

fn parse_message(bytes: &[u8]) -> Result<Value, CodexError> {
    if bytes.is_empty() || bytes.len() > MAX_JSON_LINE_BYTES {
        return Err(CodexError::MalformedMessage);
    }
    serde_json::from_slice(bytes).map_err(|_| CodexError::MalformedMessage)
}
fn identifier(value: &Value, paths: &[&str]) -> Result<String, CodexError> {
    for path in paths {
        let found = path
            .split('.')
            .fold(Some(value), |item, key| item.and_then(|v| v.get(key)))
            .and_then(Value::as_str);
        if let Some(value) = found {
            valid_id(value)?;
            return Ok(value.into());
        }
    }
    Err(CodexError::MissingIdentifier)
}
fn valid_id(value: &str) -> Result<(), CodexError> {
    if value.is_empty() || value.len() > 512 || value.contains('\0') {
        Err(CodexError::InvalidInput)
    } else {
        Ok(())
    }
}
fn bounded_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_DIAGNOSTIC_BYTES)])
        .trim()
        .to_owned()
}
fn bounded_value(value: Value) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_default();
    if text.len() <= MAX_DIAGNOSTIC_BYTES {
        value
    } else {
        let diagnostic = redact(truncate_utf8(&text, 512));
        let bounded = json!({"truncated":true,"diagnostic":diagnostic});
        if serde_json::to_vec(&bounded)
            .map(|bytes| bytes.len() <= MAX_DIAGNOSTIC_BYTES)
            .unwrap_or(false)
        {
            bounded
        } else {
            json!({"truncated":true,"diagnostic":"diagnostic omitted"})
        }
    }
}
fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis() as i64)
        .unwrap_or(0)
}
