use sentinel_git::{inspect_repository, GitError, RepositoryState, TrustedGitExecutableResolver};
use std::{path::Path, process::Command};
use tempfile::TempDir;

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(directory)
        .output()
        .expect("git fixture command starts");
    assert!(output.status.success(), "git fixture command fails");
}

fn repository() -> TempDir {
    let temporary = tempfile::tempdir().expect("temporary directory");
    git(temporary.path(), &["init"]);
    git(
        temporary.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(temporary.path(), &["config", "user.name", "Tests"]);
    std::fs::write(temporary.path().join("README.md"), "fixture").expect("fixture file");
    git(temporary.path(), &["add", "README.md"]);
    git(temporary.path(), &["commit", "-m", "fixture"]);
    temporary
}

#[tokio::test]
async fn nested_directories_resolve_to_one_primary_repository() {
    let temporary = repository();
    let nested = temporary.path().join("nested").join("deep");
    std::fs::create_dir_all(&nested).expect("nested directory");

    let root = inspect_repository(temporary.path())
        .await
        .expect("root inspection");
    let child = inspect_repository(&nested)
        .await
        .expect("nested inspection");

    assert_eq!(root.repository_root, child.repository_root);
    assert_eq!(root.identity, child.identity);
    assert!(root.is_primary);
    assert_eq!(root.state, RepositoryState::Valid);
    assert!(root.branch.is_some());
    assert!(root.head.is_some());
}

#[tokio::test]
async fn rejects_missing_files_non_repositories_and_bare_repositories() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    assert!(matches!(
        inspect_repository(&temporary.path().join("missing")).await,
        Err(GitError::PathNotFound)
    ));
    let file = temporary.path().join("file");
    std::fs::write(&file, "not a directory").expect("file");
    assert!(matches!(
        inspect_repository(&file).await,
        Err(GitError::NotDirectory)
    ));
    assert!(matches!(
        inspect_repository(temporary.path()).await,
        Err(GitError::NotRepository(_))
    ));

    let bare = tempfile::tempdir().expect("bare parent");
    let bare_path = bare.path().join("repo.git");
    let output = Command::new("/usr/bin/git")
        .args(["init", "--bare", bare_path.to_str().expect("utf8 path")])
        .output()
        .expect("bare init starts");
    assert!(output.status.success());
    assert!(matches!(
        inspect_repository(&bare_path).await,
        Err(GitError::BareRepository)
    ));
}

#[tokio::test]
async fn separate_git_directory_uses_the_real_primary_worktree() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let worktree = parent.path().join("working tree");
    let git_directory = parent.path().join("external metadata");
    let output = Command::new("/usr/bin/git")
        .args([
            "init",
            "--separate-git-dir",
            git_directory.to_str().expect("utf8 path"),
            worktree.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("separate git init starts");
    assert!(output.status.success());
    let inspection = inspect_repository(&worktree).await.expect("inspection");
    assert!(inspection.is_primary);
    assert_eq!(
        inspection.repository_root,
        worktree.canonicalize().expect("working tree canonical")
    );
    assert_eq!(
        inspection.primary_root,
        worktree.canonicalize().expect("primary canonical")
    );
    assert_eq!(
        inspection.common_dir,
        git_directory
            .canonicalize()
            .expect("git directory canonical")
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn repository_fingerprint_changes_when_a_repository_is_recreated_at_the_same_path() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let path = parent.path().join("replaceable repository");
    std::fs::create_dir_all(&path).expect("repository directory");
    git(&path, &["init"]);
    let original = inspect_repository(&path)
        .await
        .expect("original inspection");
    std::fs::remove_dir_all(&path).expect("remove fixture only");
    std::fs::create_dir_all(&path).expect("replacement directory");
    git(&path, &["init"]);
    let replacement = inspect_repository(&path)
        .await
        .expect("replacement inspection");
    assert_eq!(original.identity, replacement.identity);
    assert!(original
        .fingerprint
        .as_str()
        .starts_with("strong_v1:macos_object_v1:"));
    assert!(replacement
        .fingerprint
        .as_str()
        .starts_with("strong_v1:macos_object_v1:"));
    assert_ne!(original.fingerprint, replacement.fingerprint);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn strong_fingerprint_is_stable_through_normal_repository_changes() {
    let temporary = repository();
    let before = inspect_repository(temporary.path())
        .await
        .expect("initial inspection");
    std::fs::write(
        temporary.path().join("normal-change.txt"),
        "ordinary work\n",
    )
    .expect("fixture change");
    git(temporary.path(), &["add", "normal-change.txt"]);
    git(
        temporary.path(),
        &["commit", "-m", "normal repository change"],
    );
    let after = inspect_repository(temporary.path())
        .await
        .expect("post-change inspection");
    assert_eq!(before.fingerprint, after.fingerprint);
}

#[tokio::test]
async fn corrupt_head_is_not_accepted_as_an_unborn_repository() {
    let temporary = repository();
    std::fs::write(
        temporary.path().join(".git").join("HEAD"),
        "not a HEAD record\n",
    )
    .expect("corrupt fixture metadata");
    let result = inspect_repository(temporary.path()).await;
    assert!(
        matches!(result, Err(GitError::MetadataInvalid)),
        "{result:?}"
    );
}

#[cfg(unix)]
#[test]
fn trusted_resolver_does_not_consult_a_hostile_inherited_path() {
    use std::os::unix::fs::PermissionsExt;
    let resolver = TrustedGitExecutableResolver;
    let fixture = tempfile::tempdir().expect("hostile path fixture");
    let marker = fixture.path().join("hostile-git-was-run");
    let hostile = fixture.path().join("git");
    std::fs::write(
        &hostile,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .expect("hostile executable");
    let mut permissions = std::fs::metadata(&hostile).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hostile, permissions).expect("permissions");
    let original = std::env::var_os("PATH");
    std::env::set_var("PATH", fixture.path());
    let resolved = sentinel_git::GitExecutableResolver::resolve(&resolver);
    if let Some(path) = original {
        std::env::set_var("PATH", path);
    } else {
        std::env::remove_var("PATH");
    }
    assert!(
        resolved.is_ok(),
        "trusted candidate policy must not use PATH"
    );
    assert!(!marker.exists(), "hostile PATH executable must not run");
}

#[tokio::test]
async fn classifies_detached_unborn_and_linked_worktrees() {
    let temporary = repository();
    git(temporary.path(), &["checkout", "--detach"]);
    assert_eq!(
        inspect_repository(temporary.path())
            .await
            .expect("detached")
            .state,
        RepositoryState::Detached
    );

    let unborn = tempfile::tempdir().expect("unborn repo");
    git(unborn.path(), &["init"]);
    assert_eq!(
        inspect_repository(unborn.path())
            .await
            .expect("unborn")
            .state,
        RepositoryState::Unborn
    );

    let linked_parent = tempfile::tempdir().expect("linked parent");
    let linked = linked_parent.path().join("linked worktree");
    git(
        temporary.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().expect("utf8 path"),
        ],
    );
    let inspection = inspect_repository(&linked)
        .await
        .expect("linked inspection");
    assert_eq!(inspection.state, RepositoryState::LinkedWorktree);
    assert!(!inspection.is_primary);
    assert_eq!(
        inspection.primary_root,
        temporary
            .path()
            .canonicalize()
            .expect("primary canonical path")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn canonicalizes_symlinks_and_unicode_paths() {
    let parent = tempfile::tempdir().expect("temporary directory");
    let source = parent.path().join("repository with ünicode");
    std::fs::create_dir_all(&source).expect("source directory");
    git(&source, &["init"]);
    git(&source, &["config", "user.email", "tests@example.invalid"]);
    git(&source, &["config", "user.name", "Tests"]);
    std::fs::write(source.join("README.md"), "fixture").expect("fixture file");
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-m", "fixture"]);
    let alias = parent.path().join("repository alias");
    std::os::unix::fs::symlink(&source, &alias).expect("symlink");

    let through_alias = inspect_repository(&alias).await.expect("alias inspection");
    assert_eq!(
        through_alias.repository_root,
        source.canonicalize().expect("source canonical")
    );
}
