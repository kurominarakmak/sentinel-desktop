use sentinel_agent_api::AgentKind;
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

pub const RUN_SCHEMA_VERSION: u16 = 1;
pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_TASK_BYTES: usize = 8_000;
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 16_000;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRequest {
    pub task_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Preparing,
    Running,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        if matches!(
            (self, next),
            (Self::Queued, Self::Preparing | Self::Cancelled)
                | (
                    Self::Preparing,
                    Self::Running | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Running,
                    Self::Cancelling | Self::Completed | Self::Failed
                )
                | (Self::Cancelling, Self::Cancelled | Self::Failed)
        ) {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid transition {from:?} -> {to:?}")]
pub struct TransitionError {
    pub from: RunStatus,
    pub to: RunStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeRunError {
    pub category: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_text: String,
    pub agent: AgentKind,
    pub status: RunStatus,
    pub schema_version: u16,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub error: Option<SafeRunError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedAgentEvent {
    pub run_id: RunId,
    pub sequence_number: u64,
    pub event_type: String,
    pub schema_version: u16,
    pub occurred_at_ms: i64,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteSettings {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub busy_timeout_ms: i64,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid task")]
    InvalidTask,
    #[error("invalid event")]
    InvalidEvent,
    #[error("payload too large")]
    PayloadTooLarge,
    #[error("run not found")]
    NotFound,
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error("storage error")]
    Storage,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(i64::MAX)
        })
}

fn status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Preparing => "preparing",
        RunStatus::Running => "running",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
    }
}

fn parse_status(value: &str) -> Result<RunStatus, CoreError> {
    match value {
        "queued" => Ok(RunStatus::Queued),
        "preparing" => Ok(RunStatus::Preparing),
        "running" => Ok(RunStatus::Running),
        "cancelling" => Ok(RunStatus::Cancelling),
        "cancelled" => Ok(RunStatus::Cancelled),
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        _ => Err(CoreError::Storage),
    }
}

fn agent_name(agent: &AgentKind) -> &'static str {
    match agent {
        AgentKind::Fake => "fake",
        AgentKind::Codex => "codex",
        AgentKind::ClaudeCode => "claude_code",
    }
}

fn parse_agent(value: &str) -> Result<AgentKind, CoreError> {
    match value {
        "fake" => Ok(AgentKind::Fake),
        "codex" => Ok(AgentKind::Codex),
        "claude_code" => Ok(AgentKind::ClaudeCode),
        _ => Err(CoreError::Storage),
    }
}

/// Redacts credential values while retaining ordinary context for diagnostics.
pub fn redact(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let mut ranges = Vec::new();
    for marker in ["bearer ", "sk-"] {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(marker) {
            let start = offset + found;
            let value_start = if marker == "bearer " {
                start + marker.len()
            } else {
                start
            };
            let value_end = credential_end(message, value_start);
            ranges.push((value_start, value_end));
            offset = value_end;
        }
    }
    for marker in [
        "api_key", "api-key", "apikey", "token", "password", "secret",
    ] {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(marker) {
            let after_name = offset + found + marker.len();
            let tail = &message[after_name..];
            let trimmed = tail.trim_start();
            if let Some(separator) = trimmed
                .chars()
                .next()
                .filter(|value| matches!(value, '=' | ':'))
            {
                let prefix_bytes = tail.len() - trimmed.len() + separator.len_utf8();
                let value_tail = &tail[prefix_bytes..];
                let value_start =
                    after_name + prefix_bytes + value_tail.len() - value_tail.trim_start().len();
                if value_start < message.len() {
                    ranges.push((value_start, credential_end(message, value_start)));
                }
            }
            offset = after_name;
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    let mut result = String::with_capacity(message.len().min(512));
    let mut cursor = 0;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        result.push_str(&message[cursor..start]);
        result.push_str("[REDACTED]");
        cursor = end;
    }
    result.push_str(&message[cursor..]);
    result.chars().take(512).collect()
}

fn credential_end(message: &str, start: usize) -> usize {
    let rest = &message[start..];
    let quote = rest
        .as_bytes()
        .first()
        .copied()
        .filter(|byte| *byte == b'\'' || *byte == b'"');
    if let Some(quote) = quote {
        start
            + rest[1..]
                .find(char::from(quote))
                .map_or(rest.len(), |index| index + 2)
    } else {
        start
            + rest
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, ',' | ';' | '&')
                })
                .unwrap_or(rest.len())
    }
}

fn checked_u16(value: i64) -> Result<u16, CoreError> {
    u16::try_from(value).map_err(|_| CoreError::Storage)
}
fn checked_u64(value: i64) -> Result<u64, CoreError> {
    u64::try_from(value).map_err(|_| CoreError::Storage)
}
fn checked_i64(value: u64) -> Result<i64, CoreError> {
    i64::try_from(value).map_err(|_| CoreError::InvalidEvent)
}

fn row_to_run(row: &sqlx::sqlite::SqliteRow) -> Result<Run, CoreError> {
    let error_message = row.get::<Option<String>, _>("error_message");
    Ok(Run {
        id: RunId(Uuid::from_str(&row.get::<String, _>("id")).map_err(|_| CoreError::Storage)?),
        task_text: row.get("task_text"),
        agent: parse_agent(&row.get::<String, _>("agent_kind"))?,
        status: parse_status(&row.get::<String, _>("status"))?,
        schema_version: checked_u16(row.get("schema_version"))?,
        created_at_ms: row.get("created_at_ms"),
        started_at_ms: row.get("started_at_ms"),
        finished_at_ms: row.get("finished_at_ms"),
        exit_code: row.get("exit_code"),
        error: error_message.map(|message| SafeRunError {
            category: row.get("error_category"),
            message,
        }),
    })
}

fn validate_event(event: &NormalizedAgentEvent) -> Result<String, CoreError> {
    if event.event_type.trim().is_empty() || event.schema_version == 0 {
        return Err(CoreError::InvalidEvent);
    }
    let payload = serde_json::to_string(&event.payload).map_err(|_| CoreError::Storage)?;
    if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
        return Err(CoreError::PayloadTooLarge);
    }
    checked_i64(event.sequence_number)?;
    Ok(payload)
}

#[derive(Clone)]
pub struct RunRepository {
    pool: SqlitePool,
}

impl RunRepository {
    pub async fn open(url: &str) -> Result<Self, CoreError> {
        let options = SqliteConnectOptions::from_str(url)
            .map_err(|_| CoreError::Storage)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|_| CoreError::Storage)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        Ok(Self { pool })
    }

    pub async fn sqlite_settings(&self) -> Result<SqliteSettings, CoreError> {
        let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            != 0;
        let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        let busy_timeout_ms = sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        Ok(SqliteSettings {
            foreign_keys,
            journal_mode,
            busy_timeout_ms,
        })
    }

    pub async fn create_run(&self, request: TaskRequest) -> Result<Run, CoreError> {
        if request.task_text.trim().is_empty() || request.task_text.len() > MAX_TASK_BYTES {
            return Err(CoreError::InvalidTask);
        }
        let run = Run {
            id: RunId::new(),
            task_text: request.task_text,
            agent: AgentKind::Fake,
            status: RunStatus::Queued,
            schema_version: RUN_SCHEMA_VERSION,
            created_at_ms: now(),
            started_at_ms: None,
            finished_at_ms: None,
            exit_code: None,
            error: None,
        };
        sqlx::query("INSERT INTO runs (id, task_text, agent_kind, status, schema_version, created_at_ms) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(run.id.to_string()).bind(&run.task_text).bind(agent_name(&run.agent)).bind(status_name(run.status)).bind(i64::from(run.schema_version)).bind(run.created_at_ms)
            .execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        Ok(run)
    }

    pub async fn get_run(&self, id: &RunId) -> Result<Run, CoreError> {
        let row = sqlx::query("SELECT id, task_text, agent_kind, status, schema_version, created_at_ms, started_at_ms, finished_at_ms, exit_code, error_category, error_message FROM runs WHERE id = ?")
            .bind(id.to_string()).fetch_optional(&self.pool).await.map_err(|_| CoreError::Storage)?.ok_or(CoreError::NotFound)?;
        row_to_run(&row)
    }

    pub async fn list_recent_runs(&self) -> Result<Vec<Run>, CoreError> {
        let rows = sqlx::query("SELECT id, task_text, agent_kind, status, schema_version, created_at_ms, started_at_ms, finished_at_ms, exit_code, error_category, error_message FROM runs ORDER BY created_at_ms DESC, id DESC")
            .fetch_all(&self.pool).await.map_err(|_| CoreError::Storage)?;
        rows.iter().map(row_to_run).collect()
    }

    pub async fn transition(
        &self,
        id: &RunId,
        next: RunStatus,
        error: Option<SafeRunError>,
    ) -> Result<Run, CoreError> {
        self.transition_inner(id, next, error, None).await
    }

    /// Atomically validates and persists a run state change plus one normalized event.
    pub async fn transition_with_event(
        &self,
        id: &RunId,
        next: RunStatus,
        error: Option<SafeRunError>,
        event: &NormalizedAgentEvent,
    ) -> Result<Run, CoreError> {
        if event.run_id != *id {
            return Err(CoreError::InvalidEvent);
        }
        let payload = validate_event(event)?;
        self.transition_inner(id, next, error, Some((event, payload)))
            .await
    }

    async fn transition_inner(
        &self,
        id: &RunId,
        next: RunStatus,
        error: Option<SafeRunError>,
        event: Option<(&NormalizedAgentEvent, String)>,
    ) -> Result<Run, CoreError> {
        let mut transaction = self.pool.begin().await.map_err(|_| CoreError::Storage)?;
        let row = sqlx::query("SELECT id, task_text, agent_kind, status, schema_version, created_at_ms, started_at_ms, finished_at_ms, exit_code, error_category, error_message FROM runs WHERE id = ?")
            .bind(id.to_string()).fetch_optional(&mut *transaction).await.map_err(|_| CoreError::Storage)?.ok_or(CoreError::NotFound)?;
        let old = row_to_run(&row)?;
        old.status.transition(next)?;
        let timestamp = now();
        let started_at = if next == RunStatus::Running && old.started_at_ms.is_none() {
            Some(timestamp)
        } else {
            old.started_at_ms
        };
        let finished_at = if next.terminal() {
            Some(timestamp)
        } else {
            old.finished_at_ms
        };
        let safe_error = error.map(|value| SafeRunError {
            category: redact(&value.category),
            message: redact(&value.message),
        });
        sqlx::query("UPDATE runs SET status = ?, started_at_ms = ?, finished_at_ms = ?, error_category = ?, error_message = ? WHERE id = ?")
            .bind(status_name(next)).bind(started_at).bind(finished_at).bind(safe_error.as_ref().map(|value| &value.category)).bind(safe_error.as_ref().map(|value| &value.message)).bind(id.to_string())
            .execute(&mut *transaction).await.map_err(|_| CoreError::Storage)?;
        if let Some((event, payload)) = event {
            sqlx::query("INSERT INTO run_events (run_id, sequence_number, event_type, schema_version, occurred_at_ms, payload_json) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(event.run_id.to_string()).bind(checked_i64(event.sequence_number)?).bind(&event.event_type).bind(i64::from(event.schema_version)).bind(event.occurred_at_ms).bind(payload)
                .execute(&mut *transaction).await.map_err(|_| CoreError::Storage)?;
        }
        transaction.commit().await.map_err(|_| CoreError::Storage)?;
        self.get_run(id).await
    }

    pub async fn append_event(&self, event: &NormalizedAgentEvent) -> Result<(), CoreError> {
        let payload = validate_event(event)?;
        sqlx::query("INSERT INTO run_events (run_id, sequence_number, event_type, schema_version, occurred_at_ms, payload_json) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(event.run_id.to_string()).bind(checked_i64(event.sequence_number)?).bind(&event.event_type).bind(i64::from(event.schema_version)).bind(event.occurred_at_ms).bind(payload)
            .execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        Ok(())
    }

    pub async fn list_events(&self, id: &RunId) -> Result<Vec<NormalizedAgentEvent>, CoreError> {
        let rows = sqlx::query("SELECT sequence_number, event_type, schema_version, occurred_at_ms, payload_json FROM run_events WHERE run_id = ? ORDER BY sequence_number ASC")
            .bind(id.to_string()).fetch_all(&self.pool).await.map_err(|_| CoreError::Storage)?;
        rows.iter()
            .map(|row| {
                Ok(NormalizedAgentEvent {
                    run_id: id.clone(),
                    sequence_number: checked_u64(row.get("sequence_number"))?,
                    event_type: row.get("event_type"),
                    schema_version: checked_u16(row.get("schema_version"))?,
                    occurred_at_ms: row.get("occurred_at_ms"),
                    payload: serde_json::from_str(&row.get::<String, _>("payload_json"))
                        .map_err(|_| CoreError::Storage)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_machine_accepts_phase_two_lifecycle() {
        assert_eq!(
            RunStatus::Queued.transition(RunStatus::Preparing),
            Ok(RunStatus::Preparing)
        );
        assert_eq!(
            RunStatus::Preparing.transition(RunStatus::Running),
            Ok(RunStatus::Running)
        );
        assert_eq!(
            RunStatus::Running.transition(RunStatus::Completed),
            Ok(RunStatus::Completed)
        );
    }
    #[test]
    fn terminal_and_invalid_transitions_are_rejected() {
        assert!(RunStatus::Queued.transition(RunStatus::Completed).is_err());
        assert!(RunStatus::Cancelled.transition(RunStatus::Running).is_err());
    }
    #[test]
    fn redaction_preserves_ordinary_text() {
        assert_eq!(redact("token refresh failed"), "token refresh failed");
        assert_eq!(redact("secret note: keep this"), "secret note: keep this");
        assert!(redact("Authorization: Bearer abc.def").contains("[REDACTED]"));
    }
}
