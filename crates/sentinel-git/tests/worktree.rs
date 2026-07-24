use sentinel_git::{
    changed_files, create_worktree, remove_clean_worktree, validate_repository, GitError,
};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn run(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success());
}
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init"]);
    run(dir.path(), &["config", "user.email", "spike@example.test"]);
    run(dir.path(), &["config", "user.name", "Spike"]);
    fs::write(dir.path().join("README.md"), "fixture\n").unwrap();
    run(dir.path(), &["add", "README.md"]);
    run(dir.path(), &["commit", "-m", "fixture"]);
    dir
}

#[test]
fn validates_and_creates_a_separate_worktree() {
    let repo = fixture();
    let root = tempfile::tempdir().unwrap();
    validate_repository(repo.path()).unwrap();
    let worktree = create_worktree(repo.path(), root.path()).unwrap();
    assert!(worktree.path.exists());
    assert!(worktree.branch.starts_with("agent-sentinel/spike-"));
    assert!(changed_files(&worktree.path).unwrap().is_empty());
    remove_clean_worktree(repo.path(), &worktree).unwrap();
    assert!(!worktree.path.exists());
}

#[test]
fn refuses_to_remove_a_dirty_worktree() {
    let repo = fixture();
    let root = tempfile::tempdir().unwrap();
    let worktree = create_worktree(repo.path(), root.path()).unwrap();
    fs::write(worktree.path.join("changed.txt"), "uncommitted\n").unwrap();
    assert!(matches!(
        remove_clean_worktree(repo.path(), &worktree),
        Err(GitError::DirtyWorktree(_))
    ));
}
