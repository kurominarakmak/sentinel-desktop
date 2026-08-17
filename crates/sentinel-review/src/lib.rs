//! Read-only, supervisor-owned V3 review packets and structured findings.
//!
//! A reviewer receives facts, never a filesystem handle or a command capability.
//! This makes the review boundary intentionally incapable of changing a task tree.

use sentinel_core::{
    v3::{
        AgentSession, Approval, ApprovalId, ApprovalLifecycle, CreateApproval, CreateArtifact,
        CreateRepairRound, CreateReviewFinding, FindingDisposition, RepairRoundId,
        RepairRoundLifecycle, ReviewFinding, SessionLifecycle, TaskId, ValidationLifecycle,
    },
    CoreError, RunRepository,
};
use sentinel_validation::{ValidationProfile, ValidationRunner};
use sentinel_worktree::{ChangedFileState, WorktreeTransaction};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Blocker,
    Warning,
    Suggestion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewValidation {
    pub check_name: String,
    pub lifecycle: ValidationLifecycle,
    pub outcome: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPacket {
    pub task_id: String,
    pub task_summary: String,
    pub base_commit: String,
    /// Textual `git diff` against the pinned base. This is review evidence,
    /// not a filesystem capability.
    pub diff: String,
    pub changed_files: Vec<ChangedFileState>,
    pub validations: Vec<ReviewValidation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCandidate {
    pub severity: FindingSeverity,
    pub summary: String,
    pub file: String,
    pub line: u32,
    pub evidence: String,
    /// Optional model claim about a deterministic validation result.
    pub validation_check: Option<String>,
    pub validation_outcome: Option<String>,
}
pub trait Reviewer: Send + Sync {
    fn review(&self, packet: &ReviewPacket) -> Result<Vec<ReviewCandidate>, ReviewError>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewReport {
    pub persisted: usize,
    pub deduplicated: usize,
    pub rejected: usize,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReviewError {
    #[error("worktree reconciliation is required")]
    Reconciliation,
    #[error("reviewer returned a malformed finding")]
    MalformedFinding,
    #[error("reviewer failed")]
    Reviewer,
    #[error("storage failure")]
    Storage,
}

pub struct ReviewSupervisor;
impl ReviewSupervisor {
    pub async fn run(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        reviewer: &dyn Reviewer,
    ) -> Result<ReviewReport, ReviewError> {
        let packet = Self::build_packet(repository.clone(), task_id.clone(), main).await?;
        let candidates = reviewer.review(&packet)?;
        Self::persist_candidates(repository, task_id, &packet, candidates).await
    }

    pub async fn build_packet(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
    ) -> Result<ReviewPacket, ReviewError> {
        let task = repository.v3().get_task(&task_id).await.map_err(map_core)?;
        let worktree = WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| ReviewError::Reconciliation)?;
        let diff = WorktreeTransaction::diff(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| ReviewError::Reconciliation)?;
        let validations = repository
            .v3()
            .list_validation_results(&task_id)
            .await
            .map_err(map_core)?;
        let mut facts = Vec::new();
        for result in validations {
            let execution = repository
                .v3()
                .get_validation_execution(&result.id)
                .await
                .ok();
            facts.push(ReviewValidation {
                check_name: result.check_name,
                lifecycle: result.lifecycle,
                outcome: execution.map(|v| v.outcome),
            });
        }
        Ok(ReviewPacket {
            task_id: task_id.to_string(),
            task_summary: task.summary,
            base_commit: worktree.base_commit,
            diff: WorktreeTransaction::diff_text(repository.clone(), task_id.clone(), main)
                .await
                .map_err(|_| ReviewError::Reconciliation)?,
            changed_files: diff.files,
            validations: facts,
        })
    }

    pub async fn persist_candidates(
        repository: RunRepository,
        task_id: TaskId,
        packet: &ReviewPacket,
        candidates: Vec<ReviewCandidate>,
    ) -> Result<ReviewReport, ReviewError> {
        if candidates.iter().any(|candidate| !valid(candidate)) {
            return Err(ReviewError::MalformedFinding);
        }
        let existing = repository
            .v3()
            .list_review_findings(&task_id)
            .await
            .map_err(map_core)?;
        let mut report = ReviewReport {
            persisted: 0,
            deduplicated: 0,
            rejected: 0,
        };
        let mut seen = HashSet::new();
        for candidate in candidates {
            if unsupported(&candidate, &packet.validations) {
                report.rejected += 1;
                continue;
            }
            let evidence = serde_json::json!({"file": candidate.file, "line": candidate.line, "evidence": candidate.evidence});
            let key = format!(
                "{}\u{0}{}\u{0}{}",
                severity(&candidate.severity),
                candidate.summary,
                evidence
            );
            let duplicate = existing.iter().any(|finding| {
                finding.severity == severity(&candidate.severity)
                    && finding.summary == candidate.summary
                    && finding.evidence == evidence
            }) || !seen.insert(key);
            if duplicate {
                report.deduplicated += 1;
                continue;
            }
            repository
                .v3()
                .create_review_finding(
                    CreateReviewFinding {
                        task_id: task_id.clone(),
                        repair_round_id: None,
                        severity: severity(&candidate.severity).into(),
                        summary: candidate.summary,
                        evidence,
                    },
                    now(),
                )
                .await
                .map_err(map_core)?;
            report.persisted += 1;
        }
        // Deliberately no task transition: review completion cannot finalize a task.
        Ok(report)
    }
}
fn valid(value: &ReviewCandidate) -> bool {
    !value.summary.trim().is_empty()
        && !value.file.trim().is_empty()
        && value.line > 0
        && !value.evidence.trim().is_empty()
        && !value.file.starts_with('/')
        && !value.file.contains("..")
        && value.summary.len() <= 2048
}
fn unsupported(value: &ReviewCandidate, validations: &[ReviewValidation]) -> bool {
    match (&value.validation_check, &value.validation_outcome) {
        (None, None) => false,
        (Some(check), Some(outcome)) => !validations
            .iter()
            .any(|fact| fact.check_name == *check && fact.outcome.as_deref() == Some(outcome)),
        _ => true,
    }
}
fn severity(value: &FindingSeverity) -> &'static str {
    match value {
        FindingSeverity::Blocker => "blocker",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Suggestion => "suggestion",
    }
}
fn map_core(_: CoreError) -> ReviewError {
    ReviewError::Storage
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis() as i64)
        .unwrap_or(0)
}

/// The bounded, serializable handoff supplied to a supported Codex or Claude
/// adapter. The adapter receives an opaque Sentinel-owned session reference,
/// never a reviewer capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairPacket {
    pub repair_round_id: String,
    pub task_id: String,
    pub findings: Vec<RepairFinding>,
    pub validations: Vec<ReviewValidation>,
    pub implementer_session_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairFinding {
    pub id: String,
    pub summary: String,
    pub file: String,
    pub line: u32,
    pub evidence: String,
}
/// This is the only write-capable boundary. Codex/Claude adapter integration
/// implements it using a Sentinel-owned task session; reviewer implementations
/// cannot implement this path because they receive no worktree handle.
pub trait RepairImplementer: Send + Sync {
    fn repair(
        &self,
        session: Option<&AgentSession>,
        packet: &RepairPacket,
    ) -> Result<RepairEvidence, RepairError>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairEvidence {
    /// Files the implementer says it changed. This claim is insufficient on
    /// its own; the supervisor confirms it against the pinned-base diff and
    /// a fresh deterministic validation run.
    pub changed_files: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairReport {
    pub repair_round_id: RepairRoundId,
    pub resolved: usize,
    pub unresolved: usize,
    pub validation_passed: bool,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RepairError {
    #[error("no eligible confirmed blocker findings")]
    NoEligibleFindings,
    #[error("finding was already handed off")]
    DuplicateHandoff,
    #[error("malformed persisted finding")]
    MalformedFinding,
    #[error("worktree reconciliation is required")]
    Reconciliation,
    #[error("repair implementer failed")]
    Implementer,
    #[error("validation failed")]
    Validation,
    #[error("storage failure")]
    Storage,
}

pub struct RepairSupervisor;
impl RepairSupervisor {
    pub async fn handoff(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        profile: ValidationProfile,
        implementer: &dyn RepairImplementer,
    ) -> Result<RepairReport, RepairError> {
        // The persisted worktree identity must be current before any adapter
        // receives a repair request.
        WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| RepairError::Reconciliation)?;
        let findings = repository
            .v3()
            .list_review_findings(&task_id)
            .await
            .map_err(map_repair_core)?;
        let qualified: Vec<ReviewFinding> = findings
            .iter()
            .filter(|finding| {
                finding.disposition == FindingDisposition::ConfirmedBlocking
                    && matches!(finding.severity.as_str(), "blocker" | "high")
            })
            .cloned()
            .collect();
        let eligible: Vec<ReviewFinding> = qualified
            .iter()
            .filter(|finding| finding.repair_round_id.is_none())
            .take(1)
            .cloned()
            .collect();
        if eligible.is_empty() {
            return if qualified.is_empty() {
                Err(RepairError::NoEligibleFindings)
            } else {
                Err(RepairError::DuplicateHandoff)
            };
        }
        if eligible
            .iter()
            .any(|finding| repair_finding(finding).is_none())
        {
            return Err(RepairError::MalformedFinding);
        }
        let rounds = repository
            .v3()
            .list_repair_rounds(&task_id)
            .await
            .map_err(map_repair_core)?;
        let round = repository
            .v3()
            .create_repair_round(
                CreateRepairRound {
                    task_id: task_id.clone(),
                    round_number: rounds.len() as u32 + 1,
                },
                now(),
            )
            .await
            .map_err(map_repair_core)?;
        let active = repository
            .v3()
            .transition_repair_round(&round, RepairRoundLifecycle::Active, now())
            .await
            .map_err(map_repair_core)?;
        let mut assigned = Vec::with_capacity(eligible.len());
        for finding in &eligible {
            assigned.push(
                repository
                    .v3()
                    .assign_review_finding_to_repair_round(finding, &active, now())
                    .await
                    .map_err(map_repair_core)?,
            );
        }
        let validations = review_validations(&repository, &task_id).await?;
        let session = repository
            .v3()
            .list_sessions_for_task(&task_id)
            .await
            .map_err(map_repair_core)?
            .into_iter()
            .last();
        let packet = RepairPacket {
            repair_round_id: active.id.to_string(),
            task_id: task_id.to_string(),
            findings: assigned.iter().filter_map(repair_finding).collect(),
            validations,
            implementer_session_id: session.as_ref().map(|value| value.id.to_string()),
        };
        repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task_id.clone(),
                    kind: "repair_handoff".into(),
                    display_name: format!("repair-round-{}", active.round_number),
                    content_hash: None,
                    metadata: serde_json::to_value(&packet).map_err(|_| RepairError::Storage)?,
                },
                now(),
            )
            .await
            .map_err(map_repair_core)?;
        let evidence = implementer.repair(session.as_ref(), &packet)?;
        let awaiting = repository
            .v3()
            .transition_repair_round(&active, RepairRoundLifecycle::AwaitingValidation, now())
            .await
            .map_err(map_repair_core)?;
        let (_sender, mut cancellation) = tokio::sync::watch::channel(false);
        let validation = ValidationRunner::run_profile(
            repository.clone(),
            task_id.clone(),
            main,
            profile,
            &mut cancellation,
        )
        .await
        .map_err(|_| RepairError::Validation)?;
        let passed = validation
            .results
            .iter()
            .all(|result| result.outcome == "passed");
        let changed = WorktreeTransaction::diff(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| RepairError::Reconciliation)?;
        let changed_files: std::collections::HashSet<_> =
            changed.files.into_iter().map(|file| file.path).collect();
        let reported_files: std::collections::HashSet<_> =
            evidence.changed_files.into_iter().collect();
        let mut resolved = 0;
        if passed {
            for finding in assigned {
                let file = repair_finding(&finding).expect("prevalidated finding").file;
                if changed_files.contains(&file) && reported_files.contains(&file) {
                    repository
                        .v3()
                        .transition_review_finding(&finding, FindingDisposition::Repaired, now())
                        .await
                        .map_err(map_repair_core)?;
                    resolved += 1;
                }
            }
        }
        let unresolved = eligible.len() - resolved;
        let final_round = repository
            .v3()
            .transition_repair_round(
                &awaiting,
                if unresolved == 0 {
                    RepairRoundLifecycle::Resolved
                } else {
                    RepairRoundLifecycle::Exhausted
                },
                now(),
            )
            .await
            .map_err(map_repair_core)?;
        Ok(RepairReport {
            repair_round_id: final_round.id,
            resolved,
            unresolved,
            validation_passed: passed,
        })
    }
}
async fn review_validations(
    repository: &RunRepository,
    task_id: &TaskId,
) -> Result<Vec<ReviewValidation>, RepairError> {
    let mut values = Vec::new();
    for result in repository
        .v3()
        .list_validation_results(task_id)
        .await
        .map_err(map_repair_core)?
    {
        values.push(ReviewValidation {
            check_name: result.check_name,
            lifecycle: result.lifecycle,
            outcome: repository
                .v3()
                .get_validation_execution(&result.id)
                .await
                .ok()
                .map(|value| value.outcome),
        });
    }
    Ok(values)
}
fn repair_finding(finding: &ReviewFinding) -> Option<RepairFinding> {
    let file = finding.evidence.get("file")?.as_str()?;
    let line = finding
        .evidence
        .get("line")?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())?;
    let evidence = finding.evidence.get("evidence")?.as_str()?;
    (!file.trim().is_empty()
        && !file.starts_with('/')
        && !file.contains("..")
        && line > 0
        && !evidence.trim().is_empty())
    .then(|| RepairFinding {
        id: finding.id.to_string(),
        summary: finding.summary.clone(),
        file: file.into(),
        line,
        evidence: evidence.into(),
    })
}
fn map_repair_core(_: CoreError) -> RepairError {
    RepairError::Storage
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalApproval {
    pub approval: Approval,
    pub repair_rounds: usize,
}
/// Exact, executor-independent evidence required to use a final/destructive
/// action approval. Values are opaque identifiers/digests, never secrets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionApprovalContext {
    pub task_id: String,
    pub action: String,
    pub worktree_path: String,
    pub repository_root: String,
    pub branch: String,
    pub head: String,
    pub base_commit: String,
    pub evidence_digest: String,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActionAuthorizationError {
    #[error("approval capability is invalid or unavailable")]
    Denied,
    #[error("worktree or task recovery is required")]
    RecoveryRequired,
    #[error("authorization storage failure")]
    Storage,
}
pub struct ActionAuthorizationGuard;
impl ActionAuthorizationGuard {
    pub async fn issue(
        repository: RunRepository,
        context: ActionApprovalContext,
        timestamp: i64,
    ) -> Result<Approval, ActionAuthorizationError> {
        if context.task_id.trim().is_empty()
            || context.action.trim().is_empty()
            || context.evidence_digest.len() > 256
        {
            return Err(ActionAuthorizationError::Denied);
        }
        let payload =
            serde_json::to_string(&context).map_err(|_| ActionAuthorizationError::Storage)?;
        repository
            .v3()
            .create_approval(
                CreateApproval {
                    task_id: TaskId(context.task_id),
                    session_id: None,
                    action_kind: format!("v3_action:{}", context.action),
                    summary: payload,
                },
                timestamp,
            )
            .await
            .map_err(|_| ActionAuthorizationError::Storage)
    }
    pub async fn validate_and_consume(
        repository: RunRepository,
        approval_id: &ApprovalId,
        context: &ActionApprovalContext,
        timestamp: i64,
    ) -> Result<Approval, ActionAuthorizationError> {
        let approval = repository
            .v3()
            .get_approval(approval_id)
            .await
            .map_err(|_| ActionAuthorizationError::Denied)?;
        if approval.lifecycle != ApprovalLifecycle::Pending
            || approval.action_kind != format!("v3_action:{}", context.action)
            || approval.task_id.to_string() != context.task_id
        {
            return Err(ActionAuthorizationError::Denied);
        }
        let bound: ActionApprovalContext = serde_json::from_str(&approval.summary)
            .map_err(|_| ActionAuthorizationError::Denied)?;
        if &bound != context {
            return Err(ActionAuthorizationError::Denied);
        }
        let task = repository
            .v3()
            .get_task(&approval.task_id)
            .await
            .map_err(|_| ActionAuthorizationError::Storage)?;
        if task.recovery_condition != sentinel_core::v3::RecoveryCondition::None {
            return Err(ActionAuthorizationError::RecoveryRequired);
        }
        let worktree = repository
            .v3()
            .get_task_worktree(&approval.task_id)
            .await
            .map_err(|_| ActionAuthorizationError::Denied)?;
        if worktree.state != "ready"
            || worktree.worktree_path != context.worktree_path
            || worktree.repository_root != context.repository_root
            || worktree.branch != context.branch
            || worktree.base_commit != context.base_commit
            || context.head != context.base_commit
        {
            return Err(ActionAuthorizationError::Denied);
        }
        let consumed = repository
            .v3()
            .transition_approval(&approval, ApprovalLifecycle::Approved, timestamp)
            .await
            .map_err(|_| ActionAuthorizationError::Denied)?;
        repository.v3().create_artifact(CreateArtifact { task_id: approval.task_id.clone(), kind: "authorization_audit".into(), display_name: context.action.clone(), content_hash: None, metadata: serde_json::json!({"approval_id":approval.id.to_string(),"action":context.action,"evidence_digest":context.evidence_digest,"outcome":"consumed"}) }, timestamp).await.map_err(|_| ActionAuthorizationError::Storage)?;
        Ok(consumed)
    }
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FinalApprovalError {
    #[error("worktree reconciliation is required")]
    Reconciliation,
    #[error("implementer session is missing or recovery-required")]
    SessionUnavailable,
    #[error("deterministic validation did not pass")]
    ValidationFailed,
    #[error("confirmed blocker remains unresolved")]
    UnresolvedBlocker,
    #[error("repair/re-review round limit reached")]
    RoundLimitReached,
    #[error("human approval is required")]
    ApprovalRequired,
    #[error("approval was rejected")]
    ApprovalRejected,
    #[error("review, repair, or storage failure")]
    Storage,
}

/// Creates a durable approval request; it never performs a final Git action.
pub struct FinalApprovalSupervisor;
impl FinalApprovalSupervisor {
    pub async fn prepare(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        profile: ValidationProfile,
        reviewer: &dyn Reviewer,
        implementer: &dyn RepairImplementer,
        max_rounds: u32,
    ) -> Result<FinalApproval, FinalApprovalError> {
        if max_rounds == 0 {
            return Err(FinalApprovalError::RoundLimitReached);
        }
        WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| FinalApprovalError::Reconciliation)?;
        let sessions = repository
            .v3()
            .list_sessions_for_task(&task_id)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        if sessions.is_empty()
            || sessions
                .iter()
                .any(|session| session.lifecycle == SessionLifecycle::RecoveryRequired)
        {
            return Err(FinalApprovalError::SessionUnavailable);
        }
        ReviewSupervisor::run(repository.clone(), task_id.clone(), main, reviewer)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        let mut rounds = 0;
        loop {
            let findings = repository
                .v3()
                .list_review_findings(&task_id)
                .await
                .map_err(|_| FinalApprovalError::Storage)?;
            let confirmed = findings.iter().any(|finding| {
                finding.disposition == FindingDisposition::ConfirmedBlocking
                    && matches!(finding.severity.as_str(), "blocker" | "high")
            });
            if !confirmed {
                break;
            }
            if rounds >= max_rounds {
                return Err(FinalApprovalError::RoundLimitReached);
            }
            let repair = RepairSupervisor::handoff(
                repository.clone(),
                task_id.clone(),
                main,
                profile.clone(),
                implementer,
            )
            .await
            .map_err(|error| match error {
                RepairError::Reconciliation => FinalApprovalError::Reconciliation,
                RepairError::Validation => FinalApprovalError::ValidationFailed,
                RepairError::NoEligibleFindings
                | RepairError::DuplicateHandoff
                | RepairError::MalformedFinding => FinalApprovalError::UnresolvedBlocker,
                _ => FinalApprovalError::Storage,
            })?;
            rounds += 1;
            if !repair.validation_passed {
                return Err(FinalApprovalError::ValidationFailed);
            }
            if repair.unresolved > 0 {
                return Err(FinalApprovalError::UnresolvedBlocker);
            }
            // Every completed repair gets a fresh, read-only review pass.
            ReviewSupervisor::run(repository.clone(), task_id.clone(), main, reviewer)
                .await
                .map_err(|_| FinalApprovalError::Storage)?;
        }
        let findings = repository
            .v3()
            .list_review_findings(&task_id)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        if findings.iter().any(|finding| {
            matches!(finding.severity.as_str(), "blocker" | "high")
                && finding.disposition != FindingDisposition::Repaired
                && finding.disposition != FindingDisposition::Dismissed
        }) {
            return Err(FinalApprovalError::UnresolvedBlocker);
        }
        let (_sender, mut cancellation) = tokio::sync::watch::channel(false);
        let validation = ValidationRunner::run_profile(
            repository.clone(),
            task_id.clone(),
            main,
            profile,
            &mut cancellation,
        )
        .await
        .map_err(|_| FinalApprovalError::ValidationFailed)?;
        if !validation
            .results
            .iter()
            .all(|result| result.outcome == "passed")
        {
            return Err(FinalApprovalError::ValidationFailed);
        }
        let worktree = WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| FinalApprovalError::Reconciliation)?;
        let diff = WorktreeTransaction::diff_text(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| FinalApprovalError::Reconciliation)?;
        let validations = review_validations(&repository, &task_id)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        let history = repository
            .v3()
            .list_repair_rounds(&task_id)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        let unresolved: Vec<_> = findings
            .iter()
            .filter(|finding| {
                finding.disposition != FindingDisposition::Repaired
                    && finding.disposition != FindingDisposition::Dismissed
            })
            .map(|finding| {
                serde_json::json!({"id":finding.id.to_string(),"severity":finding.severity,"summary":finding.summary,"disposition":finding.disposition})
            })
            .collect();
        let finding_packet: Vec<_> = findings
            .iter()
            .map(|finding| {
                serde_json::json!({"id":finding.id.to_string(),"severity":finding.severity,"disposition":finding.disposition,"summary":finding.summary,"evidence":finding.evidence})
            })
            .collect();
        let repair_history: Vec<_> = history
            .iter()
            .map(|round| serde_json::json!({"id":round.id.to_string(),"round_number":round.round_number,"lifecycle":round.lifecycle,"created_at_ms":round.created_at_ms,"updated_at_ms":round.updated_at_ms}))
            .collect();
        repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task_id.clone(),
                    kind: "final_approval_packet".into(),
                    display_name: "final-approval".into(),
                    content_hash: None,
                    metadata: serde_json::json!({"base_commit":worktree.base_commit,"diff":diff,"validations":validations,"findings":finding_packet,"repair_history":repair_history,"unresolved_risks":unresolved}),
                },
                now(),
            )
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        let approval = repository
            .v3()
            .create_approval(
                CreateApproval {
                    task_id,
                    session_id: None,
                    action_kind: "v3_final_git_action".into(),
                    summary:
                        "Human approval is required before any commit, merge, push, or discard."
                            .into(),
                },
                now(),
            )
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        Ok(FinalApproval {
            approval,
            repair_rounds: rounds as usize,
        })
    }
    pub async fn require_human_approval(
        repository: RunRepository,
        approval_id: &ApprovalId,
        approve: bool,
    ) -> Result<Approval, FinalApprovalError> {
        let approval = repository
            .v3()
            .get_approval(approval_id)
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        if approval.lifecycle != ApprovalLifecycle::Pending {
            return Err(FinalApprovalError::ApprovalRequired);
        }
        let next = if approve {
            ApprovalLifecycle::Approved
        } else {
            ApprovalLifecycle::Denied
        };
        let updated = repository
            .v3()
            .transition_approval(&approval, next, now())
            .await
            .map_err(|_| FinalApprovalError::Storage)?;
        if !approve {
            return Err(FinalApprovalError::ApprovalRejected);
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{
        CreateSession, CreateTask, CreateValidationResult, ValidationExecution, ValidationLifecycle,
    };
    use std::{fs, path::Path, process::Command, sync::Mutex};
    use tempfile::TempDir;

    struct Fixed(Vec<ReviewCandidate>);
    impl Reviewer for Fixed {
        fn review(&self, _: &ReviewPacket) -> Result<Vec<ReviewCandidate>, ReviewError> {
            Ok(self.0.clone())
        }
    }
    fn finding() -> ReviewCandidate {
        ReviewCandidate {
            severity: FindingSeverity::Blocker,
            summary: "missing guard".into(),
            file: "x.rs".into(),
            line: 2,
            evidence: "the changed call lacks a guard".into(),
            validation_check: None,
            validation_outcome: None,
        }
    }
    fn repair_candidate() -> ReviewCandidate {
        ReviewCandidate {
            file: "changed.rs".into(),
            ..finding()
        }
    }
    fn profile(program: &str) -> ValidationProfile {
        ValidationProfile {
            id: "repair".into(),
            steps: vec![sentinel_validation::ValidationStep {
                name: "repair-check".into(),
                argv: vec![program.into()],
                kind: sentinel_validation::CheckKind::Test,
                cwd: ".".into(),
                env: Default::default(),
                required: true,
                timeout_ms: 500,
            }],
        }
    }
    struct Capturing {
        seen: Mutex<Option<ReviewPacket>>,
        findings: Vec<ReviewCandidate>,
    }
    impl Reviewer for Capturing {
        fn review(&self, packet: &ReviewPacket) -> Result<Vec<ReviewCandidate>, ReviewError> {
            *self.seen.lock().unwrap() = Some(packet.clone());
            Ok(self.findings.clone())
        }
    }
    struct Repairing {
        evidence: RepairEvidence,
        seen: Mutex<Option<RepairPacket>>,
    }
    impl RepairImplementer for Repairing {
        fn repair(
            &self,
            _session: Option<&AgentSession>,
            packet: &RepairPacket,
        ) -> Result<RepairEvidence, RepairError> {
            *self.seen.lock().unwrap() = Some(packet.clone());
            Ok(self.evidence.clone())
        }
    }
    fn git(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(directory)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    async fn fixture() -> (TempDir, TempDir, TempDir, RunRepository, TaskId) {
        let main = TempDir::new().unwrap();
        git(main.path(), &["init"]);
        git(
            main.path(),
            &["config", "user.email", "fixture@example.test"],
        );
        git(main.path(), &["config", "user.name", "Fixture"]);
        fs::write(main.path().join("README.md"), "fixture\n").unwrap();
        git(main.path(), &["add", "README.md"]);
        git(main.path(), &["commit", "-m", "fixture"]);
        let database = TempDir::new().unwrap();
        let db = format!("sqlite://{}", database.path().join("state.db").display());
        let repo = RunRepository::open(&db).await.unwrap();
        let task = repo
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "v3".into(),
                    summary: "review me".into(),
                },
                1,
            )
            .await
            .unwrap();
        let worktree_root = TempDir::new().unwrap();
        let worktree = WorktreeTransaction::create(
            repo.clone(),
            task.id.clone(),
            main.path(),
            worktree_root.path(),
        )
        .await
        .unwrap();
        fs::write(
            Path::new(&worktree.worktree_path).join("changed.rs"),
            "pub fn changed() {}\n",
        )
        .unwrap();
        (main, database, worktree_root, repo, task.id)
    }
    #[test]
    fn candidate_schema_requires_located_evidence() {
        let mut value = finding();
        value.evidence.clear();
        assert!(!valid(&value));
        value = finding();
        value.line = 0;
        assert!(!valid(&value));
    }
    #[test]
    fn deterministic_validation_overrides_model_claim() {
        let value = ReviewCandidate {
            validation_check: Some("test".into()),
            validation_outcome: Some("failed".into()),
            ..finding()
        };
        assert!(unsupported(
            &value,
            &[ReviewValidation {
                check_name: "test".into(),
                lifecycle: ValidationLifecycle::Passed,
                outcome: Some("passed".into())
            }]
        ));
    }
    #[tokio::test]
    async fn clean_review_receives_immutable_task_diff_and_validation_context() {
        let (main, _database, _root, repo, id) = fixture().await;
        let validation = repo
            .v3()
            .create_validation_result(
                CreateValidationResult {
                    task_id: id.clone(),
                    profile_id: "default".into(),
                    check_name: "test".into(),
                    required: true,
                    summary: None,
                },
                2,
            )
            .await
            .unwrap();
        let running = repo
            .v3()
            .transition_validation_result(&validation, ValidationLifecycle::Running, None, 3)
            .await
            .unwrap();
        let passed = repo
            .v3()
            .transition_validation_result(
                &running,
                ValidationLifecycle::Passed,
                Some("passed".into()),
                4,
            )
            .await
            .unwrap();
        repo.v3()
            .save_validation_execution(&ValidationExecution {
                validation_id: passed.id,
                command_json: "[\"true\"]".into(),
                exit_code: Some(0),
                duration_ms: 1,
                stdout: "".into(),
                stderr: "".into(),
                outcome: "passed".into(),
                started_at_ms: 3,
                finished_at_ms: 4,
            })
            .await
            .unwrap();
        let reviewer = Capturing {
            seen: Mutex::new(None),
            findings: vec![],
        };
        let report = ReviewSupervisor::run(repo.clone(), id.clone(), main.path(), &reviewer)
            .await
            .unwrap();
        assert_eq!(
            report,
            ReviewReport {
                persisted: 0,
                deduplicated: 0,
                rejected: 0
            }
        );
        let packet = reviewer.seen.lock().unwrap().clone().unwrap();
        assert!(packet.diff.contains("changed.rs"));
        assert!(packet
            .changed_files
            .iter()
            .any(|file| file.path == "changed.rs"));
        assert_eq!(packet.validations[0].outcome.as_deref(), Some("passed"));
        assert!(repo
            .v3()
            .list_review_findings(&id)
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn blocker_finding_is_persisted_with_location_evidence_and_disposition() {
        let (main, _database, _root, repo, id) = fixture().await;
        let report = ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            main.path(),
            &Fixed(vec![finding()]),
        )
        .await
        .unwrap();
        assert_eq!(report.persisted, 1);
        let stored = repo.v3().list_review_findings(&id).await.unwrap();
        assert_eq!(stored[0].severity, "blocker");
        assert_eq!(
            stored[0].disposition,
            sentinel_core::v3::FindingDisposition::Reported
        );
        assert_eq!(stored[0].evidence["file"], "x.rs");
        assert_eq!(stored[0].evidence["line"], 2);
    }
    #[tokio::test]
    async fn duplicate_findings_are_not_persisted_twice() {
        let (main, _database, _root, repo, id) = fixture().await;
        let report = ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            main.path(),
            &Fixed(vec![finding(), finding()]),
        )
        .await
        .unwrap();
        assert_eq!(
            report,
            ReviewReport {
                persisted: 1,
                deduplicated: 1,
                rejected: 0
            }
        );
        let second = ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            main.path(),
            &Fixed(vec![finding()]),
        )
        .await
        .unwrap();
        assert_eq!(
            second,
            ReviewReport {
                persisted: 0,
                deduplicated: 1,
                rejected: 0
            }
        );
        assert_eq!(repo.v3().list_review_findings(&id).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn malformed_and_unsupported_claims_fail_safely() {
        let (main, _database, _root, repo, id) = fixture().await;
        let mut malformed = finding();
        malformed.evidence.clear();
        assert_eq!(
            ReviewSupervisor::run(
                repo.clone(),
                id.clone(),
                main.path(),
                &Fixed(vec![malformed])
            )
            .await
            .unwrap_err(),
            ReviewError::MalformedFinding
        );
        let unsupported = ReviewCandidate {
            validation_check: Some("test".into()),
            validation_outcome: Some("passed".into()),
            ..finding()
        };
        let report = ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            main.path(),
            &Fixed(vec![unsupported]),
        )
        .await
        .unwrap();
        assert_eq!(report.rejected, 1);
        assert!(repo
            .v3()
            .list_review_findings(&id)
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn review_has_no_write_authority_and_never_finalizes_the_task() {
        let (main, _database, _root, repo, id) = fixture().await;
        let main_head = git(main.path(), &["rev-parse", "HEAD"]);
        let main_status = git(main.path(), &["status", "--porcelain"]);
        let task_before = repo.v3().get_task(&id).await.unwrap();
        ReviewSupervisor::run(repo.clone(), id.clone(), main.path(), &Fixed(vec![]))
            .await
            .unwrap();
        assert_eq!(git(main.path(), &["rev-parse", "HEAD"]), main_head);
        assert_eq!(git(main.path(), &["status", "--porcelain"]), main_status);
        assert_eq!(
            repo.v3().get_task(&id).await.unwrap().lifecycle,
            task_before.lifecycle
        );
    }
    #[tokio::test]
    async fn findings_survive_restart() {
        let (main, database, _root, repo, id) = fixture().await;
        ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            main.path(),
            &Fixed(vec![finding()]),
        )
        .await
        .unwrap();
        drop(repo);
        let reopened_url = format!("sqlite://{}", database.path().join("state.db").display());
        let reopened = RunRepository::open(&reopened_url).await.unwrap();
        let stored = reopened.v3().list_review_findings(&id).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].summary, "missing guard");
    }
    async fn confirmed_repair_finding(repo: &RunRepository, id: &TaskId) -> ReviewFinding {
        let main = repo
            .v3()
            .get_task_worktree(id)
            .await
            .unwrap()
            .repository_root;
        ReviewSupervisor::run(
            repo.clone(),
            id.clone(),
            Path::new(&main),
            &Fixed(vec![repair_candidate()]),
        )
        .await
        .unwrap();
        let finding = repo.v3().list_review_findings(id).await.unwrap().remove(0);
        repo.v3()
            .transition_review_finding(&finding, FindingDisposition::ConfirmedBlocking, 10)
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn confirmed_finding_hands_off_to_owned_session_and_resolves_after_validation() {
        let (main, _database, _root, repo, id) = fixture().await;
        let session = repo
            .v3()
            .create_session(
                CreateSession {
                    task_id: id.clone(),
                    provider: "openai.codex.app_server".into(),
                    provider_session_ref: "owned-thread".into(),
                },
                2,
            )
            .await
            .unwrap();
        let finding = confirmed_repair_finding(&repo, &id).await;
        let implementer = Repairing {
            evidence: RepairEvidence {
                changed_files: vec!["changed.rs".into()],
            },
            seen: Mutex::new(None),
        };
        let report = RepairSupervisor::handoff(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/true"),
            &implementer,
        )
        .await
        .unwrap();
        assert_eq!((report.resolved, report.unresolved), (1, 0));
        let packet = implementer.seen.lock().unwrap().clone().unwrap();
        assert_eq!(packet.findings[0].id, finding.id.to_string());
        assert_eq!(
            packet.implementer_session_id.as_deref(),
            Some(session.id.0.as_str())
        );
        assert_eq!(
            repo.v3()
                .get_review_finding(&finding.id)
                .await
                .unwrap()
                .disposition,
            FindingDisposition::Repaired
        );
        assert_eq!(
            repo.v3()
                .get_repair_round(&report.repair_round_id)
                .await
                .unwrap()
                .lifecycle,
            RepairRoundLifecycle::Resolved
        );
    }
    #[tokio::test]
    async fn rejected_and_duplicate_handoffs_fail_safely() {
        let (main, _database, _root, repo, id) = fixture().await;
        let implementer = Repairing {
            evidence: RepairEvidence {
                changed_files: vec!["changed.rs".into()],
            },
            seen: Mutex::new(None),
        };
        assert_eq!(
            RepairSupervisor::handoff(
                repo.clone(),
                id.clone(),
                main.path(),
                profile("/usr/bin/true"),
                &implementer
            )
            .await
            .unwrap_err(),
            RepairError::NoEligibleFindings
        );
        confirmed_repair_finding(&repo, &id).await;
        let first = RepairSupervisor::handoff(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/false"),
            &implementer,
        )
        .await
        .unwrap();
        assert_eq!(first.unresolved, 1);
        assert_eq!(
            RepairSupervisor::handoff(
                repo,
                id,
                main.path(),
                profile("/usr/bin/true"),
                &implementer
            )
            .await
            .unwrap_err(),
            RepairError::DuplicateHandoff
        );
    }
    #[tokio::test]
    async fn malformed_or_evidence_free_confirmed_findings_are_rejected() {
        let (main, _database, _root, repo, id) = fixture().await;
        let reported = repo
            .v3()
            .create_review_finding(
                CreateReviewFinding {
                    task_id: id.clone(),
                    repair_round_id: None,
                    severity: "blocker".into(),
                    summary: "unsupported claim".into(),
                    evidence: serde_json::json!({"file":"changed.rs","line":2,"evidence":""}),
                },
                2,
            )
            .await
            .unwrap();
        repo.v3()
            .transition_review_finding(&reported, FindingDisposition::ConfirmedBlocking, 3)
            .await
            .unwrap();
        let implementer = Repairing {
            evidence: RepairEvidence {
                changed_files: vec!["changed.rs".into()],
            },
            seen: Mutex::new(None),
        };
        assert_eq!(
            RepairSupervisor::handoff(
                repo,
                id,
                main.path(),
                profile("/usr/bin/true"),
                &implementer,
            )
            .await
            .unwrap_err(),
            RepairError::MalformedFinding
        );
    }
    #[tokio::test]
    async fn evidence_or_validation_cannot_resolve_a_finding_on_its_own() {
        let (main, _database, _root, repo, id) = fixture().await;
        let finding = confirmed_repair_finding(&repo, &id).await;
        let text_only = Repairing {
            evidence: RepairEvidence {
                changed_files: vec![],
            },
            seen: Mutex::new(None),
        };
        let report = RepairSupervisor::handoff(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/true"),
            &text_only,
        )
        .await
        .unwrap();
        assert_eq!((report.resolved, report.unresolved), (0, 1));
        assert_eq!(
            repo.v3()
                .get_review_finding(&finding.id)
                .await
                .unwrap()
                .disposition,
            FindingDisposition::ConfirmedBlocking
        );
    }
    #[tokio::test]
    async fn validation_failure_leaves_finding_unresolved_and_round_exhausted() {
        let (main, _database, _root, repo, id) = fixture().await;
        let finding = confirmed_repair_finding(&repo, &id).await;
        let implementer = Repairing {
            evidence: RepairEvidence {
                changed_files: vec!["changed.rs".into()],
            },
            seen: Mutex::new(None),
        };
        let report = RepairSupervisor::handoff(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/false"),
            &implementer,
        )
        .await
        .unwrap();
        assert_eq!((report.resolved, report.unresolved), (0, 1));
        assert_eq!(
            repo.v3()
                .get_review_finding(&finding.id)
                .await
                .unwrap()
                .disposition,
            FindingDisposition::ConfirmedBlocking
        );
        assert_eq!(
            repo.v3()
                .get_repair_round(&report.repair_round_id)
                .await
                .unwrap()
                .lifecycle,
            RepairRoundLifecycle::Exhausted
        );
    }
    #[tokio::test]
    async fn repair_round_and_finding_assignment_survive_restart() {
        let (main, database, _root, repo, id) = fixture().await;
        let finding = confirmed_repair_finding(&repo, &id).await;
        let implementer = Repairing {
            evidence: RepairEvidence {
                changed_files: vec!["changed.rs".into()],
            },
            seen: Mutex::new(None),
        };
        let report = RepairSupervisor::handoff(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/false"),
            &implementer,
        )
        .await
        .unwrap();
        drop(repo);
        let url = format!("sqlite://{}", database.path().join("state.db").display());
        let reopened = RunRepository::open(&url).await.unwrap();
        assert_eq!(
            reopened
                .v3()
                .get_review_finding(&finding.id)
                .await
                .unwrap()
                .repair_round_id,
            Some(report.repair_round_id.clone())
        );
        assert_eq!(
            reopened.v3().list_repair_rounds(&id).await.unwrap().len(),
            1
        );
    }
    async fn owned_session(repo: &RunRepository, id: &TaskId) -> AgentSession {
        repo.v3()
            .create_session(
                CreateSession {
                    task_id: id.clone(),
                    provider: "openai.codex.app_server".into(),
                    provider_session_ref: format!("owned-{id}"),
                },
                2,
            )
            .await
            .unwrap()
    }
    async fn second_confirmed_repair_finding(repo: &RunRepository, id: &TaskId, main: &Path) {
        let mut candidate = repair_candidate();
        candidate.summary = "second guard".into();
        ReviewSupervisor::run(repo.clone(), id.clone(), main, &Fixed(vec![candidate]))
            .await
            .unwrap();
        let finding = repo
            .v3()
            .list_review_findings(id)
            .await
            .unwrap()
            .into_iter()
            .find(|finding| finding.summary == "second guard")
            .unwrap();
        repo.v3()
            .transition_review_finding(&finding, FindingDisposition::ConfirmedBlocking, 11)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn clean_first_review_creates_final_human_approval_without_finalizing_task() {
        let (main, _database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        let before = repo.v3().get_task(&id).await.unwrap();
        let final_packet = FinalApprovalSupervisor::prepare(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/true"),
            &Fixed(vec![]),
            &Repairing {
                evidence: RepairEvidence {
                    changed_files: vec![],
                },
                seen: Mutex::new(None),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(final_packet.repair_rounds, 0);
        assert_eq!(final_packet.approval.lifecycle, ApprovalLifecycle::Pending);
        assert_eq!(
            repo.v3().get_task(&id).await.unwrap().lifecycle,
            before.lifecycle
        );
    }
    #[tokio::test]
    async fn one_repair_then_rereview_passes_to_final_approval() {
        let (main, _database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        confirmed_repair_finding(&repo, &id).await;
        let final_packet = FinalApprovalSupervisor::prepare(
            repo,
            id,
            main.path(),
            profile("/usr/bin/true"),
            &Fixed(vec![]),
            &Repairing {
                evidence: RepairEvidence {
                    changed_files: vec!["changed.rs".into()],
                },
                seen: Mutex::new(None),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(final_packet.repair_rounds, 1);
        assert_eq!(final_packet.approval.lifecycle, ApprovalLifecycle::Pending);
    }
    #[tokio::test]
    async fn multiple_confirmed_findings_use_bounded_sequential_repair_rounds() {
        let (main, _database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        confirmed_repair_finding(&repo, &id).await;
        second_confirmed_repair_finding(&repo, &id, main.path()).await;
        let final_packet = FinalApprovalSupervisor::prepare(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/true"),
            &Fixed(vec![]),
            &Repairing {
                evidence: RepairEvidence {
                    changed_files: vec!["changed.rs".into()],
                },
                seen: Mutex::new(None),
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(final_packet.repair_rounds, 2);
        assert_eq!(repo.v3().list_repair_rounds(&id).await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn round_limit_preserves_unresolved_blocker() {
        let (main, _database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        confirmed_repair_finding(&repo, &id).await;
        second_confirmed_repair_finding(&repo, &id, main.path()).await;
        assert_eq!(
            FinalApprovalSupervisor::prepare(
                repo.clone(),
                id.clone(),
                main.path(),
                profile("/usr/bin/true"),
                &Fixed(vec![]),
                &Repairing {
                    evidence: RepairEvidence {
                        changed_files: vec!["changed.rs".into()]
                    },
                    seen: Mutex::new(None)
                },
                1,
            )
            .await
            .unwrap_err(),
            FinalApprovalError::RoundLimitReached
        );
        assert!(repo
            .v3()
            .list_review_findings(&id)
            .await
            .unwrap()
            .iter()
            .any(|finding| finding.disposition == FindingDisposition::ConfirmedBlocking));
    }
    #[tokio::test]
    async fn validation_failure_and_unresolved_blocker_stop_safely() {
        let (main, _database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        assert_eq!(
            FinalApprovalSupervisor::prepare(
                repo.clone(),
                id.clone(),
                main.path(),
                profile("/usr/bin/false"),
                &Fixed(vec![]),
                &Repairing {
                    evidence: RepairEvidence {
                        changed_files: vec![]
                    },
                    seen: Mutex::new(None)
                },
                1,
            )
            .await
            .unwrap_err(),
            FinalApprovalError::ValidationFailed
        );
        confirmed_repair_finding(&repo, &id).await;
        assert_eq!(
            FinalApprovalSupervisor::prepare(
                repo,
                id,
                main.path(),
                profile("/usr/bin/true"),
                &Fixed(vec![]),
                &Repairing {
                    evidence: RepairEvidence {
                        changed_files: vec![]
                    },
                    seen: Mutex::new(None)
                },
                1,
            )
            .await
            .unwrap_err(),
            FinalApprovalError::UnresolvedBlocker
        );
    }
    #[tokio::test]
    async fn missing_or_recovery_required_session_refuses_final_orchestration() {
        let (main, _database, _root, repo, id) = fixture().await;
        assert_eq!(
            FinalApprovalSupervisor::prepare(
                repo.clone(),
                id.clone(),
                main.path(),
                profile("/usr/bin/true"),
                &Fixed(vec![]),
                &Repairing {
                    evidence: RepairEvidence {
                        changed_files: vec![]
                    },
                    seen: Mutex::new(None)
                },
                1,
            )
            .await
            .unwrap_err(),
            FinalApprovalError::SessionUnavailable
        );
        let session = owned_session(&repo, &id).await;
        repo.v3()
            .transition_session(&session, SessionLifecycle::RecoveryRequired, 3)
            .await
            .unwrap();
        assert_eq!(
            FinalApprovalSupervisor::prepare(
                repo,
                id,
                main.path(),
                profile("/usr/bin/true"),
                &Fixed(vec![]),
                &Repairing {
                    evidence: RepairEvidence {
                        changed_files: vec![]
                    },
                    seen: Mutex::new(None)
                },
                1,
            )
            .await
            .unwrap_err(),
            FinalApprovalError::SessionUnavailable
        );
    }
    #[tokio::test]
    async fn final_approval_requires_explicit_human_decision_and_survives_restart() {
        let (main, database, _root, repo, id) = fixture().await;
        owned_session(&repo, &id).await;
        let final_packet = FinalApprovalSupervisor::prepare(
            repo.clone(),
            id.clone(),
            main.path(),
            profile("/usr/bin/true"),
            &Fixed(vec![]),
            &Repairing {
                evidence: RepairEvidence {
                    changed_files: vec![],
                },
                seen: Mutex::new(None),
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(
            FinalApprovalSupervisor::require_human_approval(
                repo.clone(),
                &final_packet.approval.id,
                false
            )
            .await
            .unwrap_err(),
            FinalApprovalError::ApprovalRejected
        );
        assert_eq!(
            repo.v3()
                .get_approval(&final_packet.approval.id)
                .await
                .unwrap()
                .lifecycle,
            ApprovalLifecycle::Denied
        );
        drop(repo);
        let url = format!("sqlite://{}", database.path().join("state.db").display());
        let reopened = RunRepository::open(&url).await.unwrap();
        assert_eq!(
            reopened
                .v3()
                .get_approval(&final_packet.approval.id)
                .await
                .unwrap()
                .lifecycle,
            ApprovalLifecycle::Denied
        );
    }
}
