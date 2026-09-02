//! Provider-neutral, supervisor-owned V3 workflow records.
//!
//! Adapters may supply envelopes to a supervisor, but only supervisor code is
//! intended to call the transition methods in this module.

use super::{CoreError, SqlitePool};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, Transaction};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::broadcast;
use uuid::Uuid;

macro_rules! v3_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

v3_id!(TaskId);
v3_id!(SessionId);
v3_id!(ApprovalId);
v3_id!(ValidationResultId);
v3_id!(ReviewFindingId);
v3_id!(ReviewGenerationId);
v3_id!(RepairRoundId);
v3_id!(ArtifactId);
v3_id!(EventId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycle {
    Draft,
    Preparing,
    Implementing,
    AwaitingApproval,
    Validating,
    Reviewing,
    Repairing,
    ReadyForHuman,
    DiscardPending,
    Recovering,
    Blocked,
    Finalized,
    Integrating,
    Integrated,
    /// Direct-edit implementation completed in the user's primary checkout.
    /// This never represents a review, approval, or integration boundary.
    Completed,
    Cancelled,
    Failed,
}
impl TaskLifecycle {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Blocked
                | Self::Finalized
                | Self::Integrated
                | Self::Completed
                | Self::Cancelled
                | Self::Failed
        )
    }
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        let valid = matches!(
            (self, next),
            (Self::Draft, Self::Preparing | Self::Cancelled)
                | (
                    Self::Preparing,
                    Self::Implementing
                        | Self::Blocked
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::Implementing,
                    Self::AwaitingApproval
                        | Self::Validating
                        | Self::Completed
                        | Self::Blocked
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::AwaitingApproval,
                    Self::Implementing | Self::Blocked | Self::Cancelled | Self::Recovering
                )
                | (
                    Self::Validating,
                    Self::Reviewing
                        | Self::Completed
                        | Self::Repairing
                        | Self::ReadyForHuman
                        | Self::Blocked
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::Reviewing,
                    Self::Repairing
                        | Self::ReadyForHuman
                        | Self::Blocked
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::Repairing,
                    Self::Validating
                        | Self::AwaitingApproval
                        | Self::Blocked
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::ReadyForHuman,
                    Self::Finalized
                        | Self::Integrating
                        | Self::DiscardPending
                        | Self::Blocked
                        | Self::Recovering
                )
                | (
                    Self::Integrating,
                    Self::Integrated | Self::Blocked | Self::Recovering
                )
                | (
                    Self::DiscardPending,
                    Self::Finalized
                        | Self::ReadyForHuman
                        | Self::Integrating
                        | Self::Blocked
                        | Self::Cancelled
                        | Self::Recovering
                )
                | (
                    Self::Recovering,
                    Self::Preparing
                        | Self::Implementing
                        | Self::AwaitingApproval
                        | Self::Validating
                        | Self::Reviewing
                        | Self::Repairing
                        | Self::ReadyForHuman
                        | Self::Blocked
                        | Self::Cancelled
                        | Self::Failed
                )
                | (Self::Blocked, Self::Reviewing)
        );
        valid
            .then_some(next)
            .ok_or_else(|| CoreError::V3InvalidTransition(format!("{self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    #[default]
    Manual,
    AutoIntegrate,
    DirectEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    Created,
    Starting,
    Active,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    RecoveryRequired,
}
impl SessionLifecycle {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        let valid = matches!(
            (self, next),
            (
                Self::Created,
                Self::Starting | Self::Cancelled | Self::RecoveryRequired
            ) | (
                Self::Starting,
                Self::Active | Self::Failed | Self::Cancelled | Self::RecoveryRequired
            ) | (
                Self::Active,
                Self::Cancelling | Self::Completed | Self::Failed | Self::RecoveryRequired
            ) | (
                Self::Cancelling,
                Self::Cancelled | Self::Failed | Self::RecoveryRequired
            ) | (
                Self::RecoveryRequired,
                Self::Starting | Self::Active | Self::Cancelled | Self::Failed
            )
        );
        valid
            .then_some(next)
            .ok_or_else(|| CoreError::V3InvalidTransition(format!("session {self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLifecycle {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}
impl ApprovalLifecycle {
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Approved | Self::Denied | Self::Expired | Self::Cancelled
            )
        )
        .then_some(next)
        .ok_or_else(|| CoreError::V3InvalidTransition(format!("approval {self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationLifecycle {
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
    Blocked,
    Incomplete,
}
impl ValidationLifecycle {
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Running | Self::Skipped | Self::Blocked | Self::Incomplete
            ) | (
                Self::Running,
                Self::Passed | Self::Failed | Self::Blocked | Self::Incomplete
            )
        )
        .then_some(next)
        .ok_or_else(|| CoreError::V3InvalidTransition(format!("validation {self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingDisposition {
    Reported,
    ConfirmedBlocking,
    Informational,
    Repaired,
    Dismissed,
}
impl FindingDisposition {
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        matches!(
            (self, next),
            (
                Self::Reported,
                Self::ConfirmedBlocking | Self::Informational | Self::Dismissed
            ) | (Self::ConfirmedBlocking, Self::Repaired | Self::Dismissed)
        )
        .then_some(next)
        .ok_or_else(|| CoreError::V3InvalidTransition(format!("finding {self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairRoundLifecycle {
    Pending,
    Active,
    AwaitingValidation,
    Resolved,
    Exhausted,
    Cancelled,
}
impl RepairRoundLifecycle {
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        matches!(
            (self, next),
            (Self::Pending, Self::Active | Self::Cancelled)
                | (
                    Self::Active,
                    Self::AwaitingValidation | Self::Exhausted | Self::Cancelled
                )
                | (
                    Self::AwaitingValidation,
                    Self::Resolved | Self::Active | Self::Exhausted | Self::Cancelled
                )
        )
        .then_some(next)
        .ok_or_else(|| CoreError::V3InvalidTransition(format!("repair round {self:?} -> {next:?}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCondition {
    None,
    NeedsSupervisorReconciliation,
    NoLiveProcessAssumed,
    SessionRecoveryRequired,
    CorruptStoredState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionStarted,
    SessionResumed,
    Message,
    ToolStarted,
    ToolCompleted,
    TurnCompleted,
    FileChanged,
    ApprovalRequested,
    ValidationReported,
    ReviewFindingReported,
    RepairRoundReported,
    SessionCompleted,
    SessionFailed,
    SessionCancelled,
    Unknown { discriminator: String },
}
impl EventKind {
    fn storage_name(&self) -> String {
        match self {
            Self::Unknown { discriminator } => format!("unknown:{discriminator}"),
            known => enum_name(known),
        }
    }
    fn parse(value: &str) -> Self {
        if let Some(discriminator) = value.strip_prefix("unknown:") {
            Self::Unknown {
                discriminator: discriminator.to_owned(),
            }
        } else {
            serde_json::from_value(Value::String(value.to_owned())).unwrap_or_else(|_| {
                Self::Unknown {
                    discriminator: value.to_owned(),
                }
            })
        }
    }
}
fn enum_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
fn enum_parse<T: DeserializeOwned>(value: &str) -> Result<T, CoreError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| CoreError::CorruptV3State)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedEventEnvelope {
    pub event_id: EventId,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    /// A namespaced adapter identity. The core treats this as an opaque label.
    pub provider: String,
    pub kind: EventKind,
    pub schema_version: u16,
    pub occurred_at_ms: i64,
    pub sequence_number: u64,
    pub causation_id: Option<String>,
    pub correlation_id: Option<String>,
    pub payload: Value,
    pub raw_diagnostic_payload: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub project_id: Option<String>,
    pub workflow_id: String,
    pub workflow_mode: WorkflowMode,
    pub summary: String,
    pub lifecycle: TaskLifecycle,
    pub recovery_condition: RecoveryCondition,
    pub recovery_previous_lifecycle: Option<TaskLifecycle>,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTask {
    pub project_id: Option<String>,
    pub workflow_id: String,
    pub summary: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskIntegration {
    pub task_id: TaskId,
    pub review_generation_id: ReviewGenerationId,
    pub evidence_digest: String,
    pub source_worktree_commit: String,
    pub target_repository: String,
    pub target_branch: String,
    pub target_head_before: String,
    pub resulting_target_commit: Option<String>,
    pub outcome: String,
    pub failure_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTaskIntegration {
    pub task_id: TaskId,
    pub review_generation_id: ReviewGenerationId,
    pub evidence_digest: String,
    pub source_worktree_commit: String,
    pub target_repository: String,
    pub target_branch: String,
    pub target_head_before: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub id: SessionId,
    pub task_id: TaskId,
    pub provider: String,
    pub provider_session_ref: String,
    pub lifecycle: SessionLifecycle,
    pub recovery_condition: RecoveryCondition,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateSession {
    pub task_id: TaskId,
    pub provider: String,
    pub provider_session_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub id: ApprovalId,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub action_kind: String,
    pub summary: String,
    pub lifecycle: ApprovalLifecycle,
    pub version: u64,
    pub created_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateApproval {
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub action_kind: String,
    pub summary: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationResult {
    pub id: ValidationResultId,
    pub task_id: TaskId,
    pub profile_id: String,
    pub check_name: String,
    pub required: bool,
    pub lifecycle: ValidationLifecycle,
    pub summary: Option<String>,
    pub artifact_id: Option<ArtifactId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateValidationResult {
    pub task_id: TaskId,
    pub profile_id: String,
    pub check_name: String,
    pub required: bool,
    pub summary: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationExecution {
    pub validation_id: ValidationResultId,
    pub command_json: String,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub stdout: String,
    pub stderr: String,
    pub outcome: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairRound {
    pub id: RepairRoundId,
    pub task_id: TaskId,
    pub round_number: u32,
    pub lifecycle: RepairRoundLifecycle,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateRepairRound {
    pub task_id: TaskId,
    pub round_number: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewFinding {
    pub id: ReviewFindingId,
    pub task_id: TaskId,
    pub repair_round_id: Option<RepairRoundId>,
    pub review_generation_id: Option<ReviewGenerationId>,
    pub severity: String,
    pub disposition: FindingDisposition,
    pub summary: String,
    pub evidence: Value,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CreateReviewFinding {
    pub task_id: TaskId,
    pub repair_round_id: Option<RepairRoundId>,
    pub review_generation_id: Option<ReviewGenerationId>,
    pub severity: String,
    pub summary: String,
    pub evidence: Value,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewGeneration {
    pub id: ReviewGenerationId,
    pub task_id: TaskId,
    pub digest: String,
    pub base_commit: String,
    pub worktree_revision: String,
    pub diff_digest: String,
    pub validation_evidence: Value,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateReviewGeneration {
    pub task_id: TaskId,
    pub digest: String,
    pub base_commit: String,
    pub worktree_revision: String,
    pub diff_digest: String,
    pub validation_evidence: Value,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Artifact {
    pub id: ArtifactId,
    pub task_id: TaskId,
    pub kind: String,
    pub display_name: String,
    pub content_hash: Option<String>,
    pub metadata: Value,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CreateArtifact {
    pub task_id: TaskId,
    pub kind: String,
    pub display_name: String,
    pub content_hash: Option<String>,
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorRequestReservation {
    New,
    Pending,
    Completed(String),
}

#[derive(Clone)]
pub struct V3Repository {
    pool: SqlitePool,
    changes: V3ChangeNotifier,
}

#[derive(Clone)]
pub(crate) struct V3ChangeNotifier {
    sender: broadcast::Sender<u64>,
    sequence: Arc<AtomicU64>,
}

impl V3ChangeNotifier {
    pub(crate) fn new(sender: broadcast::Sender<u64>, sequence: Arc<AtomicU64>) -> Self {
        Self { sender, sequence }
    }

    fn changed(&self) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.sender.send(sequence);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskWorktree {
    pub task_id: TaskId,
    pub repository_root: String,
    pub worktree_path: String,
    pub branch: String,
    pub base_commit: String,
    pub state: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTaskWorktree {
    pub task_id: TaskId,
    pub repository_root: String,
    pub worktree_path: String,
    pub branch: String,
    pub base_commit: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskWorktreeMergePreparation {
    pub task_id: TaskId,
    pub target_branch: String,
    pub target_commit: String,
    pub worktree_commit: String,
    pub target_advanced: bool,
    pub merge_ready: bool,
    pub conflicts_json: String,
    pub diff_json: String,
    pub prepared_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTaskWorktreeMergePreparation {
    pub task_id: TaskId,
    pub target_branch: String,
    pub target_commit: String,
    pub worktree_commit: String,
    pub target_advanced: bool,
    pub merge_ready: bool,
    pub conflicts_json: String,
    pub diff_json: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskWorktreeCleanupOutcome {
    pub task_id: TaskId,
    pub state: String,
    pub reason: Option<String>,
    pub approval_id: Option<ApprovalId>,
    pub completed_at_ms: i64,
}
impl V3Repository {
    /// Atomically reserves a presentation-layer mutation request. The stored
    /// fingerprint contains no prompt/config text, and a completed response is
    /// safe to replay after a bridge restart without repeating the mutation.
    pub async fn reserve_supervisor_request(
        &self,
        request_id: &str,
        request_kind: &str,
        intent_fingerprint: &str,
        timestamp: i64,
    ) -> Result<SupervisorRequestReservation, CoreError> {
        nonempty(request_id, 256)?;
        nonempty(request_kind, 128)?;
        nonempty(intent_fingerprint, 256)?;
        let mut transaction = self.pool.begin().await.map_err(|_| CoreError::Storage)?;
        let inserted = sqlx::query("INSERT OR IGNORE INTO v3_supervisor_requests (request_id,request_kind,intent_fingerprint,lifecycle,response_json,created_at_ms,completed_at_ms) VALUES (?,?,?,'pending',NULL,?,NULL)")
            .bind(request_id)
            .bind(request_kind)
            .bind(intent_fingerprint)
            .bind(timestamp)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CoreError::Storage)?
            .rows_affected();
        if inserted == 1 {
            transaction.commit().await.map_err(|_| CoreError::Storage)?;
            return Ok(SupervisorRequestReservation::New);
        }
        let row = sqlx::query("SELECT request_kind,intent_fingerprint,lifecycle,response_json FROM v3_supervisor_requests WHERE request_id=?")
            .bind(request_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| CoreError::Storage)?;
        if row.get::<String, _>("request_kind") != request_kind
            || row.get::<String, _>("intent_fingerprint") != intent_fingerprint
        {
            return Err(CoreError::V3Conflict);
        }
        let lifecycle = row.get::<String, _>("lifecycle");
        let response = row.get::<Option<String>, _>("response_json");
        transaction.commit().await.map_err(|_| CoreError::Storage)?;
        match (lifecycle.as_str(), response) {
            ("pending", None) => Ok(SupervisorRequestReservation::Pending),
            ("completed", Some(response)) => Ok(SupervisorRequestReservation::Completed(response)),
            _ => Err(CoreError::CorruptV3State),
        }
    }

    pub async fn complete_supervisor_request(
        &self,
        request_id: &str,
        response_json: &str,
        timestamp: i64,
    ) -> Result<(), CoreError> {
        nonempty(request_id, 256)?;
        nonempty(response_json, 64 * 1024)?;
        serde_json::from_str::<Value>(response_json).map_err(|_| CoreError::InvalidV3Record)?;
        let updated = sqlx::query("UPDATE v3_supervisor_requests SET lifecycle='completed',response_json=?,completed_at_ms=? WHERE request_id=? AND lifecycle='pending'")
            .bind(response_json)
            .bind(timestamp)
            .bind(request_id)
            .execute(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        if updated.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        Ok(())
    }

    pub async fn get_task_worktree(&self, task_id: &TaskId) -> Result<TaskWorktree, CoreError> {
        let row = sqlx::query("SELECT task_id,repository_root,worktree_path,branch,base_commit,state FROM v3_task_worktrees WHERE task_id=?")
            .bind(task_id.to_string()).fetch_optional(&self.pool).await.map_err(|_| CoreError::Storage)?
            .ok_or(CoreError::NotFound)?;
        task_worktree_from(&row)
    }
    pub async fn find_task_worktree_by_path(
        &self,
        worktree_path: &str,
    ) -> Result<Option<TaskWorktree>, CoreError> {
        sqlx::query("SELECT task_id,repository_root,worktree_path,branch,base_commit,state FROM v3_task_worktrees WHERE worktree_path=?")
            .bind(worktree_path)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| task_worktree_from(&row))
            .transpose()
    }
    pub async fn find_task_worktree_by_branch(
        &self,
        branch: &str,
    ) -> Result<Option<TaskWorktree>, CoreError> {
        sqlx::query("SELECT task_id,repository_root,worktree_path,branch,base_commit,state FROM v3_task_worktrees WHERE branch=?")
            .bind(branch)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| task_worktree_from(&row))
            .transpose()
    }
    pub async fn create_task_worktree(
        &self,
        input: CreateTaskWorktree,
        timestamp: i64,
    ) -> Result<TaskWorktree, CoreError> {
        if input.repository_root.is_empty()
            || input.worktree_path.is_empty()
            || input.branch.is_empty()
            || input.base_commit.len() != 40
            || !input.base_commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(CoreError::InvalidV3Record);
        }
        sqlx::query("INSERT INTO v3_task_worktrees (task_id,repository_root,worktree_path,branch,base_commit,state,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,'ready',?,?)")
            .bind(input.task_id.to_string()).bind(&input.repository_root).bind(&input.worktree_path).bind(&input.branch).bind(&input.base_commit).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_task_worktree(&input.task_id).await
    }
    pub async fn set_task_worktree_recovery_required(
        &self,
        task_id: &TaskId,
        timestamp: i64,
    ) -> Result<TaskWorktree, CoreError> {
        let updated = sqlx::query("UPDATE v3_task_worktrees SET state='recovery_required',updated_at_ms=? WHERE task_id=?")
            .bind(timestamp)
            .bind(task_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        if updated.rows_affected() != 1 {
            return Err(CoreError::NotFound);
        }
        self.changes.changed();
        self.get_task_worktree(task_id).await
    }
    pub async fn save_task_worktree_merge_preparation(
        &self,
        input: CreateTaskWorktreeMergePreparation,
        timestamp: i64,
    ) -> Result<TaskWorktreeMergePreparation, CoreError> {
        sqlx::query("INSERT INTO v3_task_worktree_merge_preparations (task_id,target_branch,target_commit,worktree_commit,target_advanced,merge_ready,conflicts_json,diff_json,prepared_at_ms) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET target_branch=excluded.target_branch,target_commit=excluded.target_commit,worktree_commit=excluded.worktree_commit,target_advanced=excluded.target_advanced,merge_ready=excluded.merge_ready,conflicts_json=excluded.conflicts_json,diff_json=excluded.diff_json,prepared_at_ms=excluded.prepared_at_ms")
            .bind(input.task_id.to_string()).bind(&input.target_branch).bind(&input.target_commit).bind(&input.worktree_commit).bind(input.target_advanced).bind(input.merge_ready).bind(&input.conflicts_json).bind(&input.diff_json).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_task_worktree_merge_preparation(&input.task_id)
            .await
    }
    pub async fn get_task_worktree_merge_preparation(
        &self,
        task_id: &TaskId,
    ) -> Result<TaskWorktreeMergePreparation, CoreError> {
        let row = sqlx::query("SELECT * FROM v3_task_worktree_merge_preparations WHERE task_id=?")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .ok_or(CoreError::NotFound)?;
        Ok(TaskWorktreeMergePreparation {
            task_id: TaskId(row.get("task_id")),
            target_branch: row.get("target_branch"),
            target_commit: row.get("target_commit"),
            worktree_commit: row.get("worktree_commit"),
            target_advanced: row.get("target_advanced"),
            merge_ready: row.get("merge_ready"),
            conflicts_json: row.get("conflicts_json"),
            diff_json: row.get("diff_json"),
            prepared_at_ms: row.get("prepared_at_ms"),
        })
    }
    pub async fn get_task_worktree_cleanup_outcome(
        &self,
        task_id: &TaskId,
    ) -> Result<TaskWorktreeCleanupOutcome, CoreError> {
        let row = sqlx::query("SELECT * FROM v3_task_worktree_cleanup_outcomes WHERE task_id=?")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .ok_or(CoreError::NotFound)?;
        Ok(TaskWorktreeCleanupOutcome {
            task_id: TaskId(row.get("task_id")),
            state: row.get("state"),
            reason: row.get("reason"),
            approval_id: row.get::<Option<String>, _>("approval_id").map(ApprovalId),
            completed_at_ms: row.get("completed_at_ms"),
        })
    }
    pub async fn save_task_worktree_cleanup_outcome(
        &self,
        task_id: &TaskId,
        state: &str,
        reason: Option<&str>,
        approval_id: Option<&ApprovalId>,
        timestamp: i64,
    ) -> Result<TaskWorktreeCleanupOutcome, CoreError> {
        if !matches!(state, "discarded" | "retained") {
            return Err(CoreError::InvalidV3Record);
        }
        sqlx::query("INSERT INTO v3_task_worktree_cleanup_outcomes (task_id,state,reason,approval_id,completed_at_ms) VALUES (?,?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET state=excluded.state,reason=excluded.reason,approval_id=excluded.approval_id,completed_at_ms=excluded.completed_at_ms")
            .bind(task_id.to_string()).bind(state).bind(reason).bind(approval_id.map(ToString::to_string)).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_task_worktree_cleanup_outcome(task_id).await
    }
    pub(crate) fn new(pool: SqlitePool, changes: V3ChangeNotifier) -> Self {
        Self { pool, changes }
    }
    pub async fn create_task(&self, input: CreateTask, timestamp: i64) -> Result<Task, CoreError> {
        nonempty(&input.workflow_id, 128)?;
        nonempty(&input.summary, 8000)?;
        let task = Task {
            id: TaskId::new(),
            project_id: input.project_id,
            workflow_id: input.workflow_id,
            workflow_mode: WorkflowMode::Manual,
            summary: input.summary,
            lifecycle: TaskLifecycle::Draft,
            recovery_condition: RecoveryCondition::None,
            recovery_previous_lifecycle: None,
            version: 0,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            terminal_at_ms: None,
        };
        sqlx::query("INSERT INTO v3_tasks (id, project_id, workflow_id, workflow_mode, summary, lifecycle, recovery_condition, recovery_previous_lifecycle, version, created_at_ms, updated_at_ms, terminal_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 0, ?, ?, NULL)")
            .bind(task.id.to_string()).bind(&task.project_id).bind(&task.workflow_id).bind(enum_name(&task.workflow_mode)).bind(&task.summary).bind(enum_name(&task.lifecycle)).bind(enum_name(&task.recovery_condition)).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        Ok(task)
    }
    pub async fn get_task(&self, id: &TaskId) -> Result<Task, CoreError> {
        sqlx::query("SELECT * FROM v3_tasks WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|r| task_from(&r))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn set_task_workflow_mode(
        &self,
        task: &Task,
        mode: WorkflowMode,
        timestamp: i64,
    ) -> Result<Task, CoreError> {
        let updated = sqlx::query("UPDATE v3_tasks SET workflow_mode=?,version=version+1,updated_at_ms=? WHERE id=? AND version=?")
            .bind(enum_name(&mode)).bind(timestamp).bind(task.id.to_string()).bind(i64::try_from(task.version).map_err(|_| CoreError::V3Conflict)?)
            .execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if updated.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_task(&task.id).await
    }
    pub async fn create_task_integration(
        &self,
        input: CreateTaskIntegration,
        timestamp: i64,
    ) -> Result<TaskIntegration, CoreError> {
        if input.evidence_digest.len() != 64
            || input.source_worktree_commit.len() != 40
            || input.target_head_before.len() != 40
        {
            return Err(CoreError::InvalidV3Record);
        }
        sqlx::query("INSERT INTO v3_task_integrations (task_id,review_generation_id,evidence_digest,source_worktree_commit,target_repository,target_branch,target_head_before,resulting_target_commit,outcome,failure_reason,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,NULL,'pending',NULL,?,?)")
            .bind(input.task_id.to_string()).bind(input.review_generation_id.to_string()).bind(&input.evidence_digest).bind(&input.source_worktree_commit).bind(&input.target_repository).bind(&input.target_branch).bind(&input.target_head_before).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_task_integration(&input.task_id).await
    }
    pub async fn get_task_integration(
        &self,
        task_id: &TaskId,
    ) -> Result<TaskIntegration, CoreError> {
        sqlx::query("SELECT * FROM v3_task_integrations WHERE task_id=?")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| task_integration_from(&row))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn complete_task_integration(
        &self,
        task_id: &TaskId,
        resulting_target_commit: &str,
        timestamp: i64,
    ) -> Result<TaskIntegration, CoreError> {
        let result = sqlx::query("UPDATE v3_task_integrations SET outcome='integrated',resulting_target_commit=?,updated_at_ms=? WHERE task_id=? AND outcome='pending'").bind(resulting_target_commit).bind(timestamp).bind(task_id.to_string()).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_task_integration(task_id).await
    }
    pub async fn fail_task_integration(
        &self,
        task_id: &TaskId,
        reason: &str,
        timestamp: i64,
    ) -> Result<TaskIntegration, CoreError> {
        let result = sqlx::query("UPDATE v3_task_integrations SET outcome='failed',failure_reason=?,updated_at_ms=? WHERE task_id=? AND outcome='pending'").bind(reason).bind(timestamp).bind(task_id.to_string()).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_task_integration(task_id).await
    }
    pub async fn list_tasks(&self) -> Result<Vec<Task>, CoreError> {
        sqlx::query("SELECT * FROM v3_tasks ORDER BY updated_at_ms DESC, id DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(task_from)
            .collect()
    }
    pub async fn transition_task(
        &self,
        task: &Task,
        next: TaskLifecycle,
        timestamp: i64,
    ) -> Result<Task, CoreError> {
        task.lifecycle.transition(next)?;
        self.update_task(task, next, RecoveryCondition::None, None, timestamp)
            .await
    }
    async fn update_task(
        &self,
        task: &Task,
        next: TaskLifecycle,
        condition: RecoveryCondition,
        previous: Option<TaskLifecycle>,
        timestamp: i64,
    ) -> Result<Task, CoreError> {
        let terminal_at = next.terminal().then_some(timestamp);
        let updated = sqlx::query("UPDATE v3_tasks SET lifecycle=?, recovery_condition=?, recovery_previous_lifecycle=?, version=version+1, updated_at_ms=?, terminal_at_ms=? WHERE id=? AND version=?")
            .bind(enum_name(&next)).bind(enum_name(&condition)).bind(previous.map(|v| enum_name(&v))).bind(timestamp).bind(terminal_at).bind(task.id.to_string()).bind(i64::try_from(task.version).map_err(|_| CoreError::V3Conflict)?).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if updated.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_task(&task.id).await
    }
    pub async fn append_event(&self, event: &NormalizedEventEnvelope) -> Result<(), CoreError> {
        validate_event(event)?;
        let mut tx = self.pool.begin().await.map_err(|_| CoreError::Storage)?;
        if let Some(row) = sqlx::query("SELECT task_id, session_id, provider, event_kind, schema_version, occurred_at_ms, sequence_number, causation_id, correlation_id, payload_json, raw_diagnostic_json FROM v3_task_events WHERE event_id=?").bind(event.event_id.to_string()).fetch_optional(&mut *tx).await.map_err(|_| CoreError::Storage)? {
            if event_matches(&row, event)? { tx.commit().await.map_err(|_| CoreError::Storage)?; return Ok(()); }
            return Err(CoreError::V3Conflict);
        }
        let latest: Option<i64> =
            sqlx::query_scalar("SELECT MAX(sequence_number) FROM v3_task_events WHERE task_id=?")
                .bind(event.task_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| CoreError::Storage)?;
        if latest.is_some_and(|value| event.sequence_number <= value as u64) {
            return Err(CoreError::V3Conflict);
        }
        insert_event(&mut tx, event).await?;
        tx.commit().await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        Ok(())
    }
    pub async fn transition_task_with_event(
        &self,
        task: &Task,
        next: TaskLifecycle,
        event: &NormalizedEventEnvelope,
        timestamp: i64,
    ) -> Result<Task, CoreError> {
        task.lifecycle.transition(next)?;
        validate_event(event)?;
        if event.task_id != task.id {
            return Err(CoreError::InvalidV3Record);
        }
        let mut tx = self.pool.begin().await.map_err(|_| CoreError::Storage)?;
        let affected = sqlx::query("UPDATE v3_tasks SET lifecycle=?, recovery_condition='none', recovery_previous_lifecycle=NULL, version=version+1, updated_at_ms=?, terminal_at_ms=? WHERE id=? AND version=?")
          .bind(enum_name(&next)).bind(timestamp).bind(next.terminal().then_some(timestamp)).bind(task.id.to_string()).bind(i64::try_from(task.version).map_err(|_| CoreError::V3Conflict)?).execute(&mut *tx).await.map_err(|_| CoreError::Storage)?;
        if affected.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        insert_event(&mut tx, event).await?;
        tx.commit().await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_task(&task.id).await
    }
    pub async fn list_events(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<NormalizedEventEnvelope>, CoreError> {
        sqlx::query("SELECT * FROM v3_task_events WHERE task_id=? ORDER BY sequence_number ASC")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(event_from)
            .collect()
    }
    pub async fn create_session(
        &self,
        input: CreateSession,
        timestamp: i64,
    ) -> Result<AgentSession, CoreError> {
        nonempty(&input.provider, 128)?;
        nonempty(&input.provider_session_ref, 512)?;
        let result = AgentSession {
            id: SessionId::new(),
            task_id: input.task_id,
            provider: input.provider,
            provider_session_ref: input.provider_session_ref,
            lifecycle: SessionLifecycle::Created,
            recovery_condition: RecoveryCondition::None,
            version: 0,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            terminal_at_ms: None,
        };
        sqlx::query("INSERT INTO v3_sessions (id, task_id, provider, provider_session_ref, lifecycle, recovery_condition, version, created_at_ms, updated_at_ms, terminal_at_ms) VALUES (?, ?, ?, ?, 'created', 'none', 0, ?, ?, NULL)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(&result.provider).bind(&result.provider_session_ref).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn get_session(&self, id: &SessionId) -> Result<AgentSession, CoreError> {
        sqlx::query("SELECT * FROM v3_sessions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|r| session_from(&r))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    /// Returns only sessions owned by a task. Adapters use the opaque provider
    /// reference for supported resume/reconciliation; they never discover
    /// provider sessions outside Sentinel's durable state.
    pub async fn list_sessions_for_task(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<AgentSession>, CoreError> {
        sqlx::query("SELECT * FROM v3_sessions WHERE task_id=? ORDER BY created_at_ms ASC, id ASC")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(session_from)
            .collect()
    }
    /// Looks up an opaque adapter session reference without interpreting it.
    pub async fn get_session_by_provider_reference(
        &self,
        task_id: &TaskId,
        provider: &str,
        provider_session_ref: &str,
    ) -> Result<AgentSession, CoreError> {
        sqlx::query(
            "SELECT * FROM v3_sessions WHERE task_id=? AND provider=? AND provider_session_ref=?",
        )
        .bind(task_id.to_string())
        .bind(provider)
        .bind(provider_session_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CoreError::Storage)?
        .map(|row| session_from(&row))
        .transpose()?
        .ok_or(CoreError::NotFound)
    }
    pub async fn transition_session(
        &self,
        session: &AgentSession,
        next: SessionLifecycle,
        timestamp: i64,
    ) -> Result<AgentSession, CoreError> {
        session.lifecycle.transition(next)?;
        let affected = sqlx::query("UPDATE v3_sessions SET lifecycle=?, recovery_condition='none', version=version+1, updated_at_ms=?, terminal_at_ms=? WHERE id=? AND version=?").bind(enum_name(&next)).bind(timestamp).bind(next.terminal().then_some(timestamp)).bind(session.id.to_string()).bind(i64::try_from(session.version).map_err(|_| CoreError::V3Conflict)?).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if affected.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_session(&session.id).await
    }
    pub async fn create_approval(
        &self,
        input: CreateApproval,
        timestamp: i64,
    ) -> Result<Approval, CoreError> {
        nonempty(&input.action_kind, 128)?;
        // Final-action authorization binds the exact owned-worktree identity.
        // Isolated native profiles can legitimately produce a context longer
        // than the old UI-summary-sized limit.
        nonempty(&input.summary, 2_048)?;
        let result = Approval {
            id: ApprovalId::new(),
            task_id: input.task_id,
            session_id: input.session_id,
            action_kind: input.action_kind,
            summary: input.summary,
            lifecycle: ApprovalLifecycle::Pending,
            version: 0,
            created_at_ms: timestamp,
            decided_at_ms: None,
        };
        sqlx::query("INSERT INTO v3_approvals (id, task_id, session_id, action_kind, summary, lifecycle, version, created_at_ms, decided_at_ms) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, NULL)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(result.session_id.as_ref().map(ToString::to_string)).bind(&result.action_kind).bind(&result.summary).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn get_approval(&self, id: &ApprovalId) -> Result<Approval, CoreError> {
        sqlx::query("SELECT * FROM v3_approvals WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|r| approval_from(&r))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn list_approvals_for_task(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<Approval>, CoreError> {
        sqlx::query("SELECT * FROM v3_approvals WHERE task_id=? ORDER BY created_at_ms ASC, id ASC")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(approval_from)
            .collect()
    }
    pub async fn transition_approval(
        &self,
        approval: &Approval,
        next: ApprovalLifecycle,
        timestamp: i64,
    ) -> Result<Approval, CoreError> {
        approval.lifecycle.transition(next)?;
        let result=sqlx::query("UPDATE v3_approvals SET lifecycle=?,version=version+1,decided_at_ms=? WHERE id=? AND version=?").bind(enum_name(&next)).bind(timestamp).bind(approval.id.to_string()).bind(i64::try_from(approval.version).map_err(|_|CoreError::V3Conflict)?).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_approval(&approval.id).await
    }
    pub async fn create_validation_result(
        &self,
        input: CreateValidationResult,
        timestamp: i64,
    ) -> Result<ValidationResult, CoreError> {
        nonempty(&input.profile_id, 128)?;
        nonempty(&input.check_name, 128)?;
        let result = ValidationResult {
            id: ValidationResultId::new(),
            task_id: input.task_id,
            profile_id: input.profile_id,
            check_name: input.check_name,
            required: input.required,
            lifecycle: ValidationLifecycle::Pending,
            summary: input.summary,
            artifact_id: None,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        sqlx::query("INSERT INTO v3_validation_results (id,task_id,profile_id,check_name,required,lifecycle,summary,artifact_id,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,'pending',?,NULL,?,?)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(&result.profile_id).bind(&result.check_name).bind(result.required).bind(&result.summary).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn get_validation_result(
        &self,
        id: &ValidationResultId,
    ) -> Result<ValidationResult, CoreError> {
        sqlx::query("SELECT * FROM v3_validation_results WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|r| validation_from(&r))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn list_validation_results(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<ValidationResult>, CoreError> {
        sqlx::query("SELECT * FROM v3_validation_results WHERE task_id=? ORDER BY created_at_ms ASC, id ASC")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(validation_from)
            .collect()
    }
    pub async fn transition_validation_result(
        &self,
        value: &ValidationResult,
        next: ValidationLifecycle,
        summary: Option<String>,
        timestamp: i64,
    ) -> Result<ValidationResult, CoreError> {
        value.lifecycle.transition(next)?;
        let result=sqlx::query("UPDATE v3_validation_results SET lifecycle=?,summary=?,updated_at_ms=? WHERE id=? AND lifecycle=?").bind(enum_name(&next)).bind(summary).bind(timestamp).bind(value.id.to_string()).bind(enum_name(&value.lifecycle)).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_validation_result(&value.id).await
    }
    pub async fn save_validation_execution(
        &self,
        value: &ValidationExecution,
    ) -> Result<(), CoreError> {
        sqlx::query("INSERT INTO v3_validation_executions (validation_id,command_json,exit_code,duration_ms,stdout,stderr,outcome,started_at_ms,finished_at_ms) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(value.validation_id.to_string()).bind(&value.command_json).bind(value.exit_code).bind(value.duration_ms).bind(&value.stdout).bind(&value.stderr).bind(&value.outcome).bind(value.started_at_ms).bind(value.finished_at_ms).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        Ok(())
    }
    pub async fn get_validation_execution(
        &self,
        id: &ValidationResultId,
    ) -> Result<ValidationExecution, CoreError> {
        let row = sqlx::query("SELECT * FROM v3_validation_executions WHERE validation_id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .ok_or(CoreError::NotFound)?;
        Ok(ValidationExecution {
            validation_id: ValidationResultId(row.get("validation_id")),
            command_json: row.get("command_json"),
            exit_code: row.get("exit_code"),
            duration_ms: row.get("duration_ms"),
            stdout: row.get("stdout"),
            stderr: row.get("stderr"),
            outcome: row.get("outcome"),
            started_at_ms: row.get("started_at_ms"),
            finished_at_ms: row.get("finished_at_ms"),
        })
    }
    pub async fn recover_interrupted_validations(&self, timestamp: i64) -> Result<u64, CoreError> {
        let result = sqlx::query("UPDATE v3_validation_results SET lifecycle='incomplete',summary='interrupted by restart',updated_at_ms=? WHERE lifecycle='running'").bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() > 0 {
            self.changes.changed();
        }
        Ok(result.rows_affected())
    }
    pub async fn recover_interrupted_validations_for_task(
        &self,
        task_id: &TaskId,
        timestamp: i64,
    ) -> Result<u64, CoreError> {
        let result = sqlx::query("UPDATE v3_validation_results SET lifecycle='incomplete',summary='interrupted by restart',updated_at_ms=? WHERE task_id=? AND lifecycle='running'")
            .bind(timestamp)
            .bind(task_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?;
        if result.rows_affected() > 0 {
            self.changes.changed();
        }
        Ok(result.rows_affected())
    }
    pub async fn create_repair_round(
        &self,
        input: CreateRepairRound,
        timestamp: i64,
    ) -> Result<RepairRound, CoreError> {
        let result = RepairRound {
            id: RepairRoundId::new(),
            task_id: input.task_id,
            round_number: input.round_number,
            lifecycle: RepairRoundLifecycle::Pending,
            version: 0,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        sqlx::query("INSERT INTO v3_repair_rounds (id,task_id,round_number,lifecycle,version,created_at_ms,updated_at_ms) VALUES (?,?,?,'pending',0,?,?)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(result.round_number).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn get_repair_round(&self, id: &RepairRoundId) -> Result<RepairRound, CoreError> {
        sqlx::query("SELECT * FROM v3_repair_rounds WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| repair_round_from(&row))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn list_repair_rounds(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<RepairRound>, CoreError> {
        sqlx::query(
            "SELECT * FROM v3_repair_rounds WHERE task_id=? ORDER BY round_number ASC, id ASC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CoreError::Storage)?
        .iter()
        .map(repair_round_from)
        .collect()
    }
    pub async fn transition_repair_round(
        &self,
        round: &RepairRound,
        next: RepairRoundLifecycle,
        timestamp: i64,
    ) -> Result<RepairRound, CoreError> {
        round.lifecycle.transition(next)?;
        let result = sqlx::query("UPDATE v3_repair_rounds SET lifecycle=?,version=version+1,updated_at_ms=? WHERE id=? AND version=?")
            .bind(enum_name(&next))
            .bind(timestamp)
            .bind(round.id.to_string())
            .bind(i64::try_from(round.version).map_err(|_| CoreError::V3Conflict)?)
            .execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_repair_round(&round.id).await
    }
    pub async fn create_review_finding(
        &self,
        input: CreateReviewFinding,
        timestamp: i64,
    ) -> Result<ReviewFinding, CoreError> {
        nonempty(&input.severity, 128)?;
        nonempty(&input.summary, 2048)?;
        let evidence =
            serde_json::to_string(&input.evidence).map_err(|_| CoreError::InvalidV3Record)?;
        let result = ReviewFinding {
            id: ReviewFindingId::new(),
            task_id: input.task_id,
            repair_round_id: input.repair_round_id,
            review_generation_id: input.review_generation_id,
            severity: input.severity,
            disposition: FindingDisposition::Reported,
            summary: input.summary,
            evidence: input.evidence,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        sqlx::query("INSERT INTO v3_review_findings (id,task_id,repair_round_id,review_generation_id,severity,disposition,summary,evidence_json,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,'reported',?,?,?,?)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(result.repair_round_id.as_ref().map(ToString::to_string)).bind(result.review_generation_id.as_ref().map(ToString::to_string)).bind(&result.severity).bind(&result.summary).bind(evidence).bind(timestamp).bind(timestamp).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn create_review_generation(
        &self,
        input: CreateReviewGeneration,
        timestamp: i64,
    ) -> Result<ReviewGeneration, CoreError> {
        if input.digest.len() != 71
            || input.diff_digest.len() != 71
            || input.base_commit.len() != 40
            || input.worktree_revision.len() != 40
        {
            return Err(CoreError::InvalidV3Record);
        }
        let evidence = serde_json::to_string(&input.validation_evidence)
            .map_err(|_| CoreError::InvalidV3Record)?;
        let result = ReviewGeneration {
            id: ReviewGenerationId::new(),
            task_id: input.task_id,
            digest: input.digest,
            base_commit: input.base_commit,
            worktree_revision: input.worktree_revision,
            diff_digest: input.diff_digest,
            validation_evidence: input.validation_evidence,
            created_at_ms: timestamp,
        };
        sqlx::query("INSERT INTO v3_review_generations (id,task_id,digest,base_commit,worktree_revision,diff_digest,validation_evidence_json,created_at_ms) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(task_id,digest) DO NOTHING")
            .bind(result.id.to_string()).bind(result.task_id.to_string()).bind(&result.digest).bind(&result.base_commit).bind(&result.worktree_revision).bind(&result.diff_digest).bind(evidence).bind(timestamp).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        self.changes.changed();
        self.get_review_generation_by_digest(&result.task_id, &result.digest)
            .await
    }
    pub async fn latest_review_generation(
        &self,
        task_id: &TaskId,
    ) -> Result<ReviewGeneration, CoreError> {
        sqlx::query("SELECT * FROM v3_review_generations WHERE task_id=? ORDER BY created_at_ms DESC,id DESC LIMIT 1").bind(task_id.to_string()).fetch_optional(&self.pool).await.map_err(|_| CoreError::Storage)?.map(|row| review_generation_from(&row)).transpose()?.ok_or(CoreError::NotFound)
    }
    pub async fn get_review_generation_by_digest(
        &self,
        task_id: &TaskId,
        digest: &str,
    ) -> Result<ReviewGeneration, CoreError> {
        sqlx::query("SELECT * FROM v3_review_generations WHERE task_id=? AND digest=?")
            .bind(task_id.to_string())
            .bind(digest)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| review_generation_from(&row))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn get_review_finding(
        &self,
        id: &ReviewFindingId,
    ) -> Result<ReviewFinding, CoreError> {
        sqlx::query("SELECT * FROM v3_review_findings WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| finding_from(&row))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    pub async fn list_review_findings(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<ReviewFinding>, CoreError> {
        sqlx::query(
            "SELECT * FROM v3_review_findings WHERE task_id=? ORDER BY created_at_ms ASC, id ASC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CoreError::Storage)?
        .iter()
        .map(finding_from)
        .collect()
    }
    pub async fn transition_review_finding(
        &self,
        finding: &ReviewFinding,
        next: FindingDisposition,
        timestamp: i64,
    ) -> Result<ReviewFinding, CoreError> {
        finding.disposition.transition(next)?;
        let result = sqlx::query("UPDATE v3_review_findings SET disposition=?,updated_at_ms=? WHERE id=? AND disposition=?")
            .bind(enum_name(&next)).bind(timestamp).bind(finding.id.to_string())
            .bind(enum_name(&finding.disposition)).execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_review_finding(&finding.id).await
    }
    pub async fn assign_review_finding_to_repair_round(
        &self,
        finding: &ReviewFinding,
        round: &RepairRound,
        timestamp: i64,
    ) -> Result<ReviewFinding, CoreError> {
        if finding.task_id != round.task_id
            || finding.repair_round_id.is_some()
            || finding.disposition != FindingDisposition::ConfirmedBlocking
        {
            return Err(CoreError::InvalidV3Record);
        }
        let result = sqlx::query("UPDATE v3_review_findings SET repair_round_id=?,updated_at_ms=? WHERE id=? AND repair_round_id IS NULL AND disposition='confirmed_blocking'")
            .bind(round.id.to_string()).bind(timestamp).bind(finding.id.to_string())
            .execute(&self.pool).await.map_err(|_| CoreError::Storage)?;
        if result.rows_affected() != 1 {
            return Err(CoreError::V3Conflict);
        }
        self.changes.changed();
        self.get_review_finding(&finding.id).await
    }
    pub async fn create_artifact(
        &self,
        input: CreateArtifact,
        timestamp: i64,
    ) -> Result<Artifact, CoreError> {
        nonempty(&input.kind, 128)?;
        nonempty(&input.display_name, 512)?;
        let metadata =
            serde_json::to_string(&input.metadata).map_err(|_| CoreError::InvalidV3Record)?;
        let result = Artifact {
            id: ArtifactId::new(),
            task_id: input.task_id,
            kind: input.kind,
            display_name: input.display_name,
            content_hash: input.content_hash,
            metadata: input.metadata,
            created_at_ms: timestamp,
        };
        sqlx::query("INSERT INTO v3_artifacts (id,task_id,kind,display_name,content_hash,metadata_json,created_at_ms) VALUES (?,?,?,?,?,?,?)").bind(result.id.to_string()).bind(result.task_id.to_string()).bind(&result.kind).bind(&result.display_name).bind(&result.content_hash).bind(metadata).bind(timestamp).execute(&self.pool).await.map_err(|_|CoreError::Storage)?;
        self.changes.changed();
        Ok(result)
    }
    pub async fn list_artifacts(&self, task_id: &TaskId) -> Result<Vec<Artifact>, CoreError> {
        sqlx::query("SELECT * FROM v3_artifacts WHERE task_id=? ORDER BY created_at_ms ASC, id ASC")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .iter()
            .map(artifact_from)
            .collect()
    }
    pub async fn get_artifact(&self, id: &ArtifactId) -> Result<Artifact, CoreError> {
        sqlx::query("SELECT * FROM v3_artifacts WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| CoreError::Storage)?
            .map(|row| artifact_from(&row))
            .transpose()?
            .ok_or(CoreError::NotFound)
    }
    /// Moves one active record into an explicit recovery state. It never
    /// assumes a provider process survived restart and never derives completion.
    pub async fn restore_unfinished_task(
        &self,
        task_id: &TaskId,
        timestamp: i64,
    ) -> Result<Task, CoreError> {
        let task = self.get_task(task_id).await?;
        if task.lifecycle.terminal() || task.lifecycle == TaskLifecycle::Draft {
            return Ok(task);
        }
        if task.lifecycle == TaskLifecycle::Recovering {
            reconcile_task_sessions(&self.pool, &task.id, timestamp).await?;
            return Ok(task);
        }
        let updated = self
            .update_task(
                &task,
                TaskLifecycle::Recovering,
                RecoveryCondition::NoLiveProcessAssumed,
                Some(task.lifecycle),
                timestamp,
            )
            .await?;
        reconcile_task_sessions(&self.pool, &updated.id, timestamp).await?;
        self.changes.changed();
        Ok(updated)
    }

    /// Moves all active records into recovery. Repository-scoped supervisors
    /// should prefer `restore_unfinished_task` for their owned tasks.
    pub async fn restore_unfinished_tasks(&self, timestamp: i64) -> Result<Vec<Task>, CoreError> {
        let tasks = self.list_tasks().await?;
        let mut restored = Vec::new();
        for task in tasks
            .into_iter()
            .filter(|task| !task.lifecycle.terminal() && task.lifecycle != TaskLifecycle::Draft)
        {
            let was_recovering = task.lifecycle == TaskLifecycle::Recovering;
            let updated = self.restore_unfinished_task(&task.id, timestamp).await?;
            if was_recovering {
                continue;
            }
            restored.push(updated)
        }
        Ok(restored)
    }
}

async fn reconcile_task_sessions(
    pool: &SqlitePool,
    task_id: &TaskId,
    timestamp: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE v3_sessions SET lifecycle='recovery_required',recovery_condition='session_recovery_required',version=version+1,updated_at_ms=? WHERE task_id=? AND lifecycle IN ('created','starting','active','cancelling')")
        .bind(timestamp)
        .bind(task_id.to_string())
        .execute(pool)
        .await
        .map_err(|_| CoreError::Storage)?;
    Ok(())
}

fn nonempty(value: &str, max: usize) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.len() > max {
        Err(CoreError::InvalidV3Record)
    } else {
        Ok(())
    }
}
fn validate_event(event: &NormalizedEventEnvelope) -> Result<(), CoreError> {
    nonempty(&event.provider, 128)?;
    if event.schema_version == 0 {
        return Err(CoreError::InvalidV3Record);
    }
    if event.sequence_number > i64::MAX as u64 {
        return Err(CoreError::InvalidV3Record);
    };
    if matches!(&event.kind,EventKind::Unknown{discriminator} if discriminator.trim().is_empty()) {
        return Err(CoreError::InvalidV3Record);
    };
    serde_json::to_string(&event.payload).map_err(|_| CoreError::InvalidV3Record)?;
    event
        .raw_diagnostic_payload
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| CoreError::InvalidV3Record)?;
    Ok(())
}
async fn insert_event(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    event: &NormalizedEventEnvelope,
) -> Result<(), CoreError> {
    let payload = serde_json::to_string(&event.payload).map_err(|_| CoreError::InvalidV3Record)?;
    let raw = event
        .raw_diagnostic_payload
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| CoreError::InvalidV3Record)?;
    sqlx::query("INSERT INTO v3_task_events (event_id,task_id,session_id,provider,event_kind,schema_version,occurred_at_ms,sequence_number,causation_id,correlation_id,payload_json,raw_diagnostic_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)").bind(event.event_id.to_string()).bind(event.task_id.to_string()).bind(event.session_id.as_ref().map(ToString::to_string)).bind(&event.provider).bind(event.kind.storage_name()).bind(event.schema_version).bind(event.occurred_at_ms).bind(i64::try_from(event.sequence_number).map_err(|_|CoreError::InvalidV3Record)?).bind(&event.causation_id).bind(&event.correlation_id).bind(payload).bind(raw).execute(&mut **tx).await.map_err(|_|CoreError::Storage)?;
    Ok(())
}
fn task_worktree_from(row: &sqlx::sqlite::SqliteRow) -> Result<TaskWorktree, CoreError> {
    let state: String = row.get("state");
    if !matches!(state.as_str(), "ready" | "recovery_required") {
        return Err(CoreError::CorruptV3State);
    }
    Ok(TaskWorktree {
        task_id: TaskId(row.get("task_id")),
        repository_root: row.get("repository_root"),
        worktree_path: row.get("worktree_path"),
        branch: row.get("branch"),
        base_commit: row.get("base_commit"),
        state,
    })
}
fn task_from(row: &sqlx::sqlite::SqliteRow) -> Result<Task, CoreError> {
    let task = Task {
        id: TaskId(row.get("id")),
        project_id: row.get("project_id"),
        workflow_id: row.get("workflow_id"),
        workflow_mode: enum_parse(&row.get::<String, _>("workflow_mode"))?,
        summary: row.get("summary"),
        lifecycle: enum_parse(&row.get::<String, _>("lifecycle"))?,
        recovery_condition: enum_parse(&row.get::<String, _>("recovery_condition"))?,
        recovery_previous_lifecycle: row
            .get::<Option<String>, _>("recovery_previous_lifecycle")
            .map(|v| enum_parse(&v))
            .transpose()?,
        version: u64::try_from(row.get::<i64, _>("version"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
        terminal_at_ms: row.get("terminal_at_ms"),
    };
    if (task.lifecycle == TaskLifecycle::Recovering
        && (task.recovery_previous_lifecycle.is_none()
            || task.recovery_condition == RecoveryCondition::None))
        || (task.lifecycle.terminal() != task.terminal_at_ms.is_some())
    {
        return Err(CoreError::CorruptV3State);
    }
    Ok(task)
}
fn task_integration_from(row: &sqlx::sqlite::SqliteRow) -> Result<TaskIntegration, CoreError> {
    Ok(TaskIntegration {
        task_id: TaskId(row.get("task_id")),
        review_generation_id: ReviewGenerationId(row.get("review_generation_id")),
        evidence_digest: row.get("evidence_digest"),
        source_worktree_commit: row.get("source_worktree_commit"),
        target_repository: row.get("target_repository"),
        target_branch: row.get("target_branch"),
        target_head_before: row.get("target_head_before"),
        resulting_target_commit: row.get("resulting_target_commit"),
        outcome: row.get("outcome"),
        failure_reason: row.get("failure_reason"),
    })
}
fn session_from(row: &sqlx::sqlite::SqliteRow) -> Result<AgentSession, CoreError> {
    Ok(AgentSession {
        id: SessionId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        provider: row.get("provider"),
        provider_session_ref: row.get("provider_session_ref"),
        lifecycle: enum_parse(&row.get::<String, _>("lifecycle"))?,
        recovery_condition: enum_parse(&row.get::<String, _>("recovery_condition"))?,
        version: u64::try_from(row.get::<i64, _>("version"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
        terminal_at_ms: row.get("terminal_at_ms"),
    })
}
fn approval_from(row: &sqlx::sqlite::SqliteRow) -> Result<Approval, CoreError> {
    Ok(Approval {
        id: ApprovalId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        session_id: row.get::<Option<String>, _>("session_id").map(SessionId),
        action_kind: row.get("action_kind"),
        summary: row.get("summary"),
        lifecycle: enum_parse(&row.get::<String, _>("lifecycle"))?,
        version: u64::try_from(row.get::<i64, _>("version"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
        decided_at_ms: row.get("decided_at_ms"),
    })
}
fn validation_from(row: &sqlx::sqlite::SqliteRow) -> Result<ValidationResult, CoreError> {
    Ok(ValidationResult {
        id: ValidationResultId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        profile_id: row.get("profile_id"),
        check_name: row.get("check_name"),
        required: row.get::<i64, _>("required") != 0,
        lifecycle: enum_parse(&row.get::<String, _>("lifecycle"))?,
        summary: row.get("summary"),
        artifact_id: row.get::<Option<String>, _>("artifact_id").map(ArtifactId),
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
    })
}
fn repair_round_from(row: &sqlx::sqlite::SqliteRow) -> Result<RepairRound, CoreError> {
    Ok(RepairRound {
        id: RepairRoundId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        round_number: u32::try_from(row.get::<i64, _>("round_number"))
            .map_err(|_| CoreError::CorruptV3State)?,
        lifecycle: enum_parse(&row.get::<String, _>("lifecycle"))?,
        version: u64::try_from(row.get::<i64, _>("version"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
    })
}
fn finding_from(row: &sqlx::sqlite::SqliteRow) -> Result<ReviewFinding, CoreError> {
    Ok(ReviewFinding {
        id: ReviewFindingId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        repair_round_id: row
            .get::<Option<String>, _>("repair_round_id")
            .map(RepairRoundId),
        review_generation_id: row
            .get::<Option<String>, _>("review_generation_id")
            .map(ReviewGenerationId),
        severity: row.get("severity"),
        disposition: enum_parse(&row.get::<String, _>("disposition"))?,
        summary: row.get("summary"),
        evidence: serde_json::from_str(&row.get::<String, _>("evidence_json"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
    })
}
fn review_generation_from(row: &sqlx::sqlite::SqliteRow) -> Result<ReviewGeneration, CoreError> {
    Ok(ReviewGeneration {
        id: ReviewGenerationId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        digest: row.get("digest"),
        base_commit: row.get("base_commit"),
        worktree_revision: row.get("worktree_revision"),
        diff_digest: row.get("diff_digest"),
        validation_evidence: serde_json::from_str(
            &row.get::<String, _>("validation_evidence_json"),
        )
        .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
    })
}
fn artifact_from(row: &sqlx::sqlite::SqliteRow) -> Result<Artifact, CoreError> {
    Ok(Artifact {
        id: ArtifactId(row.get("id")),
        task_id: TaskId(row.get("task_id")),
        kind: row.get("kind"),
        display_name: row.get("display_name"),
        content_hash: row.get("content_hash"),
        metadata: serde_json::from_str(&row.get::<String, _>("metadata_json"))
            .map_err(|_| CoreError::CorruptV3State)?,
        created_at_ms: row.get("created_at_ms"),
    })
}
fn event_from(row: &sqlx::sqlite::SqliteRow) -> Result<NormalizedEventEnvelope, CoreError> {
    Ok(NormalizedEventEnvelope {
        event_id: EventId(row.get("event_id")),
        task_id: TaskId(row.get("task_id")),
        session_id: row.get::<Option<String>, _>("session_id").map(SessionId),
        provider: row.get("provider"),
        kind: EventKind::parse(&row.get::<String, _>("event_kind")),
        schema_version: u16::try_from(row.get::<i64, _>("schema_version"))
            .map_err(|_| CoreError::CorruptV3State)?,
        occurred_at_ms: row.get("occurred_at_ms"),
        sequence_number: u64::try_from(row.get::<i64, _>("sequence_number"))
            .map_err(|_| CoreError::CorruptV3State)?,
        causation_id: row.get("causation_id"),
        correlation_id: row.get("correlation_id"),
        payload: serde_json::from_str(&row.get::<String, _>("payload_json"))
            .map_err(|_| CoreError::CorruptV3State)?,
        raw_diagnostic_payload: row
            .get::<Option<String>, _>("raw_diagnostic_json")
            .map(|v| serde_json::from_str(&v).map_err(|_| CoreError::CorruptV3State))
            .transpose()?,
    })
}
fn event_matches(
    row: &sqlx::sqlite::SqliteRow,
    event: &NormalizedEventEnvelope,
) -> Result<bool, CoreError> {
    Ok(row.get::<String, _>("task_id") == event.task_id.0
        && row.get::<Option<String>, _>("session_id")
            == event.session_id.as_ref().map(ToString::to_string)
        && row.get::<String, _>("provider") == event.provider
        && row.get::<String, _>("event_kind") == event.kind.storage_name()
        && row.get::<i64, _>("schema_version") == i64::from(event.schema_version)
        && row.get::<i64, _>("occurred_at_ms") == event.occurred_at_ms
        && row.get::<i64, _>("sequence_number")
            == i64::try_from(event.sequence_number).map_err(|_| CoreError::InvalidV3Record)?
        && row.get::<Option<String>, _>("causation_id") == event.causation_id
        && row.get::<Option<String>, _>("correlation_id") == event.correlation_id
        && row.get::<String, _>("payload_json")
            == serde_json::to_string(&event.payload).map_err(|_| CoreError::InvalidV3Record)?
        && row.get::<Option<String>, _>("raw_diagnostic_json")
            == event
                .raw_diagnostic_payload
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| CoreError::InvalidV3Record)?)
}
