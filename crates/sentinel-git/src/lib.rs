use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command as TokioCommand,
    time,
};

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git command failed: {0}")]
    Command(String),
    #[error("path is not a Git repository: {0}")]
    NotRepository(PathBuf),
    #[error("git is not available")]
    GitNotAvailable,
    #[error("git discovery failed")]
    GitDiscoveryFailed,
    #[error("git validation timed out")]
    TimedOut,
    #[error("git validation stdout exceeded its limit")]
    StdoutTooLarge,
    #[error("git validation stderr exceeded its limit")]
    StderrTooLarge,
    #[error("git repository metadata is invalid")]
    MetadataInvalid,
    #[error("repository fingerprint is unavailable")]
    FingerprintUnavailable,
    #[error("path does not exist")]
    PathNotFound,
    #[error("path is not a directory")]
    NotDirectory,
    #[error("bare repositories are unsupported")]
    BareRepository,
    #[error("worktree has uncommitted changes: {0}")]
    DirtyWorktree(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryState {
    Valid,
    Detached,
    Unborn,
    LinkedWorktree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryInspection {
    pub repository_root: PathBuf,
    pub primary_root: PathBuf,
    pub common_dir: PathBuf,
    pub identity: String,
    pub fingerprint: RepositoryFingerprint,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub state: RepositoryState,
    pub is_primary: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryFingerprint(String);

impl RepositoryFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_stored(value: String) -> Result<Self, GitError> {
        if value.is_empty() {
            Err(GitError::FingerprintUnavailable)
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedGitExecutable(PathBuf);

impl ResolvedGitExecutable {
    pub fn from_path(path: PathBuf) -> Result<Self, GitError> {
        let canonical = path
            .canonicalize()
            .map_err(|_| GitError::GitDiscoveryFailed)?;
        let metadata = fs::metadata(&canonical).map_err(|_| GitError::GitDiscoveryFailed)?;
        if !metadata.is_file() || !executable_metadata(&metadata) {
            return Err(GitError::GitDiscoveryFailed);
        }
        Ok(Self(canonical))
    }
}

pub trait GitExecutableResolver: Send + Sync {
    fn resolve(&self) -> Result<ResolvedGitExecutable, GitError>;
}

pub struct TrustedGitExecutableResolver;

impl GitExecutableResolver for TrustedGitExecutableResolver {
    fn resolve(&self) -> Result<ResolvedGitExecutable, GitError> {
        for candidate in trusted_git_candidates()? {
            if let Ok(executable) = ResolvedGitExecutable::from_path(candidate) {
                return Ok(executable);
            }
        }
        Err(GitError::GitNotAvailable)
    }
}

#[cfg(unix)]
fn executable_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable_metadata(_: &fs::Metadata) -> bool {
    true
}

fn trusted_git_candidates() -> Result<Vec<PathBuf>, GitError> {
    #[cfg(unix)]
    {
        Ok(vec![
            PathBuf::from("/usr/bin/git"),
            PathBuf::from("/opt/homebrew/bin/git"),
            PathBuf::from("/usr/local/bin/git"),
        ])
    }
    #[cfg(windows)]
    {
        windows_machine_git_candidates()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
trait WindowsInstallLocationProvider {
    fn machine_program_files(&self) -> Result<Vec<PathBuf>, GitError>;
}

#[cfg(windows)]
struct WindowsKnownFolderProvider;

#[cfg(windows)]
impl WindowsInstallLocationProvider for WindowsKnownFolderProvider {
    fn machine_program_files(&self) -> Result<Vec<PathBuf>, GitError> {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, SHGetKnownFolderPath},
        };

        fn known_folder(folder: &windows_sys::core::GUID) -> Result<PathBuf, GitError> {
            let mut value = std::ptr::null_mut();
            // These machine-known folders come from the Windows shell API, not
            // inherited environment variables supplied by a launcher.
            let result = unsafe { SHGetKnownFolderPath(folder, 0, 0, &mut value) };
            if result < 0 || value.is_null() {
                return Err(GitError::GitDiscoveryFailed);
            }
            let mut length = 0;
            unsafe {
                while *value.add(length) != 0 {
                    length += 1;
                }
                let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
                    value, length,
                )));
                CoTaskMemFree(value.cast());
                Ok(path)
            }
        }

        let mut roots = vec![known_folder(&FOLDERID_ProgramFiles)?];
        if let Ok(x86) = known_folder(&FOLDERID_ProgramFilesX86) {
            if !roots.contains(&x86) {
                roots.push(x86);
            }
        }
        Ok(roots)
    }
}

#[cfg(windows)]
fn windows_machine_git_candidates() -> Result<Vec<PathBuf>, GitError> {
    windows_machine_git_candidates_with(&WindowsKnownFolderProvider)
}

#[cfg(windows)]
fn windows_machine_git_candidates_with(
    provider: &dyn WindowsInstallLocationProvider,
) -> Result<Vec<PathBuf>, GitError> {
    Ok(provider
        .machine_program_files()?
        .into_iter()
        .flat_map(|root| {
            [
                root.join("Git\\cmd\\git.exe"),
                root.join("Git\\bin\\git.exe"),
            ]
        })
        .collect())
}

const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const MAX_GIT_OUTPUT: usize = 8_192;

/// Inspects a local worktree through fixed, read-only Git commands only.
pub async fn inspect_repository(path: &Path) -> Result<RepositoryInspection, GitError> {
    inspect_repository_with_resolver(path, &TrustedGitExecutableResolver).await
}

pub async fn inspect_repository_with_resolver(
    path: &Path,
    resolver: &dyn GitExecutableResolver,
) -> Result<RepositoryInspection, GitError> {
    if !path.exists() {
        return Err(GitError::PathNotFound);
    }
    if !path.is_dir() {
        return Err(GitError::NotDirectory);
    }
    // Persist the resolved directory. Symlink inputs are deliberately treated as
    // their target so a repository cannot be registered twice through aliases.
    let input = path.canonicalize().map_err(GitError::Io)?;
    let executable = resolver.resolve()?;
    let git_dir_result = run_git(&executable, &input, ["rev-parse", "--git-dir"]).await?;
    let git_dir_output = if git_dir_result.status.success() {
        decode_output(git_dir_result.stdout)?
    } else if input.join(".git").exists() {
        return Err(GitError::MetadataInvalid);
    } else {
        return Err(GitError::NotRepository(input.clone()));
    };
    let bare = required_output(
        run_git(&executable, &input, ["rev-parse", "--is-bare-repository"]).await?,
        GitError::MetadataInvalid,
    )?;
    if bare == "true" {
        return Err(GitError::BareRepository);
    }
    if bare != "false" {
        return Err(GitError::MetadataInvalid);
    }
    let in_work_tree = required_output(
        run_git(&executable, &input, ["rev-parse", "--is-inside-work-tree"]).await?,
        GitError::NotRepository(input.clone()),
    )?;
    if in_work_tree != "true" {
        return Err(GitError::NotRepository(input));
    }
    let repository_root = canonical_git_path(
        &input,
        &required_output(
            run_git(&executable, &input, ["rev-parse", "--show-toplevel"]).await?,
            GitError::MetadataInvalid,
        )?,
    )?;
    let common_dir = canonical_git_path(
        &input,
        &required_output(
            run_git(&executable, &input, ["rev-parse", "--git-common-dir"]).await?,
            GitError::MetadataInvalid,
        )?,
    )?;
    let git_dir = canonical_git_path(&input, &git_dir_output)?;
    let is_primary = git_dir == common_dir;
    let primary_root =
        primary_worktree_root(&executable, &input, &repository_root, is_primary).await?;
    let branch_result = run_git(
        &executable,
        &repository_root,
        ["symbolic-ref", "-q", "--short", "HEAD"],
    )
    .await?;
    let branch = optional_symbolic_head(branch_result)?;
    let head_result = run_git(
        &executable,
        &repository_root,
        ["rev-parse", "--verify", "--quiet", "HEAD"],
    )
    .await?;
    let head = optional_head(head_result)?;
    let state = if !is_primary {
        RepositoryState::LinkedWorktree
    } else if head.is_none() {
        RepositoryState::Unborn
    } else if branch.is_none() {
        RepositoryState::Detached
    } else {
        RepositoryState::Valid
    };
    if branch.is_none() && head.is_none() {
        return Err(GitError::MetadataInvalid);
    }
    Ok(RepositoryInspection {
        identity: common_dir.to_string_lossy().into_owned(),
        fingerprint: repository_fingerprint(&common_dir)?,
        repository_root,
        primary_root,
        common_dir,
        branch,
        head,
        state,
        is_primary,
    })
}

struct GitCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    _stderr: Vec<u8>,
}

async fn run_git<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
) -> Result<GitCommandOutput, GitError> {
    run_git_with_timeout(executable, directory, args, GIT_TIMEOUT).await
}

async fn run_git_with_timeout<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
    timeout: std::time::Duration,
) -> Result<GitCommandOutput, GitError> {
    let mut command = TokioCommand::new(&executable.0);
    command
        .args(args)
        .current_dir(directory)
        .env_clear()
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| GitError::GitDiscoveryFailed)?;
    let stdout = child.stdout.take().ok_or(GitError::MetadataInvalid)?;
    let stderr = child.stderr.take().ok_or(GitError::MetadataInvalid)?;
    let collected = time::timeout(timeout, async {
        let streams = tokio::try_join!(
            read_capped(stdout, GitError::StdoutTooLarge),
            read_capped(stderr, GitError::StderrTooLarge)
        );
        let status = child.wait().await.map_err(|_| GitError::MetadataInvalid)?;
        streams.map(|(stdout, stderr)| GitCommandOutput {
            status,
            stdout,
            _stderr: stderr,
        })
    })
    .await;
    match collected {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(GitError::TimedOut)
        }
    }
}

async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    overflow: GitError,
) -> Result<Vec<u8>, GitError> {
    let mut bytes = Vec::with_capacity(MAX_GIT_OUTPUT.min(1024));
    let mut buffer = [0_u8; 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| GitError::MetadataInvalid)?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > MAX_GIT_OUTPUT {
            return Err(overflow);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn required_output(output: GitCommandOutput, error: GitError) -> Result<String, GitError> {
    if !output.status.success() {
        return Err(error);
    }
    decode_output(output.stdout)
}

fn optional_symbolic_head(output: GitCommandOutput) -> Result<Option<String>, GitError> {
    match output.status.code() {
        Some(0) => Ok(Some(nonempty_output(output.stdout)?)),
        Some(1) => Ok(None),
        _ => Err(GitError::MetadataInvalid),
    }
}

fn optional_head(output: GitCommandOutput) -> Result<Option<String>, GitError> {
    match output.status.code() {
        Some(0) => Ok(Some(nonempty_output(output.stdout)?)),
        Some(1) => Ok(None),
        _ => Err(GitError::MetadataInvalid),
    }
}

fn nonempty_output(output: Vec<u8>) -> Result<String, GitError> {
    let value = decode_output(output)?;
    if value.is_empty() {
        Err(GitError::MetadataInvalid)
    } else {
        Ok(value)
    }
}

fn decode_output(output: Vec<u8>) -> Result<String, GitError> {
    let value = String::from_utf8(output).map_err(|_| GitError::MetadataInvalid)?;
    let value = value.trim().to_owned();
    if value.contains('\0') {
        return Err(GitError::MetadataInvalid);
    }
    Ok(value)
}

async fn primary_worktree_root(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    repository_root: &Path,
    is_primary: bool,
) -> Result<PathBuf, GitError> {
    if is_primary {
        return Ok(repository_root.to_path_buf());
    }
    let output = run_git(
        executable,
        directory,
        ["worktree", "list", "--porcelain", "-z"],
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::MetadataInvalid);
    }
    let first = output
        .stdout
        .split(|byte| *byte == 0)
        .find_map(|record| record.strip_prefix(b"worktree "))
        .ok_or(GitError::MetadataInvalid)?;
    let path = std::str::from_utf8(first).map_err(|_| GitError::MetadataInvalid)?;
    PathBuf::from(path)
        .canonicalize()
        .map_err(|_| GitError::MetadataInvalid)
}

#[cfg(any(target_os = "macos", windows))]
fn fingerprint_digest(scheme: &str, values: &[u128]) -> RepositoryFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(b"agent-sentinel-repository-fingerprint-v1\0");
    hasher.update(scheme.as_bytes());
    for value in values {
        hasher.update(value.to_be_bytes());
    }
    let digest = hasher.finalize();
    RepositoryFingerprint(format!("strong_v1:{scheme}:{digest:x}"))
}

#[cfg(target_os = "macos")]
fn repository_fingerprint(common_dir: &Path) -> Result<RepositoryFingerprint, GitError> {
    use std::os::{
        darwin::fs::MetadataExt as DarwinMetadataExt, unix::fs::MetadataExt as UnixMetadataExt,
    };
    let metadata = fs::metadata(common_dir).map_err(|_| GitError::FingerprintUnavailable)?;
    let birth_seconds = metadata.st_birthtime();
    let birth_nanoseconds = metadata.st_birthtime_nsec();
    if birth_seconds <= 0 || birth_nanoseconds < 0 {
        return Err(GitError::FingerprintUnavailable);
    }
    Ok(fingerprint_digest(
        "macos_object_v1",
        &[
            u128::from(metadata.dev()),
            u128::from(metadata.ino()),
            birth_seconds as u128,
            birth_nanoseconds as u128,
        ],
    ))
}

#[cfg(windows)]
fn windows_wide_path(path: &Path) -> Result<Vec<u16>, GitError> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(GitError::FingerprintUnavailable);
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(windows)]
fn open_windows_directory(path: &Path) -> Result<std::os::windows::io::OwnedHandle, GitError> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };

    if !fs::metadata(path)
        .map_err(|_| GitError::FingerprintUnavailable)?
        .is_dir()
    {
        return Err(GitError::FingerprintUnavailable);
    }
    let wide = windows_wide_path(path)?;
    // `FILE_FLAG_BACKUP_SEMANTICS` is required to obtain a directory handle.
    // The handle has metadata-only access and permits normal readers, writers,
    // and deletes while Git continues operating on the repository.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(GitError::FingerprintUnavailable);
    }
    // The successful CreateFileW handle is transferred exactly once to RAII.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

#[cfg(windows)]
fn windows_directory_information(
    path: &Path,
) -> Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION, GitError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let directory = open_windows_directory(path)?;
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(directory.as_raw_handle() as isize, &mut information) }
        == 0
    {
        return Err(GitError::FingerprintUnavailable);
    }
    Ok(information)
}

#[cfg(windows)]
fn repository_fingerprint(common_dir: &Path) -> Result<RepositoryFingerprint, GitError> {
    let information = windows_directory_information(common_dir)?;
    let creation_time = (u128::from(information.ftCreationTime.dwHighDateTime) << 32)
        | u128::from(information.ftCreationTime.dwLowDateTime);
    let file_index =
        (u128::from(information.nFileIndexHigh) << 32) | u128::from(information.nFileIndexLow);
    if creation_time == 0 || file_index == 0 {
        return Err(GitError::FingerprintUnavailable);
    }
    Ok(fingerprint_digest(
        "windows_object_v1",
        &[
            u128::from(information.dwVolumeSerialNumber),
            file_index,
            creation_time,
        ],
    ))
}

// Linux's portable std metadata exposes only device/inode. The Phase 3A
// registry deliberately fails closed until a replacement-resistant statx or
// file-handle generation provider is available.
#[cfg(not(any(target_os = "macos", windows)))]
fn repository_fingerprint(_: &Path) -> Result<RepositoryFingerprint, GitError> {
    Err(GitError::FingerprintUnavailable)
}
fn canonical_git_path(base: &Path, value: &str) -> Result<PathBuf, GitError> {
    let path = PathBuf::from(value.trim());
    let candidate = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    candidate.canonicalize().map_err(GitError::Io)
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

#[cfg(all(test, unix))]
mod inspection_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture_executable(script: &str) -> (tempfile::TempDir, ResolvedGitExecutable) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = directory.path().join("trusted-git-fixture");
        fs::write(&executable, script).expect("fixture executable");
        let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("permissions");
        let resolved = ResolvedGitExecutable::from_path(executable).expect("resolved fixture");
        (directory, resolved)
    }

    #[tokio::test]
    async fn runner_uses_null_stdin_and_a_clean_environment() {
        let overrides = [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ASKPASS",
            "PAGER",
        ];
        let saved: Vec<_> = overrides
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in overrides {
            std::env::set_var(key, "hostile-override");
        }
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nif read ignored; then exit 9; fi\n[ -z \"${GIT_DIR+x}\" ] || exit 8\n[ -z \"${GIT_WORK_TREE+x}\" ] || exit 8\n[ -z \"${GIT_INDEX_FILE+x}\" ] || exit 8\n[ -z \"${GIT_OBJECT_DIRECTORY+x}\" ] || exit 8\n[ -z \"${GIT_ASKPASS+x}\" ] || exit 8\n[ \"$PAGER\" = cat ] || exit 7\nprintf false\n",
        );
        let output = run_git(&executable, directory.path(), ["ignored"])
            .await
            .expect("null stdin and cleaned Git environment");
        for (key, value) in saved {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        assert_eq!(decode_output(output.stdout).expect("output"), "false");
    }

    #[tokio::test]
    async fn runner_caps_stdout_and_stderr_while_streaming() {
        let (stdout_directory, stdout_executable) =
            fixture_executable("#!/bin/sh\nyes x | head -c 9000\n");
        assert!(matches!(
            run_git(&stdout_executable, stdout_directory.path(), ["ignored"]).await,
            Err(GitError::StdoutTooLarge)
        ));
        let (stderr_directory, stderr_executable) =
            fixture_executable("#!/bin/sh\nyes x | head -c 9000 >&2\n");
        assert!(matches!(
            run_git(&stderr_executable, stderr_directory.path(), ["ignored"]).await,
            Err(GitError::StderrTooLarge)
        ));
    }

    #[tokio::test]
    async fn runner_times_out_and_reaps_the_child() {
        let (directory, executable) = fixture_executable("#!/bin/sh\nwhile :; do :; done\n");
        assert!(matches!(
            run_git_with_timeout(
                &executable,
                directory.path(),
                ["ignored"],
                std::time::Duration::from_millis(20),
            )
            .await,
            Err(GitError::TimedOut)
        ));
    }

    #[test]
    fn unavailable_fingerprint_fails_closed() {
        assert!(matches!(
            repository_fingerprint(Path::new("/private/agent-sentinel-no-such-git-directory")),
            Err(GitError::FingerprintUnavailable)
        ));
    }
}

#[cfg(all(test, windows))]
mod windows_discovery_tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    struct FixtureProvider(Vec<PathBuf>);

    impl WindowsInstallLocationProvider for FixtureProvider {
        fn machine_program_files(&self) -> Result<Vec<PathBuf>, GitError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn candidates_are_derived_only_from_os_backed_provider_results() {
        let root = PathBuf::from(r"C:\\Program Files");
        let candidates = windows_machine_git_candidates_with(&FixtureProvider(vec![root.clone()]))
            .expect("OS-backed fixture provider");
        assert_eq!(
            candidates,
            vec![
                root.join(r"Git\\cmd\\git.exe"),
                root.join(r"Git\\bin\\git.exe"),
            ]
        );
    }

    #[test]
    fn provider_failure_fails_closed() {
        struct FailingProvider;
        impl WindowsInstallLocationProvider for FailingProvider {
            fn machine_program_files(&self) -> Result<Vec<PathBuf>, GitError> {
                Err(GitError::GitDiscoveryFailed)
            }
        }
        assert!(matches!(
            windows_machine_git_candidates_with(&FailingProvider),
            Err(GitError::GitDiscoveryFailed)
        ));
    }

    #[test]
    fn hostile_environment_roots_cannot_change_provider_candidates() {
        let keys = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)", "PATH"];
        let saved: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in keys {
            std::env::set_var(key, r"C:\\hostile");
        }
        let root = PathBuf::from(r"C:\\Program Files");
        let candidates = windows_machine_git_candidates_with(&FixtureProvider(vec![root.clone()]))
            .expect("provider candidates");
        for (key, value) in saved {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        assert!(candidates
            .iter()
            .all(|candidate| candidate.starts_with(&root)));
    }

    #[test]
    fn directory_fingerprint_uses_the_production_backup_semantics_handle_path() {
        let parent = tempfile::tempdir().expect("temporary parent");
        let common_directory = parent.path().join("common directory ünicode");
        std::fs::create_dir(&common_directory).expect("common directory");

        let first = repository_fingerprint(&common_directory).expect("directory fingerprint");
        let second = repository_fingerprint(&common_directory).expect("stable fingerprint");
        assert!(first.as_str().starts_with("strong_v1:windows_object_v1:"));
        assert_eq!(first, second);

        std::fs::write(common_directory.join("ordinary file.txt"), "safe")
            .expect("ordinary file change");
        assert_eq!(
            first,
            repository_fingerprint(&common_directory).expect("stable after file change")
        );
        assert!(matches!(
            repository_fingerprint(&common_directory.join("ordinary file.txt")),
            Err(GitError::FingerprintUnavailable)
        ));
        assert!(matches!(
            repository_fingerprint(&common_directory.join("missing")),
            Err(GitError::FingerprintUnavailable)
        ));
    }

    #[test]
    fn wide_paths_reject_embedded_nuls_before_create_file() {
        let embedded_nul = std::ffi::OsString::from_wide(&[b'C' as u16, 0, b'X' as u16]);
        assert!(matches!(
            windows_wide_path(Path::new(&embedded_nul)),
            Err(GitError::FingerprintUnavailable)
        ));
    }
}
