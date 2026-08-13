//! Read-only, supervisor-owned V3 review packets and structured findings.
//!
//! A reviewer receives facts, never a filesystem handle or a command capability.
//! This makes the review boundary intentionally incapable of changing a task tree.

use sentinel_core::{
    v3::{CreateReviewFinding, TaskId, ValidationLifecycle},
    CoreError, RunRepository,
};
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
        let packet = ReviewPacket {
            task_id: task_id.to_string(),
            task_summary: task.summary,
            base_commit: worktree.base_commit,
            diff: WorktreeTransaction::diff_text(repository.clone(), task_id.clone(), main)
                .await
                .map_err(|_| ReviewError::Reconciliation)?,
            changed_files: diff.files,
            validations: facts,
        };
        let candidates = reviewer.review(&packet)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{
        CreateTask, CreateValidationResult, ValidationExecution, ValidationLifecycle,
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
}
