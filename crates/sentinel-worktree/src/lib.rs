use sentinel_core::{
    v3::{CreateTaskWorktree, TaskId, TaskWorktree},
    CoreError, RunRepository,
};
use sentinel_git::{inspect_repository, resolve_exact_head, RepositoryState};
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
    #[error("git failure")]
    Git,
    #[error("storage failure")]
    Storage,
}
pub struct WorktreeTransaction;
impl WorktreeTransaction {
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
    resolve_exact_head(path)
        .await
        .is_ok_and(|head| head == worktree.base_commit)
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
        fs::write(
            Path::new(&created.worktree_path).join("changed.txt"),
            "changed\n",
        )
        .expect("change worktree");
        git(Path::new(&created.worktree_path), &["add", "changed.txt"]);
        git(
            Path::new(&created.worktree_path),
            &["commit", "-m", "changed"],
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
}
