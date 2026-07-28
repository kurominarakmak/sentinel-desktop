use sentinel_git::{
    add_detached_worktree, inspect_repository, inspect_worktree_destination_no_follow,
    parse_worktree_porcelain_v1_z, remove_detached_worktree, resolve_exact_head, worktree_is_clean,
    worktree_metadata_lookup, GitError, RepositoryState, TrustedGitExecutableResolver,
    WorktreeDestinationState, WorktreeMetadataLookup,
};
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

#[tokio::test]
async fn detached_worktrees_are_exact_commit_isolated_and_cleanly_removable() {
    let primary = repository();
    let root = tempfile::tempdir().expect("trusted worktree root");
    let commit = resolve_exact_head(primary.path())
        .await
        .expect("exact head");
    std::fs::write(primary.path().join("primary-untracked.txt"), "primary only")
        .expect("primary dirty fixture");
    let first = root.path().join("project").join("first");
    let second = root.path().join("project").join("second");
    std::fs::create_dir_all(first.parent().expect("parent")).expect("first parent");
    add_detached_worktree(primary.path(), &first, &commit)
        .await
        .expect("first add");
    add_detached_worktree(primary.path(), &second, &commit)
        .await
        .expect("second add");
    let first_inspection = inspect_repository(&first).await.expect("first inspection");
    let second_inspection = inspect_repository(&second)
        .await
        .expect("second inspection");
    assert!(!first_inspection.is_primary && !second_inspection.is_primary);
    assert_eq!(first_inspection.head.as_deref(), Some(commit.as_str()));
    assert_eq!(second_inspection.head.as_deref(), Some(commit.as_str()));
    assert_eq!(
        worktree_metadata_lookup(primary.path(), &first)
            .await
            .expect("first metadata"),
        WorktreeMetadataLookup::Present
    );
    std::fs::write(first.join("only-first.txt"), "a").expect("first edit");
    std::fs::write(second.join("only-second.txt"), "b").expect("second edit");
    assert!(!second.join("only-first.txt").exists());
    assert!(!first.join("only-second.txt").exists());
    assert!(!primary.path().join("only-first.txt").exists());
    assert!(!primary.path().join("only-second.txt").exists());
    assert!(!first.join("primary-untracked.txt").exists());
    assert!(!second.join("primary-untracked.txt").exists());
    assert!(!worktree_is_clean(&first).await.expect("first status"));
    assert!(matches!(
        remove_detached_worktree(primary.path(), &first).await,
        Err(GitError::DirtyWorktree(_))
    ));
    std::fs::remove_file(first.join("only-first.txt")).expect("restore first");
    assert!(worktree_is_clean(&first).await.expect("clean first"));
    remove_detached_worktree(primary.path(), &first)
        .await
        .expect("remove first");
    std::fs::remove_file(second.join("only-second.txt")).expect("restore second");
    remove_detached_worktree(primary.path(), &second)
        .await
        .expect("remove second");
    assert!(!first.exists() && !second.exists());
    assert_eq!(
        worktree_metadata_lookup(primary.path(), &first)
            .await
            .expect("removed metadata"),
        WorktreeMetadataLookup::Absent
    );
    std::fs::remove_file(primary.path().join("primary-untracked.txt")).expect("restore primary");
}

#[test]
fn strict_worktree_porcelain_parser_accepts_complete_records_and_rejects_damage() {
    let oid = "0123456789abcdef0123456789abcdef01234567";
    let valid = format!("worktree /tmp/primary\0HEAD {oid}\0branch refs/heads/main\0\0worktree /tmp/linked space 空\0HEAD {oid}\0detached\0locked reason\0prunable stale\0\0");
    let records = parse_worktree_porcelain_v1_z(valid.as_bytes()).expect("valid records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].branch.as_deref(), Some("refs/heads/main"));
    assert!(records[1].detached);
    assert_eq!(records[1].locked.as_deref(), Some("reason"));
    assert_eq!(records[1].prunable.as_deref(), Some("stale"));
    let malformed = [
        Vec::new(),
        format!("worktree /tmp/a\0HEAD {oid}\0").into_bytes(),
        format!("HEAD {oid}\0\0").into_bytes(),
        format!("worktree /tmp/a\0worktree /tmp/b\0HEAD {oid}\0\0").into_bytes(),
        format!("worktree /tmp/a\0HEAD {oid}\0HEAD {oid}\0\0").into_bytes(),
        b"worktree /tmp/a\0HEAD nope\0\0".to_vec(),
        format!("worktree /tmp/a\0HEAD {oid}\0branch refs/heads/main\0detached\0\0").into_bytes(),
        "worktree /tmp/a\0unknown value\0\0".as_bytes().to_vec(),
        format!("worktree /tmp/a\0HEAD {oid}\0\0worktree /tmp/a\0HEAD {oid}\0\0").into_bytes(),
        b"worktree /tmp/\xff\0HEAD 0123456789abcdef0123456789abcdef01234567\0\0".to_vec(),
    ];
    for bytes in malformed {
        assert!(parse_worktree_porcelain_v1_z(&bytes).is_err(), "{bytes:?}");
    }
}

#[test]
fn strict_worktree_porcelain_paths_are_absolute_normalized_and_unambiguous() {
    let oid = "0123456789abcdef0123456789abcdef01234567";
    let valid = format!(
        "worktree /tmp/-leading-ü\nname\0HEAD {oid}\0detached\0\0worktree /tmp/space path\0HEAD {oid}\0branch refs/heads/main\0\0"
    );
    let records = parse_worktree_porcelain_v1_z(valid.as_bytes()).expect("valid absolute paths");
    assert_eq!(records.len(), 2);

    let malformed = [
        format!("worktree relative/id\0HEAD {oid}\0\0"),
        format!("worktree ./worktree\0HEAD {oid}\0\0"),
        format!("worktree /managed/project/./id\0HEAD {oid}\0\0"),
        format!("worktree /managed/project/../other\0HEAD {oid}\0\0"),
        format!("worktree /managed/project/id\0HEAD {oid}\0\0worktree /managed/project/./id\0HEAD {oid}\0\0"),
        format!("worktree C:relative\\id\0HEAD {oid}\0\0"),
        format!("worktree \\\\?\\C:\\device\\id\0HEAD {oid}\0\0"),
    ];
    for fixture in malformed {
        assert!(
            parse_worktree_porcelain_v1_z(fixture.as_bytes()).is_err(),
            "{fixture:?}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_destination_never_becomes_metadata_absent() {
    let primary = repository();
    let dangling = primary.path().join("dangling managed leaf");
    std::os::unix::fs::symlink(primary.path().join("missing target"), &dangling)
        .expect("dangling symlink");
    assert_eq!(
        inspect_worktree_destination_no_follow(&dangling),
        WorktreeDestinationState::UnsafeLinkOrReparse
    );
    assert!(matches!(
        worktree_metadata_lookup(primary.path(), &dangling).await,
        Err(GitError::MetadataInvalid)
    ));
}

#[cfg(unix)]
#[test]
fn metadata_access_failure_is_not_missing() {
    use std::os::unix::ffi::OsStringExt;

    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0, b'x']));
    assert_eq!(
        inspect_worktree_destination_no_follow(&path),
        WorktreeDestinationState::MetadataUnavailable
    );
}
