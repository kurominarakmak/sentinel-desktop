use sentinel_core::{
    v3::{
        ApprovalId, ApprovalLifecycle, CreateTaskWorktree, CreateTaskWorktreeMergePreparation,
        TaskId, TaskWorktree, TaskWorktreeCleanupOutcome, TaskWorktreeMergePreparation,
    },
    CoreError, RunRepository,
};
use sentinel_git::{inspect_repository, resolve_exact_head, RepositoryState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum TransactionError {
    #[error("invalid worktree request")]
    Invalid,
    #[error("worktree ownership conflict")]
    Conflict,
    #[error("worktree recovery is required")]
    RecoveryRequired,
    #[error("approved destructive action is required")]
    ApprovalRequired,
    #[error("destructive preflight refused")]
    PreflightRefused,
    #[error("git failure")]
    Git,
    #[error("storage failure")]
    Storage,
}
pub struct WorktreeTransaction;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFileState {
    pub path: String,
    pub index_status: Option<String>,
    pub worktree_status: Option<String>,
    pub untracked: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeDiff {
    pub files: Vec<ChangedFileState>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewWorktreeEvidence {
    pub base_commit: String,
    pub revision: String,
    pub diff_text: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePreparation {
    pub diff: WorktreeDiff,
    pub persisted: TaskWorktreeMergePreparation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupResult {
    pub outcome: TaskWorktreeCleanupOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationResult {
    pub source_commit: String,
    pub target_head_before: String,
    pub resulting_target_commit: String,
    pub target_branch: String,
}
impl WorktreeTransaction {
    /// Sentinel-owned Git integration. Providers never receive this capability.
    pub async fn integrate_verified(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        expected_branch: &str,
        expected_target_head: &str,
    ) -> Result<IntegrationResult, TransactionError> {
        let worktree = Self::reopen(repository, task_id.clone(), main).await?;
        let inspection = inspect_repository(main)
            .await
            .map_err(|_| TransactionError::PreflightRefused)?;
        if !inspection.is_primary || inspection.branch.as_deref() != Some(expected_branch) {
            return Err(TransactionError::PreflightRefused);
        }
        if !git_output(main, &["status", "--porcelain"])
            .await?
            .is_empty()
        {
            return Err(TransactionError::PreflightRefused);
        }
        let target_head_before =
            git_output(main, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
        if target_head_before != expected_target_head {
            return Err(TransactionError::PreflightRefused);
        }
        let worktree_path = Path::new(&worktree.worktree_path);
        if git_output(worktree_path, &["status", "--porcelain"])
            .await?
            .is_empty()
        {
            return Err(TransactionError::PreflightRefused);
        }
        git_success(worktree_path, &["add", "-A"]).await?;
        let message = format!("sentinel: integrate task {}", task_id);
        git_success(worktree_path, &["commit", "--no-verify", "-m", &message]).await?;
        let source_commit =
            git_output(worktree_path, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
        if !git_output(main, &["status", "--porcelain"])
            .await?
            .is_empty()
            || git_output(main, &["rev-parse", "--verify", "HEAD^{commit}"]).await?
                != target_head_before
        {
            return Err(TransactionError::PreflightRefused);
        }
        let cherry_pick = Command::new("/usr/bin/git")
            .current_dir(main)
            .args(["cherry-pick", "--no-edit", &source_commit])
            .output()
            .await
            .map_err(|_| TransactionError::Git)?;
        if !cherry_pick.status.success() {
            let _ = Command::new("/usr/bin/git")
                .current_dir(main)
                .args(["cherry-pick", "--abort"])
                .output()
                .await;
            return Err(TransactionError::PreflightRefused);
        }
        let resulting_target_commit =
            git_output(main, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
        Ok(IntegrationResult {
            source_commit,
            target_head_before,
            resulting_target_commit,
            target_branch: expected_branch.into(),
        })
    }
    pub async fn diff(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
    ) -> Result<WorktreeDiff, TransactionError> {
        let worktree = Self::reopen(repository, task_id, main).await?;
        diff_at_base(Path::new(&worktree.worktree_path), &worktree.base_commit).await
    }
    /// Obtain an immutable review diff. No Git mutation command is invoked.
    pub async fn diff_text(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
    ) -> Result<String, TransactionError> {
        let worktree = Self::reopen(repository, task_id, main).await?;
        let mut text = git_output(
            Path::new(&worktree.worktree_path),
            &["diff", "--no-ext-diff", "--binary", &worktree.base_commit],
        )
        .await?;
        // Git's normal diff intentionally omits untracked content. Preserve
        // their presence as review evidence without reading their contents.
        for file in diff_at_base(Path::new(&worktree.worktree_path), &worktree.base_commit)
            .await?
            .files
            .into_iter()
            .filter(|file| file.untracked)
        {
            text.push_str("\n# untracked file: ");
            text.push_str(&file.path);
            let path = Path::new(&worktree.worktree_path).join(&file.path);
            if std::fs::symlink_metadata(&path)
                .map_err(|_| TransactionError::Git)?
                .file_type()
                .is_symlink()
            {
                return Err(TransactionError::Git);
            }
            let bytes = std::fs::read(path).map_err(|_| TransactionError::Git)?;
            text.push_str(" sha256:");
            text.push_str(&format!("{:x}", Sha256::digest(bytes)));
        }
        Ok(text)
    }
    /// Read-only evidence used to bind a validation run to its exact review input.
    pub async fn review_evidence(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
    ) -> Result<ReviewWorktreeEvidence, TransactionError> {
        let worktree = Self::reopen(repository.clone(), task_id.clone(), main).await?;
        let revision = git_output(
            Path::new(&worktree.worktree_path),
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .await?;
        let diff_text = Self::diff_text(repository, task_id, main).await?;
        Ok(ReviewWorktreeEvidence {
            base_commit: worktree.base_commit,
            revision,
            diff_text,
        })
    }
    pub async fn create(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        root: &Path,
    ) -> Result<TaskWorktree, TransactionError> {
        match repository.v3().get_task_worktree(&task_id).await {
            Ok(_) => return Self::reopen(repository, task_id, main).await,
            Err(CoreError::NotFound) => {}
            Err(_) => return Err(TransactionError::Storage),
        }
        let inspection = inspect_repository(main)
            .await
            .map_err(|_| TransactionError::Invalid)?;
        if !inspection.is_primary || !matches!(inspection.state, RepositoryState::Valid) {
            return Err(TransactionError::Invalid);
        }
        let base = resolve_exact_head(main)
            .await
            .map_err(|_| TransactionError::Git)?;
        let root = root.canonicalize().map_err(|_| TransactionError::Invalid)?;
        let path = root.join(task_id.to_string());
        let branch = format!("agent-sentinel/v3-{}", task_id);
        if path.starts_with(&inspection.primary_root) {
            return Err(TransactionError::Invalid);
        }
        if repository
            .v3()
            .find_task_worktree_by_path(path.to_str().ok_or(TransactionError::Invalid)?)
            .await
            .map_err(map_core)?
            .is_some()
            || repository
                .v3()
                .find_task_worktree_by_branch(&branch)
                .await
                .map_err(map_core)?
                .is_some()
        {
            return Err(TransactionError::Conflict);
        }
        let output = Command::new("/usr/bin/git")
            .current_dir(&inspection.primary_root)
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_str().ok_or(TransactionError::Invalid)?,
                &base,
            ])
            .output()
            .await
            .map_err(|_| TransactionError::Git)?;
        if !output.status.success() {
            return Err(TransactionError::Conflict);
        }
        repository
            .v3()
            .create_task_worktree(
                CreateTaskWorktree {
                    task_id,
                    repository_root: inspection.primary_root.to_string_lossy().into(),
                    worktree_path: path.to_string_lossy().into(),
                    branch,
                    base_commit: base,
                },
                now(),
            )
            .await
            .map_err(map_core)
    }

    pub async fn reopen(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
    ) -> Result<TaskWorktree, TransactionError> {
        let stored = repository
            .v3()
            .get_task_worktree(&task_id)
            .await
            .map_err(map_core)?;
        if stored.state != "ready" {
            return Err(TransactionError::RecoveryRequired);
        }
        let main_inspection = inspect_repository(main)
            .await
            .map_err(|_| TransactionError::Invalid)?;
        let matches = main_inspection.is_primary
            && matches!(main_inspection.state, RepositoryState::Valid)
            && stored.repository_root == main_inspection.primary_root.to_string_lossy()
            && persisted_worktree_matches(&stored, &main_inspection.primary_root).await;
        if matches {
            Ok(stored)
        } else {
            repository
                .v3()
                .set_task_worktree_recovery_required(&task_id, now())
                .await
                .map_err(map_core)?;
            Err(TransactionError::RecoveryRequired)
        }
    }

    pub async fn prepare_merge(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        target_branch: &str,
    ) -> Result<MergePreparation, TransactionError> {
        let worktree = Self::reopen(repository.clone(), task_id.clone(), main).await?;
        let main_inspection = inspect_repository(main)
            .await
            .map_err(|_| TransactionError::Invalid)?;
        if !main_inspection.is_primary || main_inspection.branch.as_deref() != Some(target_branch) {
            return Err(TransactionError::Invalid);
        }
        let diff = diff_at_base(Path::new(&worktree.worktree_path), &worktree.base_commit).await?;
        let target_commit = git_output(main, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
        let worktree_commit = git_output(
            Path::new(&worktree.worktree_path),
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .await?;
        let target_advanced = target_commit != worktree.base_commit;
        let conflicts = merge_conflicts(
            Path::new(&worktree.worktree_path),
            &target_commit,
            &worktree_commit,
        )
        .await?;
        let persisted = repository
            .v3()
            .save_task_worktree_merge_preparation(
                CreateTaskWorktreeMergePreparation {
                    task_id,
                    target_branch: target_branch.into(),
                    target_commit,
                    worktree_commit,
                    target_advanced,
                    merge_ready: conflicts.is_empty(),
                    conflicts_json: serde_json::to_string(&conflicts)
                        .map_err(|_| TransactionError::Storage)?,
                    diff_json: serde_json::to_string(&diff.files)
                        .map_err(|_| TransactionError::Storage)?,
                },
                now(),
            )
            .await
            .map_err(map_core)?;
        Ok(MergePreparation { diff, persisted })
    }

    pub async fn discard(
        repository: RunRepository,
        task_id: TaskId,
        main: &Path,
        approval_id: ApprovalId,
        approval_version: u64,
        approved_head: &str,
    ) -> Result<CleanupResult, TransactionError> {
        if let Ok(outcome) = repository
            .v3()
            .get_task_worktree_cleanup_outcome(&task_id)
            .await
        {
            if outcome.state == "discarded" {
                return Ok(CleanupResult { outcome });
            }
        }
        let approval = match repository.v3().get_approval(&approval_id).await {
            Ok(approval) => approval,
            Err(CoreError::NotFound) => return Err(TransactionError::ApprovalRequired),
            Err(_) => return Err(TransactionError::Storage),
        };
        if approval.task_id != task_id
            || approval.action_kind != "worktree_discard"
            || approval.lifecycle != ApprovalLifecycle::Approved
            || approval.version != approval_version
        {
            return Err(TransactionError::ApprovalRequired);
        }
        let worktree = match Self::reopen(repository.clone(), task_id.clone(), main).await {
            Ok(worktree) => worktree,
            Err(error) => {
                return retain(
                    repository,
                    &task_id,
                    Some(&approval_id),
                    "identity_unverified",
                    error,
                )
                .await
            }
        };
        let path = Path::new(&worktree.worktree_path);
        let current_head = match git_output(path, &["rev-parse", "--verify", "HEAD^{commit}"]).await
        {
            Ok(head) if head == approved_head => head,
            _ => {
                return retain(
                    repository,
                    &task_id,
                    Some(&approval_id),
                    "head_changed",
                    TransactionError::PreflightRefused,
                )
                .await
            }
        };
        if !git_output(path, &["status", "--porcelain"])
            .await?
            .is_empty()
        {
            return retain(
                repository,
                &task_id,
                Some(&approval_id),
                "dirty_worktree",
                TransactionError::PreflightRefused,
            )
            .await;
        }
        let main_inspection = inspect_repository(main)
            .await
            .map_err(|_| TransactionError::PreflightRefused)?;
        if !main_inspection.is_primary
            || main_inspection.primary_root.to_string_lossy() != worktree.repository_root
            || current_head != approved_head
        {
            return retain(
                repository,
                &task_id,
                Some(&approval_id),
                "primary_or_identity_changed",
                TransactionError::PreflightRefused,
            )
            .await;
        }
        let removal = Command::new("/usr/bin/git")
            .current_dir(&main_inspection.primary_root)
            .args([
                "worktree",
                "remove",
                path.to_str().ok_or(TransactionError::PreflightRefused)?,
            ])
            .output()
            .await
            .map_err(|_| TransactionError::Git)?;
        if !removal.status.success() {
            return retain(
                repository,
                &task_id,
                Some(&approval_id),
                "worktree_remove_failed",
                TransactionError::Git,
            )
            .await;
        }
        let branch = Command::new("/usr/bin/git")
            .current_dir(&main_inspection.primary_root)
            .args(["branch", "-D", &worktree.branch])
            .output()
            .await
            .map_err(|_| TransactionError::Git)?;
        if !branch.status.success() {
            return retain(
                repository,
                &task_id,
                Some(&approval_id),
                "branch_remove_failed",
                TransactionError::Git,
            )
            .await;
        }
        let outcome = repository
            .v3()
            .save_task_worktree_cleanup_outcome(
                &task_id,
                "discarded",
                None,
                Some(&approval_id),
                now(),
            )
            .await
            .map_err(map_core)?;
        Ok(CleanupResult { outcome })
    }
}

async fn retain(
    repository: RunRepository,
    task_id: &TaskId,
    approval_id: Option<&ApprovalId>,
    reason: &str,
    error: TransactionError,
) -> Result<CleanupResult, TransactionError> {
    repository
        .v3()
        .save_task_worktree_cleanup_outcome(task_id, "retained", Some(reason), approval_id, now())
        .await
        .map_err(map_core)?;
    Err(error)
}

async fn persisted_worktree_matches(worktree: &TaskWorktree, primary_root: &Path) -> bool {
    let path = Path::new(&worktree.worktree_path);
    let Ok(inspection) = inspect_repository(path).await else {
        return false;
    };
    if inspection.is_primary
        || !matches!(inspection.state, RepositoryState::LinkedWorktree)
        || inspection.primary_root != primary_root
        || inspection.branch.as_deref() != Some(worktree.branch.as_str())
    {
        return false;
    }
    git_succeeds(
        path,
        &["merge-base", "--is-ancestor", &worktree.base_commit, "HEAD"],
    )
    .await
}

async fn diff_at_base(
    worktree: &Path,
    base_commit: &str,
) -> Result<WorktreeDiff, TransactionError> {
    let output = git_output_bytes(worktree, &["status", "--porcelain=v1", "-z"]).await?;
    let mut files = Vec::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        if record.len() < 4 {
            return Err(TransactionError::Git);
        }
        let status = std::str::from_utf8(&record[..2]).map_err(|_| TransactionError::Git)?;
        let path = std::str::from_utf8(&record[3..]).map_err(|_| TransactionError::Git)?;
        // `status --porcelain` collapses an untracked directory to one
        // `?? directory/` record. Do not preserve that synthetic directory as
        // review evidence: `diff_text` must hash individual files.
        if status == "??" {
            continue;
        }
        files.push(ChangedFileState {
            path: path.into(),
            index_status: (&status[0..1] != " ").then(|| status[0..1].into()),
            worktree_status: (&status[1..2] != " ").then(|| status[1..2].into()),
            untracked: false,
        });
    }
    let untracked = git_output_bytes(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(path).map_err(|_| TransactionError::Git)?;
        files.push(ChangedFileState {
            path: path.into(),
            index_status: None,
            worktree_status: None,
            untracked: true,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let base_diff = git_output(worktree, &["diff", "--name-status", base_commit]).await?;
    for line in base_diff.lines() {
        let Some((status, path)) = line.split_once('\t') else {
            return Err(TransactionError::Git);
        };
        if files.iter().all(|file| file.path != path) {
            files.push(ChangedFileState {
                path: path.into(),
                index_status: Some(status.into()),
                worktree_status: None,
                untracked: false,
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(WorktreeDiff { files })
}

async fn merge_conflicts(
    worktree: &Path,
    target: &str,
    task: &str,
) -> Result<Vec<String>, TransactionError> {
    let output = Command::new("/usr/bin/git")
        .current_dir(worktree)
        .args(["merge-tree", "--write-tree", target, task])
        .output()
        .await
        .map_err(|_| TransactionError::Git)?;
    if output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut conflicts: Vec<String> = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("CONFLICT (content): Merge conflict in ")
                .map(str::to_owned)
        })
        .collect();
    if conflicts.is_empty() {
        conflicts.push("merge_conflict".into());
    }
    Ok(conflicts)
}
async fn git_output(directory: &Path, args: &[&str]) -> Result<String, TransactionError> {
    String::from_utf8(git_output_bytes(directory, args).await?)
        .map(|value| value.trim().into())
        .map_err(|_| TransactionError::Git)
}
async fn git_output_bytes(directory: &Path, args: &[&str]) -> Result<Vec<u8>, TransactionError> {
    let output = Command::new("/usr/bin/git")
        .current_dir(directory)
        .args(args)
        .output()
        .await
        .map_err(|_| TransactionError::Git)?;
    output
        .status
        .success()
        .then_some(output.stdout)
        .ok_or(TransactionError::Git)
}
async fn git_success(directory: &Path, args: &[&str]) -> Result<(), TransactionError> {
    git_output_bytes(directory, args).await.map(|_| ())
}
async fn git_succeeds(directory: &Path, args: &[&str]) -> bool {
    Command::new("/usr/bin/git")
        .current_dir(directory)
        .args(args)
        .output()
        .await
        .is_ok_and(|output| output.status.success())
}

fn map_core(_: CoreError) -> TransactionError {
    TransactionError::Storage
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{CreateTask, CreateTaskWorktree};
    use std::{fs, process::Command as StdCommand};
    use tempfile::TempDir;

    fn git(directory: &Path, args: &[&str]) -> String {
        let output = StdCommand::new("git")
            .current_dir(directory)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8 git output")
    }

    async fn fixture() -> (TempDir, TempDir, RunRepository, TaskId, String) {
        let main = tempfile::tempdir().expect("main repository");
        git(main.path(), &["init"]);
        git(
            main.path(),
            &["config", "user.email", "fixture@example.test"],
        );
        git(main.path(), &["config", "user.name", "Fixture"]);
        fs::write(main.path().join("README.md"), "fixture\n").expect("fixture file");
        git(main.path(), &["add", "README.md"]);
        git(main.path(), &["commit", "-m", "fixture"]);
        let base = git(main.path(), &["rev-parse", "HEAD"]).trim().to_owned();

        let database = tempfile::tempdir().expect("database directory");
        let database_url = format!(
            "sqlite://{}",
            database.path().join("sentinel.sqlite").display()
        );
        let repository = RunRepository::open(&database_url)
            .await
            .expect("open repository");
        let task = create_task(&repository).await;
        (main, database, repository, task, base)
    }

    async fn create_task(repository: &RunRepository) -> TaskId {
        repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "default".into(),
                    summary: "Create worktree".into(),
                },
                1,
            )
            .await
            .expect("create task")
            .id
    }

    #[tokio::test]
    async fn creates_task_branch_from_the_pinned_base() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");

        let created = WorktreeTransaction::create(repository, task_id, main.path(), root.path())
            .await
            .expect("create worktree");

        assert_eq!(created.base_commit, base);
        assert_eq!(
            git(Path::new(&created.worktree_path), &["rev-parse", "HEAD"]).trim(),
            base
        );
        assert_eq!(
            git(
                Path::new(&created.worktree_path),
                &["branch", "--show-current"]
            )
            .trim(),
            created.branch
        );
    }

    #[tokio::test]
    async fn verified_integration_commits_once_and_refuses_a_duplicate() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        fs::write(
            Path::new(&created.worktree_path).join("dashboard.txt"),
            "verified\n",
        )
        .unwrap();
        let branch = git(main.path(), &["branch", "--show-current"])
            .trim()
            .to_owned();
        let result = WorktreeTransaction::integrate_verified(
            repository.clone(),
            task_id.clone(),
            main.path(),
            &branch,
            &base,
        )
        .await
        .expect("integrate");
        assert_eq!(result.target_head_before, base);
        assert_eq!(
            git(main.path(), &["rev-parse", "HEAD"]).trim(),
            result.resulting_target_commit
        );
        assert_eq!(
            fs::read_to_string(main.path().join("dashboard.txt")).unwrap(),
            "verified\n"
        );
        assert!(WorktreeTransaction::integrate_verified(
            repository,
            task_id,
            main.path(),
            &branch,
            &result.target_head_before
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn verified_integration_fails_closed_for_dirty_or_moved_target() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        fs::write(
            Path::new(&created.worktree_path).join("dashboard.txt"),
            "verified\n",
        )
        .unwrap();
        let branch = git(main.path(), &["branch", "--show-current"])
            .trim()
            .to_owned();
        fs::write(main.path().join("local.txt"), "dirty\n").unwrap();
        assert!(WorktreeTransaction::integrate_verified(
            repository.clone(),
            task_id.clone(),
            main.path(),
            &branch,
            &base
        )
        .await
        .is_err());
        fs::remove_file(main.path().join("local.txt")).unwrap();
        fs::write(main.path().join("target.txt"), "moved\n").unwrap();
        git(main.path(), &["add", "target.txt"]);
        git(main.path(), &["commit", "-m", "target moved"]);
        assert!(WorktreeTransaction::integrate_verified(
            repository,
            task_id,
            main.path(),
            &branch,
            &base
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn persists_task_worktree_metadata() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");

        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        let stored = repository
            .v3()
            .get_task_worktree(&task_id)
            .await
            .expect("stored worktree");

        assert_eq!(stored, created);
        assert_eq!(
            stored.repository_root,
            main.path()
                .canonicalize()
                .expect("canonical main")
                .to_string_lossy()
        );
        assert_eq!(stored.base_commit, base);
        assert_eq!(stored.state, "ready");
    }

    #[tokio::test]
    async fn leaves_the_main_worktree_untouched() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let branch = git(main.path(), &["branch", "--show-current"]);
        let head = git(main.path(), &["rev-parse", "HEAD"]);
        let status = git(main.path(), &["status", "--porcelain"]);

        WorktreeTransaction::create(repository, task_id, main.path(), root.path())
            .await
            .expect("create worktree");

        assert_eq!(git(main.path(), &["branch", "--show-current"]), branch);
        assert_eq!(git(main.path(), &["rev-parse", "HEAD"]), head);
        assert_eq!(git(main.path(), &["status", "--porcelain"]), status);
    }

    #[tokio::test]
    async fn duplicate_retry_reopens_the_same_worktree() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("first create");

        let retried = WorktreeTransaction::create(repository, task_id, main.path(), root.path())
            .await
            .expect("retry");

        assert_eq!(retried, created);
    }

    #[tokio::test]
    async fn rejects_a_persisted_path_owned_by_another_task() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let other_task = create_task(&repository).await;
        repository
            .v3()
            .create_task_worktree(
                CreateTaskWorktree {
                    task_id: other_task,
                    repository_root: main
                        .path()
                        .canonicalize()
                        .expect("canonical main")
                        .to_string_lossy()
                        .into(),
                    worktree_path: root
                        .path()
                        .canonicalize()
                        .expect("canonical root")
                        .join(task_id.to_string())
                        .to_string_lossy()
                        .into(),
                    branch: "agent-sentinel/v3-other".into(),
                    base_commit: base,
                },
                2,
            )
            .await
            .expect("persist conflict");

        assert!(matches!(
            WorktreeTransaction::create(repository, task_id, main.path(), root.path()).await,
            Err(TransactionError::Conflict)
        ));
    }

    #[tokio::test]
    async fn rejects_a_persisted_branch_owned_by_another_task() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let other_task = create_task(&repository).await;
        repository
            .v3()
            .create_task_worktree(
                CreateTaskWorktree {
                    task_id: other_task,
                    repository_root: main
                        .path()
                        .canonicalize()
                        .expect("canonical main")
                        .to_string_lossy()
                        .into(),
                    worktree_path: root
                        .path()
                        .canonicalize()
                        .expect("canonical root")
                        .join("other")
                        .to_string_lossy()
                        .into(),
                    branch: format!("agent-sentinel/v3-{task_id}"),
                    base_commit: base,
                },
                2,
            )
            .await
            .expect("persist conflict");

        assert!(matches!(
            WorktreeTransaction::create(repository, task_id, main.path(), root.path()).await,
            Err(TransactionError::Conflict)
        ));
    }

    #[tokio::test]
    async fn reopens_persisted_worktree_after_restart() {
        let (main, database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        drop(repository);
        let database_url = format!(
            "sqlite://{}",
            database.path().join("sentinel.sqlite").display()
        );
        let restarted = RunRepository::open(&database_url)
            .await
            .expect("reopen repository");

        let reopened = WorktreeTransaction::reopen(restarted, task_id, main.path())
            .await
            .expect("reopen persisted worktree");

        assert_eq!(reopened, created);
    }

    #[tokio::test]
    async fn missing_worktree_becomes_recovery_required() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        fs::rename(&created.worktree_path, root.path().join("moved")).expect("move worktree");

        assert!(matches!(
            WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main.path()).await,
            Err(TransactionError::RecoveryRequired)
        ));
        assert_eq!(
            repository
                .v3()
                .get_task_worktree(&task_id)
                .await
                .expect("stored worktree")
                .state,
            "recovery_required"
        );
    }

    #[tokio::test]
    async fn mismatched_branch_becomes_recovery_required() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        git(
            Path::new(&created.worktree_path),
            &["checkout", "-b", "unexpected"],
        );

        assert!(matches!(
            WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main.path()).await,
            Err(TransactionError::RecoveryRequired)
        ));
        assert_eq!(
            repository
                .v3()
                .get_task_worktree(&task_id)
                .await
                .expect("stored")
                .state,
            "recovery_required"
        );
    }

    #[tokio::test]
    async fn mismatched_base_becomes_recovery_required() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let created = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create worktree");
        git(
            Path::new(&created.worktree_path),
            &["checkout", "--orphan", "foreign"],
        );
        fs::write(
            Path::new(&created.worktree_path).join("foreign.txt"),
            "foreign\n",
        )
        .expect("foreign file");
        git(Path::new(&created.worktree_path), &["add", "foreign.txt"]);
        git(
            Path::new(&created.worktree_path),
            &["commit", "-m", "foreign"],
        );
        git(main.path(), &["branch", "-f", &created.branch, "foreign"]);
        git(
            Path::new(&created.worktree_path),
            &["checkout", &created.branch],
        );

        assert!(matches!(
            WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main.path()).await,
            Err(TransactionError::RecoveryRequired)
        ));
        assert_eq!(
            repository
                .v3()
                .get_task_worktree(&task_id)
                .await
                .expect("stored")
                .state,
            "recovery_required"
        );
    }

    #[tokio::test]
    async fn leaves_an_unrelated_worktree_untouched() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let unrelated = root.path().join("unrelated");
        git(
            main.path(),
            &[
                "worktree",
                "add",
                "-b",
                "unrelated",
                unrelated.to_str().expect("utf8 path"),
            ],
        );
        let unrelated_head = git(&unrelated, &["rev-parse", "HEAD"]);
        let unrelated_branch = git(&unrelated, &["branch", "--show-current"]);

        WorktreeTransaction::create(repository, task_id, main.path(), root.path())
            .await
            .expect("create managed worktree");

        assert_eq!(git(&unrelated, &["rev-parse", "HEAD"]), unrelated_head);
        assert_eq!(
            git(&unrelated, &["branch", "--show-current"]),
            unrelated_branch
        );
    }

    async fn prepare(repository: RunRepository, task_id: TaskId, main: &Path) -> MergePreparation {
        let branch = git(main, &["branch", "--show-current"]);
        WorktreeTransaction::prepare_merge(repository, task_id, main, branch.trim())
            .await
            .expect("prepare merge")
    }

    #[tokio::test]
    async fn prepares_a_clean_diff() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let preparation = prepare(repository, task_id, main.path()).await;
        assert!(preparation.diff.files.is_empty());
        assert!(preparation.persisted.merge_ready);
    }

    #[tokio::test]
    async fn reports_modified_staged_and_unstaged_files() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::write(
            Path::new(&worktree.worktree_path).join("README.md"),
            "staged\n",
        )
        .expect("write");
        git(Path::new(&worktree.worktree_path), &["add", "README.md"]);
        fs::write(
            Path::new(&worktree.worktree_path).join("README.md"),
            "unstaged\n",
        )
        .expect("write");
        let preparation = prepare(repository, task_id, main.path()).await;
        assert_eq!(
            preparation.diff.files,
            vec![ChangedFileState {
                path: "README.md".into(),
                index_status: Some("M".into()),
                worktree_status: Some("M".into()),
                untracked: false
            }]
        );
    }

    #[tokio::test]
    async fn reports_added_deleted_and_untracked_files() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::remove_file(Path::new(&worktree.worktree_path).join("README.md")).expect("delete");
        git(Path::new(&worktree.worktree_path), &["add", "-u"]);
        fs::write(
            Path::new(&worktree.worktree_path).join("added.txt"),
            "added\n",
        )
        .expect("add");
        git(Path::new(&worktree.worktree_path), &["add", "added.txt"]);
        fs::write(
            Path::new(&worktree.worktree_path).join("untracked.txt"),
            "untracked\n",
        )
        .expect("untracked");
        let preparation = prepare(repository, task_id, main.path()).await;
        assert_eq!(
            preparation
                .diff
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["README.md", "added.txt", "untracked.txt"]
        );
        assert!(preparation
            .diff
            .files
            .iter()
            .any(|file| file.untracked && file.path == "untracked.txt"));
    }

    #[tokio::test]
    async fn review_evidence_hashes_files_inside_an_untracked_directory() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let source = Path::new(&worktree.worktree_path).join("src");
        fs::create_dir_all(&source).expect("source directory");
        fs::write(source.join("status.js"), "export const status = true;\n").expect("source file");
        fs::write(source.join("status.test.js"), "test('status', () => {});\n").expect("test file");

        let evidence = WorktreeTransaction::diff_text(repository, task_id, main.path())
            .await
            .expect("review evidence");

        assert!(evidence.contains("# untracked file: src/status.js"));
        assert!(evidence.contains("# untracked file: src/status.test.js"));
        assert!(!evidence.contains("# untracked file: src/\n"));
    }

    #[tokio::test]
    async fn records_when_target_branch_has_advanced() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::write(main.path().join("target.txt"), "target\n").expect("target change");
        git(main.path(), &["add", "target.txt"]);
        git(main.path(), &["commit", "-m", "target advanced"]);
        let preparation = prepare(repository.clone(), task_id.clone(), main.path()).await;
        assert!(preparation.persisted.target_advanced);
        assert_eq!(
            repository
                .v3()
                .get_task_worktree_merge_preparation(&task_id)
                .await
                .expect("persisted preparation"),
            preparation.persisted
        );
    }

    #[tokio::test]
    async fn records_a_clean_merge_candidate() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::write(
            Path::new(&worktree.worktree_path).join("task.txt"),
            "task\n",
        )
        .expect("task change");
        git(Path::new(&worktree.worktree_path), &["add", "task.txt"]);
        git(
            Path::new(&worktree.worktree_path),
            &["commit", "-m", "task"],
        );
        fs::write(main.path().join("target.txt"), "target\n").expect("target change");
        git(main.path(), &["add", "target.txt"]);
        git(main.path(), &["commit", "-m", "target"]);
        assert!(
            prepare(repository, task_id, main.path())
                .await
                .persisted
                .merge_ready
        );
    }

    #[tokio::test]
    async fn records_a_merge_conflict_candidate() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::write(
            Path::new(&worktree.worktree_path).join("README.md"),
            "task\n",
        )
        .expect("task change");
        git(Path::new(&worktree.worktree_path), &["add", "README.md"]);
        git(
            Path::new(&worktree.worktree_path),
            &["commit", "-m", "task"],
        );
        fs::write(main.path().join("README.md"), "target\n").expect("target change");
        git(main.path(), &["add", "README.md"]);
        git(main.path(), &["commit", "-m", "target"]);
        let preparation = prepare(repository, task_id, main.path()).await;
        assert!(!preparation.persisted.merge_ready);
        assert!(!preparation.persisted.conflicts_json.is_empty());
    }

    #[tokio::test]
    async fn refuses_preparation_for_recovery_required_worktree() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        fs::rename(&worktree.worktree_path, root.path().join("missing")).expect("move");
        let branch = git(main.path(), &["branch", "--show-current"]);
        assert!(matches!(
            WorktreeTransaction::prepare_merge(repository, task_id, main.path(), branch.trim())
                .await,
            Err(TransactionError::RecoveryRequired)
        ));
    }

    #[tokio::test]
    async fn preparation_preserves_primary_and_unrelated_worktrees() {
        let (main, _database, repository, task_id, _base) = fixture().await;
        let root = tempfile::tempdir().expect("worktree root");
        let unrelated = root.path().join("unrelated");
        git(
            main.path(),
            &[
                "worktree",
                "add",
                "-b",
                "unrelated-b",
                unrelated.to_str().expect("path"),
            ],
        );
        let main_head = git(main.path(), &["rev-parse", "HEAD"]);
        let unrelated_head = git(&unrelated, &["rev-parse", "HEAD"]);
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let _ = prepare(repository, task_id, main.path()).await;
        assert_eq!(git(main.path(), &["rev-parse", "HEAD"]), main_head);
        assert_eq!(git(&unrelated, &["rev-parse", "HEAD"]), unrelated_head);
    }

    async fn approval(repository: &RunRepository, task_id: TaskId) -> (ApprovalId, u64) {
        let pending = repository
            .v3()
            .create_approval(
                sentinel_core::v3::CreateApproval {
                    task_id,
                    session_id: None,
                    action_kind: "worktree_discard".into(),
                    summary: "Discard task worktree".into(),
                },
                10,
            )
            .await
            .expect("approval");
        let approved = repository
            .v3()
            .transition_approval(&pending, ApprovalLifecycle::Approved, 11)
            .await
            .expect("approve");
        (approved.id, approved.version)
    }

    #[tokio::test]
    async fn approved_clean_discard_removes_only_owned_worktree_and_branch() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        let result = WorktreeTransaction::discard(
            repository.clone(),
            task_id.clone(),
            main.path(),
            approval_id,
            version,
            &base,
        )
        .await
        .expect("discard");
        assert_eq!(result.outcome.state, "discarded");
        assert!(!Path::new(&worktree.worktree_path).exists());
        assert!(
            !git(main.path(), &["branch", "--list", &worktree.branch]).contains(&worktree.branch)
        );
        assert_eq!(
            repository
                .v3()
                .get_task_worktree_cleanup_outcome(&task_id)
                .await
                .expect("outcome"),
            result.outcome
        );
    }

    #[tokio::test]
    async fn discard_requires_approval() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        assert!(matches!(
            WorktreeTransaction::discard(
                repository,
                task_id,
                main.path(),
                ApprovalId::new(),
                0,
                &base
            )
            .await,
            Err(TransactionError::ApprovalRequired)
        ));
    }

    #[tokio::test]
    async fn discard_refuses_stale_approval_or_changed_head() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        assert!(matches!(
            WorktreeTransaction::discard(
                repository.clone(),
                task_id.clone(),
                main.path(),
                approval_id.clone(),
                version + 1,
                &base
            )
            .await,
            Err(TransactionError::ApprovalRequired)
        ));
        fs::write(
            Path::new(&worktree.worktree_path).join("task.txt"),
            "task\n",
        )
        .expect("write");
        git(Path::new(&worktree.worktree_path), &["add", "task.txt"]);
        git(
            Path::new(&worktree.worktree_path),
            &["commit", "-m", "task"],
        );
        assert!(matches!(
            WorktreeTransaction::discard(
                repository.clone(),
                task_id.clone(),
                main.path(),
                approval_id,
                version,
                &base
            )
            .await,
            Err(TransactionError::PreflightRefused)
        ));
        assert_eq!(
            repository
                .v3()
                .get_task_worktree_cleanup_outcome(&task_id)
                .await
                .expect("outcome")
                .state,
            "retained"
        );
    }

    #[tokio::test]
    async fn discard_refuses_dirty_or_identity_mismatched_worktrees() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        fs::write(
            Path::new(&worktree.worktree_path).join("dirty.txt"),
            "dirty\n",
        )
        .expect("dirty");
        assert!(matches!(
            WorktreeTransaction::discard(
                repository.clone(),
                task_id.clone(),
                main.path(),
                approval_id,
                version,
                &base
            )
            .await,
            Err(TransactionError::PreflightRefused)
        ));
        assert!(Path::new(&worktree.worktree_path).exists());
    }

    #[tokio::test]
    async fn cleanup_failure_is_retained_and_repeat_is_idempotent() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        let worktree = WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        fs::rename(&worktree.worktree_path, root.path().join("moved")).expect("move");
        assert!(WorktreeTransaction::discard(
            repository.clone(),
            task_id.clone(),
            main.path(),
            approval_id,
            version,
            &base
        )
        .await
        .is_err());
        assert_eq!(
            repository
                .v3()
                .get_task_worktree_cleanup_outcome(&task_id)
                .await
                .expect("outcome")
                .state,
            "retained"
        );
        // A successfully discarded task reports the recorded outcome without a second Git mutation.
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        let first = WorktreeTransaction::discard(
            repository.clone(),
            task_id.clone(),
            main.path(),
            approval_id.clone(),
            version,
            &base,
        )
        .await
        .expect("first");
        let second = WorktreeTransaction::discard(
            repository,
            task_id,
            main.path(),
            approval_id,
            version,
            &base,
        )
        .await
        .expect("repeat");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn discard_preserves_primary_and_unrelated_worktrees() {
        let (main, _database, repository, task_id, base) = fixture().await;
        let root = tempfile::tempdir().expect("root");
        let unrelated = root.path().join("unrelated");
        git(
            main.path(),
            &[
                "worktree",
                "add",
                "-b",
                "unrelated-cleanup",
                unrelated.to_str().expect("path"),
            ],
        );
        let main_head = git(main.path(), &["rev-parse", "HEAD"]);
        let unrelated_head = git(&unrelated, &["rev-parse", "HEAD"]);
        WorktreeTransaction::create(
            repository.clone(),
            task_id.clone(),
            main.path(),
            root.path(),
        )
        .await
        .expect("create");
        let (approval_id, version) = approval(&repository, task_id.clone()).await;
        WorktreeTransaction::discard(
            repository,
            task_id,
            main.path(),
            approval_id,
            version,
            &base,
        )
        .await
        .expect("discard");
        assert_eq!(git(main.path(), &["rev-parse", "HEAD"]), main_head);
        assert_eq!(git(&unrelated, &["rev-parse", "HEAD"]), unrelated_head);
    }
}
