//! Sentinel-owned transport for Oh My Pi (OMP) JSONL RPC mode.
//!
//! OMP owns provider authentication and model-specific request handling. This
//! adapter owns only the OMP child process and maps its documented RPC frames
//! to Sentinel's durable event/session model.

use sentinel_core::{
    redact,
    v3::{CreateSession, EventKind, NormalizedEventEnvelope, SessionId, TaskId},
    RunRepository,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{oneshot, watch, Mutex},
    task::JoinHandle,
    time,
};

pub const PROVIDER: &str = "oh_my_pi.omp.rpc";
pub const START_TIMEOUT: Duration = Duration::from_secs(15);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OmpInstallation {
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
pub struct OmpProgram {
    executable: PathBuf,
    digest: [u8; 32],
}
impl OmpProgram {
    pub fn from_executable(executable: impl Into<PathBuf>) -> Result<Self, OmpError> {
        let executable = executable
            .into()
            .canonicalize()
            .map_err(|_| OmpError::MissingExecutable)?;
        let digest = executable_digest(&executable).ok_or(OmpError::MissingExecutable)?;
        Ok(Self { executable, digest })
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    fn verified_executable(&self) -> Result<&Path, OmpError> {
        let canonical = self
            .executable
            .canonicalize()
            .map_err(|_| OmpError::MissingExecutable)?;
        if canonical != self.executable || executable_digest(&canonical) != Some(self.digest) {
            return Err(OmpError::MissingExecutable);
        }
        Ok(&self.executable)
    }
}
fn executable_digest(path: &Path) -> Option<[u8; 32]> {
    std::fs::metadata(path).ok()?.is_file().then_some(())?;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().into())
}
pub async fn detect_installation(executable: impl Into<PathBuf>) -> OmpInstallation {
    let executable = executable.into();
    if !executable.is_file() {
        return OmpInstallation::Missing;
    }
    match time::timeout(
        START_TIMEOUT,
        Command::new(&executable).arg("--version").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() && !bounded(&output.stdout).is_empty() => {
            OmpInstallation::Available {
                executable,
                version: bounded(&output.stdout),
            }
        }
        Ok(Ok(output)) => OmpInstallation::Unsupported {
            detail: bounded(&output.stderr),
        },
        Ok(Err(error)) => OmpInstallation::Unsupported {
            detail: redact(&error.to_string()),
        },
        Err(_) => OmpInstallation::Unsupported {
            detail: "OMP version check timed out.".into(),
        },
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum OmpError {
    #[error("OMP executable is missing or not a regular file")]
    MissingExecutable,
    #[error("OMP RPC process failed to start")]
    StartFailed,
    #[error("OMP RPC process did not become ready")]
    StartTimeout,
    #[error("OMP RPC process exited unexpectedly")]
    UnexpectedExit,
    #[error("OMP rejected the prompt command")]
    PromptRejected,
    #[error("OMP completed the prompt without invoking an agent")]
    PromptLocalCompletion,
    #[error("OMP did not acknowledge the prompt command in time")]
    PromptTimeout,
    #[error("OMP reported a late prompt scheduling error")]
    LatePromptError,
    #[error("invalid adapter input")]
    InvalidInput,
    #[error("persistence failure")]
    Storage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmpSession {
    pub session_id: SessionId,
    pub provider_session_id: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OmpPromptOutcome {
    AgentInvoked,
    LocalCompletion,
}
struct RpcState {
    pending: HashMap<String, oneshot::Sender<Result<OmpPromptOutcome, OmpError>>>,
    accepted: HashSet<String>,
    late_error: Option<OmpError>,
}
struct EventState {
    repository: RunRepository,
    task_id: TaskId,
    sequence: u64,
    session: Option<OmpSession>,
    completed: bool,
    cancelling: bool,
}

/// One OMP child, started by Sentinel. OMP's own model/auth configuration is
/// intentionally opaque; the model selector is passed as a normal OMP CLI arg.
pub struct OmpProcess {
    child: Child,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    reader: Option<JoinHandle<()>>,
    state: Arc<Mutex<EventState>>,
    rpc: Arc<Mutex<RpcState>>,
    next_request_id: AtomicU64,
    request_timeout: Duration,
    ready: watch::Receiver<bool>,
}
impl OmpProcess {
    pub async fn start(
        program: OmpProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        model: &str,
        prompt: &str,
        read_only: bool,
    ) -> Result<(Self, OmpSession), OmpError> {
        if !cwd.is_dir()
            || !valid(model)
            || prompt.trim().is_empty()
            || prompt.len() > 8_000
            || prompt.contains('\0')
        {
            return Err(OmpError::InvalidInput);
        }
        let sequence = repository
            .v3()
            .list_events(&task_id)
            .await
            .map_err(|_| OmpError::Storage)?
            .last()
            .map(|e| e.sequence_number)
            .unwrap_or(0);
        let mut command = Command::new(program.verified_executable()?);
        command
            .arg("--mode")
            .arg("rpc")
            .arg("--no-session")
            .arg("--no-lsp")
            .arg("--no-pty")
            .arg("--model")
            .arg(model);
        if read_only {
            command.arg("--tools").arg("read,grep,find");
        }
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| OmpError::StartFailed)?;
        let stdin = Arc::new(Mutex::new(Some(
            child.stdin.take().ok_or(OmpError::StartFailed)?,
        )));
        let stdout = child.stdout.take().ok_or(OmpError::StartFailed)?;
        let (ready_tx, ready) = watch::channel(false);
        let state = Arc::new(Mutex::new(EventState {
            repository,
            task_id,
            sequence,
            session: None,
            completed: false,
            cancelling: false,
        }));
        let rpc = Arc::new(Mutex::new(RpcState {
            pending: HashMap::new(),
            accepted: HashSet::new(),
            late_error: None,
        }));
        let reader = tokio::spawn(read_stdout(stdout, state.clone(), rpc.clone(), ready_tx));
        let process = Self {
            child,
            stdin,
            reader: Some(reader),
            state,
            rpc,
            next_request_id: AtomicU64::new(1),
            request_timeout: REQUEST_TIMEOUT,
            ready,
        };
        let mut receiver = process.ready.clone();
        time::timeout(START_TIMEOUT, async move {
            loop {
                if *receiver.borrow() {
                    return Ok(());
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| OmpError::UnexpectedExit)?;
            }
        })
        .await
        .map_err(|_| OmpError::StartTimeout)??;
        let (session, outcome) = process.send_prompt(prompt).await?;
        if outcome == OmpPromptOutcome::LocalCompletion {
            return Err(OmpError::PromptLocalCompletion);
        }
        Ok((process, session))
    }
    pub async fn send_prompt(
        &self,
        prompt: &str,
    ) -> Result<(OmpSession, OmpPromptOutcome), OmpError> {
        if prompt.trim().is_empty() || prompt.len() > 8_000 || prompt.contains('\0') {
            return Err(OmpError::InvalidInput);
        }
        self.ensure_healthy().await?;
        // Persist the Sentinel session before writing: an immediate OMP response
        // can otherwise race this caller's session creation.
        let session = ensure_session(self.state.clone()).await?;
        let id = format!(
            "sentinel-{}",
            self.next_request_id.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = oneshot::channel();
        self.rpc.lock().await.pending.insert(id.clone(), sender);
        let frame = json!({"id": id, "type":"prompt", "message":prompt}).to_string() + "\n";
        {
            let mut stdin = self.stdin.lock().await;
            let Some(stdin) = stdin.as_mut() else {
                self.rpc.lock().await.pending.remove(&id);
                return Err(OmpError::UnexpectedExit);
            };
            stdin
                .write_all(frame.as_bytes())
                .await
                .map_err(|_| OmpError::UnexpectedExit)?;
            stdin.flush().await.map_err(|_| OmpError::UnexpectedExit)?;
        }
        // OMP does not expose a mandatory session id in the ready frame. Sentinel
        // creates an opaque process-scoped ref before receipt of the first event.
        let outcome = match time::timeout(self.request_timeout, receiver).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => Err(OmpError::UnexpectedExit)?,
            Err(_) => {
                self.rpc.lock().await.pending.remove(&id);
                Err(OmpError::PromptTimeout)?
            }
        };
        Ok((session, outcome))
    }
    pub async fn ensure_healthy(&self) -> Result<(), OmpError> {
        self.rpc.lock().await.late_error.clone().map_or(Ok(()), Err)
    }
    pub async fn cancel(&mut self) -> Result<(), OmpError> {
        self.state.lock().await.cancelling = true;
        let frame = json!({"id":format!("abort-{}", now_ms()), "type":"abort"}).to_string() + "\n";
        if let Some(stdin) = self.stdin.lock().await.as_mut() {
            let _ = stdin.write_all(frame.as_bytes()).await;
            let _ = stdin.flush().await;
        }
        self.child
            .start_kill()
            .map_err(|_| OmpError::UnexpectedExit)?;
        self.child
            .wait()
            .await
            .map_err(|_| OmpError::UnexpectedExit)?;
        emit(
            &self.state,
            EventKind::SessionCancelled,
            json!({"reason":"owned_process_cancelled"}),
        )
        .await
    }
    pub async fn wait_for_exit(&mut self) -> Result<(), OmpError> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| OmpError::UnexpectedExit)?;
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        let state = self.state.lock().await;
        let cancelling = state.cancelling;
        let completed = state.completed;
        let session = state.session.clone();
        drop(state);
        if !cancelling && completed && status.success() {
            emit(
                &self.state,
                EventKind::SessionCompleted,
                json!({"exit_code":status.code()}),
            )
            .await?;
        } else if !cancelling {
            emit(
                &self.state,
                EventKind::SessionFailed,
                json!({"exit_code":status.code()}),
            )
            .await?;
        }
        if session.is_none() {
            return Err(OmpError::UnexpectedExit);
        }
        Ok(())
    }
    /// RPC mode exits cleanly when stdin closes. Use this after `agent_end` for
    /// short-lived review workers; long-lived implementers remain owned until
    /// cancelled by Sentinel.
    pub async fn shutdown(&mut self) -> Result<(), OmpError> {
        let stdin = self.stdin.lock().await.take();
        drop(stdin);
        self.wait_for_exit().await
    }
}
async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<EventState>>,
    rpc: Arc<Mutex<RpcState>>,
    ready: watch::Sender<bool>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_LINE_BYTES {
            let _ = emit(
                &state,
                EventKind::Unknown {
                    discriminator: "omp/oversized_line".into(),
                },
                json!({"bytes":line.len()}),
            )
            .await;
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                let _ = emit(
                    &state,
                    EventKind::Unknown {
                        discriminator: "omp/malformed_json".into(),
                    },
                    json!({}),
                )
                .await;
                continue;
            }
        };
        if value.get("type").and_then(Value::as_str) == Some("ready") {
            ready.send_replace(true);
            continue;
        }
        let ty = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("missing_type");
        let kind = normalize_event_kind(ty);
        if ty == "response" {
            settle_response(&value, &rpc).await;
        }
        if ty == "prompt_result" {
            settle_prompt_result(&value, &rpc).await;
        }
        if ty == "agent_end" {
            state.lock().await.completed = true;
        }
        let _ = ensure_session(state.clone()).await;
        let _ = emit(&state, kind, json!({"omp_event":value})).await;
    }
    let mut rpc = rpc.lock().await;
    for (_, sender) in rpc.pending.drain() {
        let _ = sender.send(Err(OmpError::UnexpectedExit));
    }
}
fn normalize_event_kind(ty: &str) -> EventKind {
    match ty {
        "agent_start" | "turn_start" | "message_start" | "message_update" | "message_end" => {
            EventKind::Message
        }
        "tool_execution_start" => EventKind::ToolStarted,
        "tool_execution_end" => EventKind::ToolCompleted,
        "agent_end" => EventKind::TurnCompleted,
        "response" => EventKind::Message,
        other => EventKind::Unknown {
            discriminator: format!("omp/{other}"),
        },
    }
}
async fn settle_response(value: &Value, rpc: &Arc<Mutex<RpcState>>) {
    if value.get("command").and_then(Value::as_str) != Some("prompt") {
        return;
    }
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return;
    };
    let success = value.get("success").and_then(Value::as_bool) == Some(true);
    let mut state = rpc.lock().await;
    if let Some(sender) = state.pending.remove(id) {
        if !success {
            let _ = sender.send(Err(OmpError::PromptRejected));
            return;
        }
        let outcome = match value.pointer("/data/agentInvoked").and_then(Value::as_bool) {
            Some(false) => OmpPromptOutcome::LocalCompletion,
            _ => OmpPromptOutcome::AgentInvoked,
        };
        if outcome == OmpPromptOutcome::AgentInvoked {
            state.accepted.insert(id.into());
        }
        let _ = sender.send(Ok(outcome));
    } else if !success && state.accepted.contains(id) {
        state.late_error = Some(OmpError::LatePromptError);
    }
}
async fn settle_prompt_result(value: &Value, rpc: &Arc<Mutex<RpcState>>) {
    if value.get("agentInvoked").and_then(Value::as_bool) != Some(false) {
        return;
    }
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return;
    };
    let mut state = rpc.lock().await;
    if let Some(sender) = state.pending.remove(id) {
        let _ = sender.send(Ok(OmpPromptOutcome::LocalCompletion));
    } else if state.accepted.contains(id) {
        state.late_error = Some(OmpError::PromptLocalCompletion);
    }
}
async fn ensure_session(state: Arc<Mutex<EventState>>) -> Result<OmpSession, OmpError> {
    if let Some(session) = state.lock().await.session.clone() {
        return Ok(session);
    }
    let (repository, task_id) = {
        let state = state.lock().await;
        (state.repository.clone(), state.task_id.clone())
    };
    let ref_id = format!("omp-rpc:{}:{}", task_id, now_ms());
    let session = repository
        .v3()
        .create_session(
            CreateSession {
                task_id,
                provider: PROVIDER.into(),
                provider_session_ref: ref_id.clone(),
            },
            now_ms(),
        )
        .await
        .map_err(|_| OmpError::Storage)?;
    let result = OmpSession {
        session_id: session.id,
        provider_session_id: ref_id,
    };
    state.lock().await.session = Some(result.clone());
    emit(
        &state,
        EventKind::SessionStarted,
        json!({"provider_session_id":result.provider_session_id}),
    )
    .await?;
    Ok(result)
}
async fn emit(
    state: &Arc<Mutex<EventState>>,
    kind: EventKind,
    payload: Value,
) -> Result<(), OmpError> {
    let mut state = state.lock().await;
    state.sequence += 1;
    let event = NormalizedEventEnvelope {
        event_id: sentinel_core::v3::EventId::new(),
        task_id: state.task_id.clone(),
        session_id: state.session.as_ref().map(|s| s.session_id.clone()),
        provider: PROVIDER.into(),
        kind,
        schema_version: 1,
        occurred_at_ms: now_ms(),
        sequence_number: state.sequence,
        causation_id: None,
        correlation_id: None,
        payload,
        raw_diagnostic_payload: None,
    };
    state
        .repository
        .v3()
        .append_event(&event)
        .await
        .map_err(|_| OmpError::Storage)
}
fn valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.contains('\0')
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}
fn bounded(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::CreateTask;

    #[test]
    fn agent_end_is_turn_completion_while_tool_end_remains_tool_completion() {
        assert_eq!(normalize_event_kind("agent_end"), EventKind::TurnCompleted);
        assert_eq!(
            normalize_event_kind("tool_execution_end"),
            EventKind::ToolCompleted
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn rpc_prompt_outcomes_are_correlated_by_id() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-omp");
        std::fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo omp-test; exit 0; fi\necho '{\"type\":\"ready\"}'\nwhile IFS= read -r line; do\n id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/p')\n case \"$line\" in\n  *initial*|*accepted*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":true,\\\"data\\\":{\\\"agentInvoked\\\":true}}\" ;;\n  *omitted*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":true}\" ;;\n  *local*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":true,\\\"data\\\":{\\\"agentInvoked\\\":false}}\" ;;\n  *result*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"prompt_result\\\",\\\"agentInvoked\\\":false}\" ;;\n  *reject*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":false}\" ;;\n  *late*) echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":true,\\\"data\\\":{\\\"agentInvoked\\\":true}}\"; sleep 0.05; echo \"{\\\"id\\\":\\\"$id\\\",\\\"type\\\":\\\"response\\\",\\\"command\\\":\\\"prompt\\\",\\\"success\\\":false}\" ;;\n  *exit*) exit 0 ;;\n  *timeout*) sleep 1 ;;\n esac\ndone\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = format!(
            "sqlite://{}",
            directory.path().join("test.sqlite").display()
        );
        let repository = RunRepository::open(&database).await.unwrap();
        let task = repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "test".into(),
                    summary: "test".into(),
                },
                1,
            )
            .await
            .unwrap();
        let (mut process, session) = OmpProcess::start(
            OmpProgram::from_executable(&executable).unwrap(),
            repository.clone(),
            task.id.clone(),
            directory.path(),
            "zai/glm-5.2",
            "initial",
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            process.send_prompt("accepted").await.unwrap().1,
            OmpPromptOutcome::AgentInvoked
        );
        assert_eq!(
            process.send_prompt("omitted").await.unwrap().1,
            OmpPromptOutcome::AgentInvoked
        );
        assert_eq!(
            process.send_prompt("local").await.unwrap().1,
            OmpPromptOutcome::LocalCompletion
        );
        assert_eq!(
            process.send_prompt("result").await.unwrap().1,
            OmpPromptOutcome::LocalCompletion
        );
        assert_eq!(
            process.send_prompt("reject").await.unwrap_err(),
            OmpError::PromptRejected
        );
        assert_eq!(
            process.send_prompt("late").await.unwrap().1,
            OmpPromptOutcome::AgentInvoked
        );
        time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            process.ensure_healthy().await.unwrap_err(),
            OmpError::LatePromptError
        );
        process.rpc.lock().await.late_error = None;
        process.request_timeout = Duration::from_millis(25);
        assert_eq!(
            process.send_prompt("timeout").await.unwrap_err(),
            OmpError::PromptTimeout
        );
        time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(
            process.send_prompt("exit").await.unwrap_err(),
            OmpError::UnexpectedExit
        );
        process.shutdown().await.unwrap();
        process.shutdown().await.unwrap();
        let events = repository.v3().list_events(&task.id).await.unwrap();
        assert!(events.iter().any(|event| event
            .payload
            .pointer("/omp_event/type")
            .and_then(Value::as_str)
            == Some("response")));
        assert!(events
            .iter()
            .any(|event| event.session_id.as_ref() == Some(&session.session_id)));
    }
}
