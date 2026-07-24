use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git command failed: {0}")]
    Command(String),
    #[error("path is not a Git repository: {0}")]
    NotRepository(PathBuf),
    #[error("worktree has uncommitted changes: {0}")]
    DirtyWorktree(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

pub fn validate_repository(repository: &Path) -> Result<(), GitError> {
    let result = git(repository, ["rev-parse", "--is-inside-work-tree"])?;
    if result.trim() == "true" {
        Ok(())
    } else {
        Err(GitError::NotRepository(repository.into()))
    }
}

pub fn current_commit(repository: &Path) -> Result<String, GitError> {
    validate_repository(repository)?;
    Ok(git(repository, ["rev-parse", "HEAD"])?.trim().to_owned())
}

pub fn create_worktree(repository: &Path, root: &Path) -> Result<Worktree, GitError> {
    let base_commit = current_commit(repository)?;
    fs::create_dir_all(root)?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let branch = format!("agent-sentinel/spike-{suffix}");
    let path = root.join(format!("spike-{suffix}"));
    git(
        repository,
        [
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_str().unwrap(),
            &base_commit,
        ],
    )?;
    Ok(Worktree {
        path,
        branch,
        base_commit,
    })
}

pub fn changed_files(worktree: &Path) -> Result<Vec<String>, GitError> {
    Ok(git(worktree, ["status", "--porcelain"])?
        .lines()
        .filter_map(|line| line.get(3..).map(str::to_owned))
        .collect())
}

pub fn remove_clean_worktree(repository: &Path, worktree: &Worktree) -> Result<(), GitError> {
    if !changed_files(&worktree.path)?.is_empty() {
        return Err(GitError::DirtyWorktree(worktree.path.clone()));
    }
    git(
        repository,
        ["worktree", "remove", worktree.path.to_str().unwrap()],
    )?;
    Ok(())
}

fn git<const N: usize>(directory: &Path, args: [&str; N]) -> Result<String, GitError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(GitError::Command(
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}
