//! Sentinel-owned adapter for Claude Code's supported non-interactive JSONL mode.
//!
//! It deliberately consumes only `claude --print --output-format stream-json`.
//! Sentinel persists the opaque CLI `session_id`; it never reads transcripts or
//! discovers provider sessions outside the durable V3 task record.

use sentinel_core::{
    redact,
    v3::{CreateSession, EventKind, NormalizedEventEnvelope, SessionId, TaskId},
    CoreError, RunRepository,
};
use sentinel_provider_api::{
    ModelDiscoveryKind, ProviderError, ProviderFuture, ProviderId, ProviderModel,
    ProviderModelAvailability, ProviderModelCatalog, ProviderModelDiscovery, CLAUDE_PROVIDER_ID,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{watch, Mutex},
    task::JoinHandle,
    time,
};

pub const PROVIDER: &str = "anthropic.claude_code.stream_json";
pub const START_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeInstallation {
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
pub struct ClaudeProgram {
    executable: PathBuf,
    digest: [u8; 32],
}
impl ClaudeProgram {
    pub fn from_executable(executable: impl Into<PathBuf>) -> Result<Self, ClaudeError> {
        let executable = executable
            .into()
            .canonicalize()
            .map_err(|_| ClaudeError::MissingExecutable)?;
        let digest = executable_digest(&executable).ok_or(ClaudeError::MissingExecutable)?;
        Ok(Self { executable, digest })
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    fn verified_executable(&self) -> Result<&Path, ClaudeError> {
        let canonical = self
            .executable
            .canonicalize()
            .map_err(|_| ClaudeError::MissingExecutable)?;
        if canonical != self.executable || executable_digest(&canonical) != Some(self.digest) {
            return Err(ClaudeError::MissingExecutable);
        }
        Ok(&self.executable)
    }
}

/// Claude Code does not expose a supported model enumeration endpoint. This
/// adapter verifies the installed CLI and honestly exposes its CLI-owned
/// current/default configuration instead of fabricating a Claude catalog.
#[derive(Clone)]
pub struct ClaudeModelDiscovery {
    program: ClaudeProgram,
    provider_id: ProviderId,
}

impl ClaudeModelDiscovery {
    pub fn new(program: ClaudeProgram) -> Self {
        Self {
            program,
            provider_id: ProviderId::new(CLAUDE_PROVIDER_ID).expect("constant provider ID"),
        }
    }
}

impl ProviderModelDiscovery for ClaudeModelDiscovery {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    fn discover_models(&self) -> ProviderFuture<'_, ProviderModelCatalog> {
        Box::pin(async move {
            let executable = self
                .program
                .verified_executable()
                .map_err(|_| ProviderError::Unavailable("Claude Code is unavailable".into()))?;
            let output = time::timeout(
                START_TIMEOUT,
                Command::new(executable)
                    .arg("auth")
                    .arg("status")
                    .arg("--json")
                    .stdin(Stdio::null())
                    .output(),
            )
            .await
            .map_err(|_| ProviderError::Timeout)?
            .map_err(|_| ProviderError::Unavailable("Claude Code is unavailable".into()))?;
            let status = serde_json::from_slice::<Value>(&output.stdout);
            if status.as_ref().ok().and_then(|value| value.get("loggedIn"))
                == Some(&Value::Bool(false))
            {
                return Err(ProviderError::Authentication);
            }
            if !output.status.success() {
                return Err(ProviderError::Unavailable(
                    "Claude Code is unavailable".into(),
                ));
            }
            let status = status.map_err(|_| ProviderError::MalformedResponse)?;
            if status.get("loggedIn").and_then(Value::as_bool) != Some(true) {
                return Err(ProviderError::MalformedResponse);
            }
            Ok(ProviderModelCatalog {
                provider_id: self.provider_id.clone(),
                discovery_kind: ModelDiscoveryKind::CurrentConfiguration,
                models: vec![ProviderModel {
                    provider_id: self.provider_id.clone(),
                    model_id: "cli-owned".into(),
                    display_name: Some("Default / currently configured model".into()),
                    supported_reasoning_efforts: Vec::new(),
                    default_reasoning_effort: None,
                    is_default: true,
                    availability: ProviderModelAvailability::Available,
                }],
            })
        })
    }
}

fn executable_digest(path: &Path) -> Option<[u8; 32]> {
    std::fs::metadata(path).ok()?.is_file().then_some(())?;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Some(hasher.finalize().into())
}
pub async fn detect_installation(executable: impl Into<PathBuf>) -> ClaudeInstallation {
    let executable = executable.into();
    if !executable.is_file() {
        return ClaudeInstallation::Missing;
    }
    match time::timeout(
        START_TIMEOUT,
        Command::new(&executable).arg("--version").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() && !bounded(&output.stdout).is_empty() => {
            ClaudeInstallation::Available {
                executable,
                version: bounded(&output.stdout),
            }
        }
        Ok(Ok(output)) => ClaudeInstallation::Unsupported {
            detail: bounded(&output.stderr),
        },
        Ok(Err(error)) => ClaudeInstallation::Unsupported {
            detail: redact(&error.to_string()),
        },
        Err(_) => ClaudeInstallation::Unsupported {
            detail: "Claude version check timed out.".into(),
        },
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ClaudeError {
    #[error("Claude executable is missing or not a regular file")]
    MissingExecutable,
    #[error("Claude failed to start")]
    StartFailed,
    #[error("Claude session did not initialize")]
    StartTimeout,
    #[error("invalid adapter input")]
    InvalidInput,
    #[error("Claude exited before initializing a session")]
    UnexpectedExit,
    #[error("persistence failure")]
    Storage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeSession {
    pub session_id: SessionId,
    pub provider_session_id: String,
}

struct EventState {
    repository: RunRepository,
    task_id: TaskId,
    sequence: u64,
    session: Option<ClaudeSession>,
    cancelling: bool,
    completed: bool,
}

/// A single Claude process that Sentinel started and is therefore allowed to stop.
pub struct ClaudeProcess {
    child: Child,
    reader: Option<JoinHandle<()>>,
    state: Arc<Mutex<EventState>>,
    initialized: watch::Receiver<Option<ClaudeSession>>,
}
impl ClaudeProcess {
    pub async fn start(
        program: ClaudeProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        prompt: &str,
    ) -> Result<(Self, ClaudeSession), ClaudeError> {
        Self::spawn(program, repository, task_id, cwd, prompt, None, false).await
    }
    /// Runs Claude as a reviewer with tools disabled in an isolated cwd.
    pub async fn start_read_only(
        program: ClaudeProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        prompt: &str,
    ) -> Result<(Self, ClaudeSession), ClaudeError> {
        Self::spawn(program, repository, task_id, cwd, prompt, None, true).await
    }
    pub async fn resume(
        program: ClaudeProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        provider_session_id: &str,
        prompt: &str,
    ) -> Result<(Self, ClaudeSession), ClaudeError> {
        valid(provider_session_id)?;
        Self::spawn(
            program,
            repository,
            task_id,
            cwd,
            prompt,
            Some(provider_session_id),
            false,
        )
        .await
    }
    async fn spawn(
        program: ClaudeProgram,
        repository: RunRepository,
        task_id: TaskId,
        cwd: &Path,
        prompt: &str,
        resume: Option<&str>,
        read_only: bool,
    ) -> Result<(Self, ClaudeSession), ClaudeError> {
        if !cwd.is_dir() || prompt.trim().is_empty() || prompt.len() > 8000 || prompt.contains('\0')
        {
            return Err(ClaudeError::InvalidInput);
        }
        let sequence = repository
            .v3()
            .list_events(&task_id)
            .await
            .map_err(|_| ClaudeError::Storage)?
            .last()
            .map(|event| event.sequence_number)
            .unwrap_or(0);
        let mut command = Command::new(program.verified_executable()?);
        command
            .arg("--print")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
        if let Some(id) = resume {
            command.arg("--resume").arg(id);
        }
        if read_only {
            command.arg("--tools").arg("");
        }
        command
            .arg(prompt)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| ClaudeError::StartFailed)?;
        let stdout = child.stdout.take().ok_or(ClaudeError::StartFailed)?;
        let (initialized_tx, initialized) = watch::channel(None);
        let state = Arc::new(Mutex::new(EventState {
            repository,
            task_id,
            sequence,
            session: None,
            cancelling: false,
            completed: false,
        }));
        if let Some(provider_session_id) = resume {
            let session = ensure_session(
                state.clone(),
                provider_session_id,
                EventKind::SessionResumed,
            )
            .await?;
            initialized_tx.send_replace(Some(session));
        }
        let reader = tokio::spawn(read_stdout(
            stdout,
            state.clone(),
            initialized_tx,
            resume.is_some(),
        ));
        let process = Self {
            child,
            reader: Some(reader),
            state,
            initialized,
        };
        let mut receiver = process.initialized.clone();
        let session = time::timeout(START_TIMEOUT, async move {
            loop {
                if let Some(session) = receiver.borrow().clone() {
                    return Ok(session);
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| ClaudeError::UnexpectedExit)?;
            }
        })
        .await
        .map_err(|_| ClaudeError::StartTimeout)??;
        Ok((process, session))
    }
    pub async fn cancel(&mut self) -> Result<(), ClaudeError> {
        self.state.lock().await.cancelling = true;
        self.child
            .start_kill()
            .map_err(|_| ClaudeError::UnexpectedExit)?;
        let _ = self
            .child
            .wait()
            .await
            .map_err(|_| ClaudeError::UnexpectedExit)?;
        emit(
            &self.state,
            EventKind::SessionCancelled,
            json!({"reason":"owned_process_cancelled"}),
        )
        .await
    }
    pub async fn wait_for_exit(&mut self) -> Result<(), ClaudeError> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| ClaudeError::UnexpectedExit)?;
        // stdout can still contain the terminal stream-json `result` after the
        // child exits. Drain it before deriving a session outcome.
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        let state = self.state.lock().await;
        let cancelling = state.cancelling;
        let completed = state.completed;
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
        Ok(())
    }
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub async fn reconcile_persisted_sessions(
        repository: &RunRepository,
        task_id: &TaskId,
    ) -> Result<Vec<ClaudeSession>, ClaudeError> {
        repository
            .v3()
            .list_sessions_for_task(task_id)
            .await
            .map_err(|_| ClaudeError::Storage)
            .map(|sessions| {
                sessions
                    .into_iter()
                    .filter(|session| session.provider == PROVIDER)
                    .map(|session| ClaudeSession {
                        session_id: session.id,
                        provider_session_id: session.provider_session_ref,
                    })
                    .collect()
            })
    }
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<EventState>>,
    initialized: watch::Sender<Option<ClaudeSession>>,
    resumed: bool,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_LINE_BYTES {
            let _ = emit(
                &state,
                EventKind::Unknown {
                    discriminator: "claude/oversized_line".into(),
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
                        discriminator: "claude/malformed_json".into(),
                    },
                    json!({}),
                )
                .await;
                continue;
            }
        };
        if value.get("type").and_then(Value::as_str) == Some("system")
            && value.get("subtype").and_then(Value::as_str) == Some("init")
        {
            if let Some(id) = value
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            {
                match ensure_session(
                    state.clone(),
                    id,
                    if resumed {
                        EventKind::SessionResumed
                    } else {
                        EventKind::SessionStarted
                    },
                )
                .await
                {
                    Ok(session) => {
                        initialized.send_replace(Some(session));
                    }
                    Err(_) => break,
                }
            }
        }
        let kind = match value.get("type").and_then(Value::as_str) {
            Some("assistant") | Some("stream_event") => EventKind::Message,
            Some("tool_use") => EventKind::ToolStarted,
            Some("result") => EventKind::TurnCompleted,
            Some(other) => EventKind::Unknown {
                discriminator: format!("claude/{other}"),
            },
            None => EventKind::Unknown {
                discriminator: "claude/missing_type".into(),
            },
        };
        if value.get("type").and_then(Value::as_str) == Some("result") {
            state.lock().await.completed = true;
        }
        let _ = emit(&state, kind, json!({"claude_event":value})).await;
    }
}
async fn ensure_session(
    state: Arc<Mutex<EventState>>,
    provider_session_id: &str,
    kind: EventKind,
) -> Result<ClaudeSession, ClaudeError> {
    if let Some(session) = state
        .lock()
        .await
        .session
        .as_ref()
        .filter(|session| session.provider_session_id == provider_session_id)
        .cloned()
    {
        return Ok(session);
    }
    let (repository, task_id) = {
        let state = state.lock().await;
        (state.repository.clone(), state.task_id.clone())
    };
    let session = match repository
        .v3()
        .get_session_by_provider_reference(&task_id, PROVIDER, provider_session_id)
        .await
    {
        Ok(session) => session,
        Err(CoreError::NotFound) => repository
            .v3()
            .create_session(
                CreateSession {
                    task_id,
                    provider: PROVIDER.into(),
                    provider_session_ref: provider_session_id.into(),
                },
                now_ms(),
            )
            .await
            .map_err(|_| ClaudeError::Storage)?,
        Err(_) => return Err(ClaudeError::Storage),
    };
    let result = ClaudeSession {
        session_id: session.id,
        provider_session_id: provider_session_id.into(),
    };
    {
        state.lock().await.session = Some(result.clone());
    }
    emit(
        &state,
        kind,
        json!({"provider_session_id":provider_session_id}),
    )
    .await?;
    Ok(result)
}
async fn emit(
    state: &Arc<Mutex<EventState>>,
    kind: EventKind,
    payload: Value,
) -> Result<(), ClaudeError> {
    // Allocation and insertion share one lock: stdout and cancellation can race.
    let mut state = state.lock().await;
    state.sequence += 1;
    let event = NormalizedEventEnvelope {
        event_id: sentinel_core::v3::EventId::new(),
        task_id: state.task_id.clone(),
        session_id: state
            .session
            .as_ref()
            .map(|session| session.session_id.clone()),
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
        .map_err(|_| ClaudeError::Storage)
}
fn valid(value: &str) -> Result<(), ClaudeError> {
    (!value.trim().is_empty() && value.len() <= 512 && !value.contains('\0'))
        .then_some(())
        .ok_or(ClaudeError::InvalidInput)
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
mod discovery_tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn unavailable_enumeration_is_represented_as_verified_current_configuration() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-claude");
        std::fs::write(&executable, "#!/bin/sh\necho '{\"loggedIn\":true}'\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let discovery =
            ClaudeModelDiscovery::new(ClaudeProgram::from_executable(&executable).unwrap());
        let catalog = discovery.discover_models().await.unwrap();
        assert_eq!(
            catalog.discovery_kind,
            ModelDiscoveryKind::CurrentConfiguration
        );
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].model_id, "cli-owned");
        assert!(catalog.models[0].supported_reasoning_efforts.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn discovery_failure_surfaces_provider_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-claude");
        std::fs::write(&executable, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let discovery =
            ClaudeModelDiscovery::new(ClaudeProgram::from_executable(&executable).unwrap());
        assert!(matches!(
            discovery.discover_models().await,
            Err(ProviderError::Unavailable(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn structured_logged_out_status_is_authentication_even_with_nonzero_exit() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-claude");
        std::fs::write(
            &executable,
            "#!/bin/sh\necho '{\"loggedIn\":false,\"authMethod\":\"none\"}'\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let discovery =
            ClaudeModelDiscovery::new(ClaudeProgram::from_executable(&executable).unwrap());
        assert_eq!(
            discovery.discover_models().await,
            Err(ProviderError::Authentication)
        );
    }
}
