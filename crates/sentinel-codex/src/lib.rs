//! Supported Codex App Server transport owned by the Sentinel supervisor.
//!
//! This crate owns only one child it started. It never changes workflow state,
//! grants approvals, or uses transcript text as a completion signal.

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
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
    time,
};

pub const PROVIDER: &str = "openai.codex.app_server";
pub const MAX_JSON_LINE_BYTES: usize = 16 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;
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

#[derive(Debug, Error, PartialEq, Eq)]
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
    #[error("Codex thread or turn identifier is missing")]
    MissingIdentifier,
    #[error("invalid adapter input")]
    InvalidInput,
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

enum Incoming {
    Line(Vec<u8>),
    Closed,
}

/// A direct JSONL connection to exactly one child started by this object.
pub struct CodexAppServer {
    child: Child,
    stdin: ChildStdin,
    incoming: mpsc::Receiver<Incoming>,
    next_request_id: u64,
    seen_notifications: HashSet<String>,
    sessions_by_thread: HashMap<String, SessionId>,
    pending_turn_thread: Option<String>,
    buffered_turn_notifications: Vec<Value>,
    repository: RunRepository,
    task_id: TaskId,
    sequence: u64,
    timeouts: CodexTimeouts,
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
        let mut command = Command::new(program.executable());
        command
            .arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| CodexError::StartFailed)?;
        let stdin = child.stdin.take().ok_or(CodexError::StartFailed)?;
        let stdout = child.stdout.take().ok_or(CodexError::StartFailed)?;
        let (sender, incoming) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match reader.read_until(b'\n', &mut bytes).await {
                    Ok(0) => {
                        let _ = sender.send(Incoming::Closed).await;
                        break;
                    }
                    Ok(_) if bytes.len() > MAX_JSON_LINE_BYTES => {
                        let _ = sender.send(Incoming::Line(Vec::new())).await;
                    }
                    Ok(_) => {
                        let _ = sender.send(Incoming::Line(bytes)).await;
                    }
                    Err(_) => {
                        let _ = sender.send(Incoming::Closed).await;
                        break;
                    }
                }
            }
        });
        let mut server = Self {
            child,
            stdin,
            incoming,
            next_request_id: 1,
            seen_notifications: HashSet::new(),
            sessions_by_thread: HashMap::new(),
            pending_turn_thread: None,
            buffered_turn_notifications: Vec::new(),
            repository,
            task_id,
            sequence: 0,
            timeouts,
        };
        let initialize = json!({"clientInfo":{"name":"agent-sentinel","title":"Agent Sentinel","version":"3"},"capabilities":{}});
        if let Err(error) = time::timeout(
            server.timeouts.startup,
            server.request("initialize", initialize),
        )
        .await
        .map_err(|_| CodexError::Timeout)
        .and_then(|value| value)
        {
            let _ = server.cleanup_owned_child().await;
            return Err(error);
        }
        server.notify("initialized", json!({})).await?;
        Ok(server)
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
    pub async fn start_thread(&mut self, cwd: &Path) -> Result<CodexSession, CodexError> {
        let result = self.request("thread/start", json!({"cwd": cwd})).await?;
        let thread_id = identifier(&result, &["thread.id", "thread_id", "id"])?;
        let session = match self
            .repository
            .v3()
            .get_session_by_provider_reference(&self.task_id, PROVIDER, &thread_id)
            .await
        {
            Ok(session) => session,
            Err(CoreError::NotFound) => self
                .repository
                .v3()
                .create_session(
                    CreateSession {
                        task_id: self.task_id.clone(),
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
            EventKind::SessionStarted,
            Some(session.id.clone()),
            json!({"thread_id":thread_id}),
            None,
        )
        .await?;
        self.sessions_by_thread
            .insert(thread_id.clone(), session.id.clone());
        Ok(CodexSession {
            session_id: session.id,
            thread_id,
        })
    }
    pub async fn resume_thread(&mut self, thread_id: &str) -> Result<CodexSession, CodexError> {
        valid_id(thread_id)?;
        let result = self
            .request("thread/resume", json!({"threadId":thread_id}))
            .await?;
        let thread_id = identifier(&result, &["thread.id", "thread_id", "id"])
            .unwrap_or_else(|_| thread_id.into());
        let session = match self
            .repository
            .v3()
            .get_session_by_provider_reference(&self.task_id, PROVIDER, &thread_id)
            .await
        {
            Ok(session) => session,
            Err(CoreError::NotFound) => self
                .repository
                .v3()
                .create_session(
                    CreateSession {
                        task_id: self.task_id.clone(),
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
            EventKind::SessionResumed,
            Some(session.id.clone()),
            json!({"thread_id":thread_id}),
            None,
        )
        .await?;
        self.sessions_by_thread
            .insert(thread_id.clone(), session.id.clone());
        Ok(CodexSession {
            session_id: session.id,
            thread_id,
        })
    }
    pub async fn start_turn(
        &mut self,
        session: &CodexSession,
        prompt: &str,
    ) -> Result<CodexTurn, CodexError> {
        if prompt.trim().is_empty() || prompt.len() > 8000 || prompt.contains('\0') {
            return Err(CodexError::InvalidInput);
        }
        self.pending_turn_thread = Some(session.thread_id.clone());
        self.buffered_turn_notifications.clear();
        let result = self
            .request(
                "turn/start",
                json!({"threadId":session.thread_id,"input":[{"type":"text","text":prompt}]}),
            )
            .await;
        let result = match result {
            Ok(value) => value,
            Err(error) => {
                self.clear_turn_buffer();
                return Err(error);
            }
        };
        let turn_id = identifier(&result, &["turn.id", "turn_id", "id"])?;
        self.emit(
            EventKind::ToolStarted,
            Some(session.session_id.clone()),
            json!({"thread_id":session.thread_id,"turn_id":turn_id,"event":"turn_started"}),
            None,
        )
        .await?;
        let buffered = std::mem::take(&mut self.buffered_turn_notifications);
        self.pending_turn_thread = None;
        for notification in buffered {
            self.handle_notification(&notification).await?;
        }
        Ok(CodexTurn { turn_id })
    }
    pub async fn interrupt_turn(
        &mut self,
        session: &CodexSession,
        turn: &CodexTurn,
    ) -> Result<(), CodexError> {
        valid_id(&turn.turn_id)?;
        let interrupt = time::timeout(
            self.timeouts.interrupt,
            self.request(
                "turn/interrupt",
                json!({"threadId":session.thread_id,"turnId":turn.turn_id}),
            ),
        )
        .await
        .map_err(|_| CodexError::Timeout)?;
        interrupt?;
        self.emit(
            EventKind::SessionCancelled,
            Some(session.session_id.clone()),
            json!({"turn_id":turn.turn_id}),
            None,
        )
        .await
    }
    pub async fn shutdown(&mut self) -> Result<(), CodexError> {
        self.clear_turn_buffer();
        let _ = self.stdin.shutdown().await;
        self.clear_turn_buffer();
        match time::timeout(self.timeouts.shutdown, self.child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(CodexError::UnexpectedExit),
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .map_err(|_| CodexError::UnexpectedExit)?;
                Ok(())
            }
        }
    }
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), CodexError> {
        self.write(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, CodexError> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let deadline = time::Instant::now() + self.timeouts.request;
        loop {
            let remaining = deadline
                .checked_duration_since(time::Instant::now())
                .ok_or(CodexError::Timeout)?;
            let incoming = time::timeout(remaining, self.incoming.recv())
                .await
                .map_err(|_| CodexError::Timeout)?
                .ok_or(CodexError::UnexpectedExit)?;
            let value = match incoming {
                Incoming::Closed => return Err(CodexError::UnexpectedExit),
                Incoming::Line(bytes) => parse_message(&bytes)?,
            };
            if value.get("method").is_some() {
                self.handle_notification(&value).await?;
                continue;
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(CodexError::UnexpectedResponse);
            }
            if value.get("error").is_some() {
                return Err(CodexError::RpcError);
            }
            return value
                .get("result")
                .cloned()
                .ok_or(CodexError::MalformedMessage);
        }
    }
    async fn cleanup_owned_child(&mut self) -> Result<(), CodexError> {
        self.clear_turn_buffer();
        let _ = self.stdin.shutdown().await;
        if self.child.id().is_some() {
            self.child
                .kill()
                .await
                .map_err(|_| CodexError::UnexpectedExit)?;
        }
        Ok(())
    }
    async fn write(&mut self, value: Value) -> Result<(), CodexError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| CodexError::MalformedMessage)?;
        if encoded.len() > MAX_JSON_LINE_BYTES {
            return Err(CodexError::InvalidInput);
        }
        self.stdin
            .write_all(&encoded)
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|_| CodexError::UnexpectedExit)?;
        self.stdin
            .flush()
            .await
            .map_err(|_| CodexError::UnexpectedExit)
    }
    async fn handle_notification(&mut self, value: &Value) -> Result<(), CodexError> {
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .ok_or(CodexError::MalformedMessage)?;
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let thread_id = params.get("threadId").and_then(Value::as_str).or_else(|| {
            params
                .get("thread")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
        });
        if self.pending_turn_thread.as_deref() == thread_id && !matches!(method, "turn/started") {
            self.buffered_turn_notifications.push(value.clone());
            return Ok(());
        }
        let provider_id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{method}:{}",
                    serde_json::to_string(&params).unwrap_or_default()
                )
            });
        if !self.seen_notifications.insert(provider_id.clone()) {
            return Ok(());
        }
        let kind = match method {
            "turn/started" => EventKind::ToolStarted,
            "item/agentMessage/delta" => EventKind::Message,
            "item/started" | "item/updated" => EventKind::ToolStarted,
            "item/completed" => EventKind::ToolCompleted,
            "thread/updated" => EventKind::FileChanged,
            // Provider completion is recorded as activity only; it never completes a Sentinel task.
            "turn/completed" => EventKind::ToolCompleted,
            "turn/failed" => EventKind::SessionFailed,
            "turn/cancelled" => EventKind::SessionCancelled,
            "approval/requested" => EventKind::ApprovalRequested,
            _ => EventKind::Unknown {
                discriminator: method.into(),
            },
        };
        self.emit(
            kind,
            thread_id.and_then(|id| self.sessions_by_thread.get(id).cloned()),
            json!({"provider_event_id":provider_id,"method":method,"thread_id":thread_id,"turn_id":params.get("turnId"),"item_id":params.get("item").and_then(|item|item.get("id")),"params":bounded_value(params)}),
            Some(json!({"method":method})),
        )
        .await
    }
    fn clear_turn_buffer(&mut self) {
        self.pending_turn_thread = None;
        self.buffered_turn_notifications.clear();
    }
    async fn emit(
        &mut self,
        kind: EventKind,
        session_id: Option<SessionId>,
        payload: Value,
        raw: Option<Value>,
    ) -> Result<(), CodexError> {
        self.sequence += 1;
        self.repository
            .v3()
            .append_event(&NormalizedEventEnvelope {
                event_id: EventId::new(),
                task_id: self.task_id.clone(),
                session_id,
                provider: PROVIDER.into(),
                kind,
                schema_version: 1,
                occurred_at_ms: now_ms(),
                sequence_number: self.sequence,
                causation_id: None,
                correlation_id: None,
                payload: bounded_value(payload),
                raw_diagnostic_payload: raw.map(bounded_value),
            })
            .await
            .map_err(|_| CodexError::Storage)
    }
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
        json!({"truncated":true,"diagnostic":redact(&text[..MAX_DIAGNOSTIC_BYTES])})
    }
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis() as i64)
        .unwrap_or(0)
}
