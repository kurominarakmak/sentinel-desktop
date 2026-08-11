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
            return Err(TransactionError::Git);
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
    use sentinel_core::v3::CreateTask;
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
        let task = repository
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
            .expect("create task");
        (main, database, repository, task.id, base)
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
}
