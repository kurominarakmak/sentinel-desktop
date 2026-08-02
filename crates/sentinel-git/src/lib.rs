use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
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
    #[error("git status output is malformed")]
    StatusMalformed,
    #[error("git status output has too many records")]
    StatusTooManyRecords,
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
    #[error("filter-free inventory is not available")]
    InventoryUnavailable,
    #[error("filter-free worktree cleanliness verification is not available")]
    CleanlinessUnavailable,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Maximum bytes accepted from the read-only Phase 3C-A status operation.
pub const MAX_STATUS_STDOUT: usize = 256 * 1024;
pub const MAX_STATUS_STDERR: usize = 16 * 1024;
pub const MAX_STATUS_RECORDS: usize = 1_000;
pub const MAX_REPOSITORY_RELATIVE_PATH_BYTES: usize = 4 * 1024;
#[cfg(test)]
const STATUS_INVENTORY_ARGS: [&str; 11] = [
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.untrackedCache=false",
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=none",
    "--no-renames",
];
pub const MAX_NUMSTAT_STDOUT: usize = 32 * 1024;
pub const MAX_NUMSTAT_STDERR: usize = 16 * 1024;
pub const MAX_STAGED_RAW_STDOUT: usize = 256 * 1024;
pub const MAX_STAGED_RAW_STDERR: usize = 16 * 1024;
pub const MAX_FILTER_ATTRIBUTE_STDOUT: usize = 16 * 1024;
pub const MAX_FILTER_ATTRIBUTE_STDERR: usize = 16 * 1024;
pub const MAX_INVENTORY_STDOUT: usize = 256 * 1024;
pub const MAX_INVENTORY_STDERR: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEligibleMetadata {
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumstatClassification {
    NoChanges,
    TextEligible(TextEligibleMetadata),
    Binary,
}

/// The only filter states which are safe to use for a working-tree diff.
/// Driver names and configuration are deliberately not represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterAttributeState {
    SafeUnspecified,
    SafeUnset,
    Deferred,
}

/// Non-authoritative, filter-free base-to-index metadata. It is deliberately
/// not a complete worktree inventory: AH1 has no safe unstaged comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedIndexChange {
    pub path: RepositoryRelativePath,
    pub old_mode: Option<RepositoryMode>,
    pub new_mode: Option<RepositoryMode>,
    pub status: ChangeKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepositoryRelativePath(String);

impl RepositoryRelativePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    BothDeleted,
    AddedByUs,
    DeletedByThem,
    AddedByThem,
    DeletedByUs,
    BothAdded,
    BothModified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryMode {
    Absent,
    Regular,
    Executable,
    SymbolicLink,
    Gitlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmoduleStatus {
    pub commit_changed: bool,
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: RepositoryRelativePath,
    pub index_change: Option<ChangeKind>,
    pub worktree_change: Option<ChangeKind>,
    pub conflict: Option<ConflictKind>,
    pub untracked: bool,
    pub submodule: Option<SubmoduleStatus>,
    pub mode_head: Option<RepositoryMode>,
    pub mode_index: Option<RepositoryMode>,
    pub mode_worktree: Option<RepositoryMode>,
}

/// The result of a complete, successfully parsed worktree-list lookup.  This
/// deliberately has no boolean default: callers must not turn a malformed
/// metadata response into an absent entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeMetadataLookup {
    Present,
    Absent,
}

/// No-follow state for a worktree destination.  Only `Missing` is a proven
/// absence; all other states must be handled explicitly by ownership callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeDestinationState {
    Missing,
    RealDirectory,
    UnsafeLinkOrReparse,
    NonDirectory,
    MetadataUnavailable,
}

/// A lexically validated, absolute porcelain worktree path.  Its comparison
/// key is platform-normalized without consulting the filesystem, so prunable
/// worktrees remain parseable while aliases are rejected.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidatedWorktreePath {
    path: PathBuf,
    comparison_key: String,
}

impl ValidatedWorktreePath {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn comparison_key(&self) -> &str {
        &self.comparison_key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeMetadataRecord {
    pub path: ValidatedWorktreePath,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
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
/// The complete authoritative inventory, including both snapshots, has one
/// shared budget. Per-command limits are still applied, but may never reset
/// this operation-level deadline.
const INVENTORY_OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

tokio::task_local! {
    static INVENTORY_DEADLINE: std::time::Instant;
}

tokio::task_local! {
    static INVENTORY_GIT_ENVIRONMENT: InventoryGitEnvironment;
}

/// Runs a caller-owned authoritative inventory operation under one monotonic
/// budget. Nested inventory helpers observe this scope and never allocate a
/// replacement deadline.
pub async fn with_inventory_operation_deadline<T>(
    future: impl std::future::Future<Output = T>,
) -> T {
    INVENTORY_DEADLINE
        .scope(
            std::time::Instant::now() + INVENTORY_OPERATION_TIMEOUT,
            future,
        )
        .await
}

/// Runs a caller-owned read-only operation with the same trusted Git
/// configuration isolation and absolute deadline used by authoritative
/// inventory. This is the only supported context for B1 metadata commands.
pub async fn with_inventory_operation_context<T>(
    future: impl std::future::Future<Output = T>,
) -> Result<T, GitError> {
    let environment = InventoryGitEnvironment::create()?;
    Ok(INVENTORY_GIT_ENVIRONMENT
        .scope(environment, with_inventory_operation_deadline(future))
        .await)
}

/// Test-independent, application-owned Git configuration isolation retained
/// for one complete inventory operation. It is outside the repository and is
/// kept alive until every command has finished.
struct InventoryGitEnvironment {
    home: tempfile::TempDir,
    global_config: PathBuf,
}

impl InventoryGitEnvironment {
    fn create() -> Result<Self, GitError> {
        let home = tempfile::Builder::new()
            .prefix("agent-sentinel-git-home-")
            .tempdir()
            .map_err(|_| GitError::InventoryUnavailable)?;
        let global_config = home.path().join("global.gitconfig");
        std::fs::File::create(&global_config).map_err(|_| GitError::InventoryUnavailable)?;
        Ok(Self {
            home,
            global_config,
        })
    }

    fn home(&self) -> &Path {
        self.home.path()
    }
}

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

/// Resolves the current HEAD to an immutable full commit object through the
/// same trusted, bounded Git boundary used for repository inspection.
pub async fn resolve_exact_head(repository: &Path) -> Result<String, GitError> {
    let inspection = inspect_repository(repository).await?;
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git(
        &executable,
        &inspection.repository_root,
        ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )
    .await?;
    let commit = nonempty_output(output.stdout)?;
    if !output.status.success()
        || commit.len() != 40
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(GitError::MetadataInvalid);
    }
    Ok(commit)
}

/// Adds a detached worktree using backend-owned path and exact OID arguments.
/// Callers must perform containment and ownership checks before and after this
/// intentionally mutating operation.
pub async fn add_detached_worktree(
    repository: &Path,
    destination: &Path,
    commit: &str,
) -> Result<(), GitError> {
    if inspect_worktree_destination_no_follow(destination) != WorktreeDestinationState::Missing
        || commit.len() != 40
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(GitError::MetadataInvalid);
    }
    let inspection = inspect_repository(repository).await?;
    let executable = TrustedGitExecutableResolver.resolve()?;
    let destination = destination.to_str().ok_or(GitError::MetadataInvalid)?;
    let output = run_git(
        &executable,
        &inspection.repository_root,
        ["worktree", "add", "--detach", destination, commit],
    )
    .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GitError::MetadataInvalid)
    }
}

pub async fn worktree_is_clean(worktree: &Path) -> Result<bool, GitError> {
    let _ = worktree;
    // Inventory is now authoritative, but managed-worktree deletion remains
    // deliberately blocked until the holistic Phase 3C-A review.
    Err(GitError::CleanlinessUnavailable)
}

/// Returns a complete, read-only porcelain-v2 inventory for a caller-verified
/// linked worktree.  The caller owns exact managed-leaf validation; this crate
/// deliberately accepts only a backend-provided directory and fixed arguments.
pub async fn inspect_worktree_changes(worktree: &Path) -> Result<Vec<ChangedFile>, GitError> {
    let environment = InventoryGitEnvironment::create()?;
    let deadline = std::time::Instant::now() + INVENTORY_OPERATION_TIMEOUT;
    INVENTORY_GIT_ENVIRONMENT
        .scope(
            environment,
            INVENTORY_DEADLINE.scope(deadline, async {
                let inspection = inspect_repository(worktree)
                    .await
                    .map_err(|_| GitError::InventoryUnavailable)?;
                let base = inspection.head.ok_or(GitError::InventoryUnavailable)?;
                inspect_worktree_changes_at_base_in_context(worktree, &base).await
            }),
        )
        .await
}

/// Produces a complete, filter-free inventory for a linked worktree whose
/// caller has already recorded the expected immutable base commit. Two exact
/// snapshots must agree before a successful result is returned; ambiguity is
/// deliberately represented as `InventoryUnavailable`, never as clean.
pub async fn inspect_worktree_changes_at_base(
    worktree: &Path,
    expected_base: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    let environment = InventoryGitEnvironment::create()?;
    let operation =
        async { inspect_worktree_changes_at_base_in_context(worktree, expected_base).await };
    if INVENTORY_DEADLINE.try_with(|_| ()).is_ok() {
        INVENTORY_GIT_ENVIRONMENT
            .scope(environment, operation)
            .await
    } else {
        INVENTORY_GIT_ENVIRONMENT
            .scope(environment, with_inventory_operation_deadline(operation))
            .await
    }
}

async fn inspect_worktree_changes_at_base_in_context(
    worktree: &Path,
    expected_base: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    if !valid_oid(expected_base.as_bytes()) {
        return Err(GitError::InventoryUnavailable);
    }
    let before = inspect_repository(worktree)
        .await
        .map_err(|_| GitError::InventoryUnavailable)?;
    if before.is_primary || before.head.as_deref() != Some(expected_base) {
        return Err(GitError::InventoryUnavailable);
    }
    let executable = TrustedGitExecutableResolver
        .resolve()
        .map_err(|_| GitError::InventoryUnavailable)?;
    let first =
        collect_inventory_snapshot(&executable, &before.repository_root, expected_base).await?;
    let second =
        collect_inventory_snapshot(&executable, &before.repository_root, expected_base).await?;
    let after = inspect_repository(worktree)
        .await
        .map_err(|_| GitError::InventoryUnavailable)?;
    if after.is_primary
        || after.identity != before.identity
        || after.fingerprint != before.fingerprint
        || after.repository_root != before.repository_root
        || after.head.as_deref() != Some(expected_base)
        || first != second
    {
        return Err(GitError::InventoryUnavailable);
    }
    merge_inventory_snapshot(first)
}

/// Test-only control preserving the security regression. Production code must
/// never call this stock-status operation.
#[cfg(test)]
pub async fn legacy_unsafe_status_control(worktree: &Path) -> Result<Vec<ChangedFile>, GitError> {
    let inspection = inspect_repository(worktree).await?;
    if inspection.is_primary {
        return Err(GitError::MetadataInvalid);
    }
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git_with_limits(
        &executable,
        &inspection.repository_root,
        STATUS_INVENTORY_ARGS,
        GIT_TIMEOUT,
        MAX_STATUS_STDOUT,
        MAX_STATUS_STDERR,
        true,
        false,
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::Command("status failed".into()));
    }
    parse_status_porcelain_v2_z(&output.stdout)
}

/// Reads bounded base-to-index raw metadata without asking Git to compare a
/// working-tree file. This is AH1 foundation data only, never a full inventory.
pub async fn inspect_staged_index_changes(
    worktree: &Path,
    base_commit: &str,
) -> Result<Vec<StagedIndexChange>, GitError> {
    if !valid_oid(base_commit.as_bytes()) {
        return Err(GitError::MetadataInvalid);
    }
    let inspection = inspect_repository(worktree).await?;
    if inspection.is_primary {
        return Err(GitError::MetadataInvalid);
    }
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git_with_limits(
        &executable,
        &inspection.repository_root,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "diff-index",
            "--cached",
            "--raw",
            "-z",
            "--no-renames",
            base_commit,
            "--",
        ],
        GIT_TIMEOUT,
        MAX_STAGED_RAW_STDOUT,
        MAX_STAGED_RAW_STDERR,
        true,
        true,
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::Command("staged raw metadata failed".into()));
    }
    parse_staged_raw_z(&output.stdout)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InventorySnapshot {
    index_before: Vec<u8>,
    staged: Vec<u8>,
    unstaged: Vec<StagedIndexChange>,
    untracked: Vec<u8>,
    unmerged: Vec<u8>,
    index_after: Vec<u8>,
}

async fn inventory_output<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
) -> Result<Vec<u8>, GitError> {
    ensure_inventory_deadline()?;
    let output = run_git_with_limits(
        executable,
        directory,
        args,
        GIT_TIMEOUT,
        MAX_INVENTORY_STDOUT,
        MAX_INVENTORY_STDERR,
        true,
        true,
    )
    .await
    .map_err(|_| GitError::InventoryUnavailable)?;
    if !output.status.success() {
        return Err(GitError::InventoryUnavailable);
    }
    ensure_inventory_deadline()?;
    Ok(output.stdout)
}

fn ensure_inventory_deadline() -> Result<(), GitError> {
    match INVENTORY_DEADLINE
        .try_with(|deadline| (std::time::Instant::now() < *deadline).then_some(()))
    {
        Ok(Some(())) | Err(_) => Ok(()),
        Ok(None) => Err(GitError::InventoryUnavailable),
    }
}

fn cap_timeout_to_inventory_deadline(
    timeout: std::time::Duration,
) -> Result<std::time::Duration, GitError> {
    match INVENTORY_DEADLINE
        .try_with(|deadline| deadline.saturating_duration_since(std::time::Instant::now()))
    {
        Ok(remaining) if remaining.is_zero() => Err(GitError::TimedOut),
        Ok(remaining) => Ok(timeout.min(remaining)),
        Err(_) => Ok(timeout),
    }
}

async fn collect_inventory_snapshot(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    base: &str,
) -> Result<InventorySnapshot, GitError> {
    ensure_inventory_deadline()?;
    let index_flags = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "ls-files",
            "-t",
            "-z",
            "--",
        ],
    )
    .await?;
    if has_skip_worktree(&index_flags)? {
        return Err(GitError::InventoryUnavailable);
    }
    let index_before = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "ls-files",
            "--stage",
            "-z",
            "--",
        ],
    )
    .await?;
    let staged = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "color.ui=false",
            "diff-index",
            "--cached",
            "--raw",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--ignore-submodules=none",
            base,
            "--",
        ],
    )
    .await?;
    let unstaged = collect_worktree_changes(executable, directory, &index_before).await?;
    let untracked = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ],
    )
    .await?;
    let unmerged = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "ls-files",
            "--unmerged",
            "-z",
            "--",
        ],
    )
    .await?;
    let index_after = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "ls-files",
            "--stage",
            "-z",
            "--",
        ],
    )
    .await?;
    if index_before != index_after {
        return Err(GitError::InventoryUnavailable);
    }
    Ok(InventorySnapshot {
        index_before,
        staged,
        unstaged,
        untracked,
        unmerged,
        index_after,
    })
}

#[derive(Clone, Debug)]
struct IndexStageEntry {
    path: RepositoryRelativePath,
    mode: RepositoryMode,
    oid: String,
    stage: u8,
}

fn parse_index_stage_z(bytes: &[u8]) -> Result<Vec<IndexStageEntry>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for raw in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let mut record = raw.splitn(2, |byte| *byte == b'\t');
        let (Some(header), Some(path)) = (record.next(), record.next()) else {
            return Err(GitError::StatusMalformed);
        };
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) || !valid_oid(fields[1])
        {
            return Err(GitError::StatusMalformed);
        }
        let stage = match fields[2] {
            b"0" => 0,
            b"1" => 1,
            b"2" => 2,
            b"3" => 3,
            _ => return Err(GitError::StatusMalformed),
        };
        let path = parse_repository_relative_path(path)?;
        if !seen.insert((path.0.clone(), stage)) || entries.len() == MAX_STATUS_RECORDS {
            return Err(GitError::StatusTooManyRecords);
        }
        entries.push(IndexStageEntry {
            path,
            mode: parse_mode(fields[0])?,
            oid: std::str::from_utf8(fields[1])
                .map_err(|_| GitError::StatusMalformed)?
                .to_owned(),
            stage,
        });
    }
    Ok(entries)
}

async fn hash_worktree_path(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    path: &RepositoryRelativePath,
) -> Result<String, GitError> {
    validate_inventory_lookup_path(path.as_str(), current_lookup_path_semantics())
        .map_err(|_| GitError::InventoryUnavailable)?;
    let output = inventory_output(
        executable,
        directory,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "hash-object",
            "--no-filters",
            "--",
            path.as_str(),
        ],
    )
    .await?;
    let Some(oid) = output.strip_suffix(b"\n") else {
        return Err(GitError::InventoryUnavailable);
    };
    if !valid_oid(oid) {
        return Err(GitError::InventoryUnavailable);
    }
    std::str::from_utf8(oid)
        .map(|value| value.to_owned())
        .map_err(|_| GitError::InventoryUnavailable)
}

fn worktree_mode(metadata: &fs::Metadata) -> Result<RepositoryMode, GitError> {
    if metadata.file_type().is_symlink() {
        return Ok(RepositoryMode::SymbolicLink);
    }
    if !metadata.is_file() {
        return Err(GitError::InventoryUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(if metadata.permissions().mode() & 0o111 == 0 {
            RepositoryMode::Regular
        } else {
            RepositoryMode::Executable
        })
    }
    #[cfg(not(unix))]
    Ok(RepositoryMode::Regular)
}

async fn collect_worktree_changes(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    index: &[u8],
) -> Result<Vec<StagedIndexChange>, GitError> {
    let mut changes = Vec::new();
    for entry in parse_index_stage_z(index).map_err(|_| GitError::InventoryUnavailable)? {
        if entry.stage != 0 {
            continue;
        }
        if entry.mode == RepositoryMode::Gitlink {
            // A generic linked submodule's nested state needs a separate
            // snapshot contract. Do not silently flatten it into a file.
            return Err(GitError::InventoryUnavailable);
        }
        validate_inventory_lookup_path(entry.path.as_str(), current_lookup_path_semantics())
            .map_err(|_| GitError::InventoryUnavailable)?;
        ensure_inventory_deadline()?;
        let path = directory.join(entry.path.as_str());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                changes.push(StagedIndexChange {
                    path: entry.path,
                    old_mode: Some(entry.mode),
                    new_mode: None,
                    status: ChangeKind::Deleted,
                });
                continue;
            }
            Err(_) => return Err(GitError::InventoryUnavailable),
        };
        if metadata.is_dir() {
            // A tracked file replaced by a directory is represented as a
            // tracked deletion; any descendants are independently returned by
            // the untracked component.
            changes.push(StagedIndexChange {
                path: entry.path,
                old_mode: Some(entry.mode),
                new_mode: None,
                status: ChangeKind::Deleted,
            });
            continue;
        }
        let actual_mode = worktree_mode(&metadata)?;
        let actual_oid = hash_worktree_path(executable, directory, &entry.path).await?;
        let status = if actual_mode != entry.mode {
            ChangeKind::TypeChanged
        } else if actual_oid != entry.oid {
            ChangeKind::Modified
        } else {
            continue;
        };
        changes.push(StagedIndexChange {
            path: entry.path,
            old_mode: Some(entry.mode),
            new_mode: Some(actual_mode),
            status,
        });
    }
    Ok(changes)
}

fn merge_inventory_snapshot(snapshot: InventorySnapshot) -> Result<Vec<ChangedFile>, GitError> {
    let staged =
        parse_staged_raw_z(&snapshot.staged).map_err(|_| GitError::InventoryUnavailable)?;
    let untracked =
        parse_untracked_paths_z(&snapshot.untracked).map_err(|_| GitError::InventoryUnavailable)?;
    let conflicts =
        parse_unmerged_paths_z(&snapshot.unmerged).map_err(|_| GitError::InventoryUnavailable)?;
    let mut files: HashMap<String, ChangedFile> = HashMap::new();
    for change in staged {
        let key = change.path.0.clone();
        let file = files
            .entry(key)
            .or_insert_with(|| empty_changed_file(change.path.clone()));
        if file.index_change.replace(change.status).is_some() {
            return Err(GitError::InventoryUnavailable);
        }
        file.mode_head = change.old_mode;
        file.mode_index = change.new_mode;
        note_gitlink(file, change.old_mode, change.new_mode);
    }
    for change in snapshot.unstaged {
        let key = change.path.0.clone();
        let file = files
            .entry(key)
            .or_insert_with(|| empty_changed_file(change.path.clone()));
        if file.worktree_change.replace(change.status).is_some() {
            return Err(GitError::InventoryUnavailable);
        }
        if file.mode_index.is_none() {
            file.mode_index = change.old_mode;
        }
        file.mode_worktree = change.new_mode;
        note_gitlink(file, change.old_mode, change.new_mode);
    }
    for path in untracked {
        let key = path.0.clone();
        if files.contains_key(&key) {
            return Err(GitError::InventoryUnavailable);
        }
        let mut file = empty_changed_file(path);
        file.untracked = true;
        files.insert(key, file);
    }
    for (path, conflict, head_mode, index_mode) in conflicts {
        let key = path.0.clone();
        let file = files.entry(key).or_insert_with(|| empty_changed_file(path));
        if file.conflict.replace(conflict).is_some() || file.untracked {
            return Err(GitError::InventoryUnavailable);
        }
        if file.mode_head.is_none() {
            file.mode_head = head_mode;
        }
        if file.mode_index.is_none() {
            file.mode_index = index_mode;
        }
    }
    if files.len() > MAX_STATUS_RECORDS {
        return Err(GitError::InventoryUnavailable);
    }
    let mut files: Vec<_> = files.into_values().collect();
    files.sort_by(|left, right| left.path.0.as_bytes().cmp(right.path.0.as_bytes()));
    Ok(files)
}

fn empty_changed_file(path: RepositoryRelativePath) -> ChangedFile {
    ChangedFile {
        path,
        index_change: None,
        worktree_change: None,
        conflict: None,
        untracked: false,
        submodule: None,
        mode_head: None,
        mode_index: None,
        mode_worktree: None,
    }
}

fn note_gitlink(
    file: &mut ChangedFile,
    old_mode: Option<RepositoryMode>,
    new_mode: Option<RepositoryMode>,
) {
    if matches!(old_mode, Some(RepositoryMode::Gitlink))
        || matches!(new_mode, Some(RepositoryMode::Gitlink))
    {
        file.submodule = Some(SubmoduleStatus {
            commit_changed: true,
            modified: file.worktree_change.is_some(),
            untracked: false,
        });
    }
}

/// Runs one fixed, literal-pathspec numstat operation for a caller-verified
/// linked worktree. It returns metadata only; no file content is interpreted.
pub async fn inspect_worktree_numstat(
    worktree: &Path,
    path: &RepositoryRelativePath,
    base_commit: Option<&str>,
) -> Result<NumstatClassification, GitError> {
    if let Some(base_commit) = base_commit {
        if !valid_oid(base_commit.as_bytes()) {
            return Err(GitError::MetadataInvalid);
        }
    }
    let inspection = inspect_repository(worktree).await?;
    if inspection.is_primary {
        return Err(GitError::MetadataInvalid);
    }
    let executable = TrustedGitExecutableResolver.resolve()?;
    let path = path.as_str();
    let output = if let Some(base_commit) = base_commit {
        run_git_with_limits(
            &executable,
            &inspection.repository_root,
            [
                "--no-optional-locks",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "color.ui=false",
                "diff",
                "--cached",
                "--numstat",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                base_commit,
                "--",
                path,
            ],
            GIT_TIMEOUT,
            MAX_NUMSTAT_STDOUT,
            MAX_NUMSTAT_STDERR,
            true,
            true,
        )
        .await?
    } else {
        run_git_with_limits(
            &executable,
            &inspection.repository_root,
            [
                "--no-optional-locks",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "color.ui=false",
                "diff",
                "--numstat",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--",
                path,
            ],
            GIT_TIMEOUT,
            MAX_NUMSTAT_STDOUT,
            MAX_NUMSTAT_STDERR,
            true,
            true,
        )
        .await?
    };
    if !output.status.success() {
        return Err(GitError::Command("numstat failed".into()));
    }
    parse_numstat_z(&output.stdout, path)
}

/// Reads only the effective `filter` attribute for one caller-verified path.
/// This is a preflight for an unstaged diff: Git clean/process filters must
/// never be launched by the classification path.
pub async fn inspect_worktree_filter_attribute(
    worktree: &Path,
    path: &RepositoryRelativePath,
) -> Result<FilterAttributeState, GitError> {
    let inspection = inspect_repository(worktree).await?;
    if inspection.is_primary {
        return Err(GitError::MetadataInvalid);
    }
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git_with_limits(
        &executable,
        &inspection.repository_root,
        [
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "check-attr",
            "-z",
            "filter",
            "--",
            path.as_str(),
        ],
        GIT_TIMEOUT,
        MAX_FILTER_ATTRIBUTE_STDOUT,
        MAX_FILTER_ATTRIBUTE_STDERR,
        true,
        true,
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::Command(
            "filter attribute inspection failed".into(),
        ));
    }
    parse_filter_attribute_z(&output.stdout, path.as_str())
}

/// Strict parser for one `git check-attr -z filter -- <literal-path>` triple.
pub fn parse_filter_attribute_z(
    bytes: &[u8],
    expected_path: &str,
) -> Result<FilterAttributeState, GitError> {
    if !bytes.ends_with(&[0]) || bytes.is_empty() {
        return Err(GitError::StatusMalformed);
    }
    let fields: Vec<_> = bytes[..bytes.len() - 1].split(|byte| *byte == 0).collect();
    if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) {
        return Err(GitError::StatusMalformed);
    }
    let path = parse_repository_relative_path(fields[0])?;
    if path.as_str() != expected_path || fields[1] != b"filter" {
        return Err(GitError::StatusMalformed);
    }
    let value = std::str::from_utf8(fields[2]).map_err(|_| GitError::StatusMalformed)?;
    match value {
        "unspecified" => Ok(FilterAttributeState::SafeUnspecified),
        "unset" => Ok(FilterAttributeState::SafeUnset),
        "set" => Ok(FilterAttributeState::Deferred),
        value if !value.is_empty() => Ok(FilterAttributeState::Deferred),
        _ => Err(GitError::StatusMalformed),
    }
}

/// Strict parser for one fixed `git diff --numstat -z -- <literal-path>`
/// record. Empty output is represented explicitly; multiple records are never
/// accepted for a one-path request.
pub fn parse_numstat_z(
    bytes: &[u8],
    expected_path: &str,
) -> Result<NumstatClassification, GitError> {
    if bytes.is_empty() {
        return Ok(NumstatClassification::NoChanges);
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let records: Vec<_> = bytes[..bytes.len() - 1].split(|byte| *byte == 0).collect();
    if records.len() != 1 || records[0].is_empty() {
        return Err(GitError::StatusMalformed);
    }
    let mut fields = records[0].splitn(3, |byte| *byte == b'\t');
    let added = fields.next().ok_or(GitError::StatusMalformed)?;
    let deleted = fields.next().ok_or(GitError::StatusMalformed)?;
    let path = fields.next().ok_or(GitError::StatusMalformed)?;
    let path = parse_repository_relative_path(path)?;
    if path.as_str() != expected_path {
        return Err(GitError::StatusMalformed);
    }
    if added == b"-" && deleted == b"-" {
        return Ok(NumstatClassification::Binary);
    }
    if added == b"-" || deleted == b"-" {
        return Err(GitError::StatusMalformed);
    }
    let parse_count = |value: &[u8]| {
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(GitError::StatusMalformed);
        }
        let value = std::str::from_utf8(value).map_err(|_| GitError::StatusMalformed)?;
        value.parse::<u32>().map_err(|_| GitError::StatusMalformed)
    };
    Ok(NumstatClassification::TextEligible(TextEligibleMetadata {
        additions: parse_count(added)?,
        deletions: parse_count(deleted)?,
    }))
}

/// Strict parser for the one-path-free `diff-index --cached --raw -z` stream.
/// Each non-rename record has a NUL-terminated header followed by one
/// NUL-terminated path. AH1 rejects conflict and rename/copy encodings until
/// their dedicated safe metadata operations exist.
pub fn parse_staged_raw_z(bytes: &[u8]) -> Result<Vec<StagedIndexChange>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let fields: Vec<_> = bytes[..bytes.len() - 1].split(|byte| *byte == 0).collect();
    if fields.len() % 2 != 0 || fields.len() / 2 > MAX_STATUS_RECORDS {
        return Err(GitError::StatusMalformed);
    }
    let mut paths = HashSet::new();
    let mut changes = Vec::with_capacity(fields.len() / 2);
    for pair in fields.chunks_exact(2) {
        let header = pair[0];
        let path = parse_repository_relative_path(pair[1])?;
        if !paths.insert(path.clone()) {
            return Err(GitError::StatusMalformed);
        }
        let header = std::str::from_utf8(header).map_err(|_| GitError::StatusMalformed)?;
        let parts: Vec<_> = header.split(' ').collect();
        if parts.len() != 5 || !parts[0].starts_with(':') {
            return Err(GitError::StatusMalformed);
        }
        let old_mode = parse_mode(&parts[0].as_bytes()[1..])?;
        let new_mode = parse_mode(parts[1].as_bytes())?;
        if !valid_oid(parts[2].as_bytes()) || !valid_oid(parts[3].as_bytes()) {
            return Err(GitError::StatusMalformed);
        }
        let status = match parts[4] {
            "A" => ChangeKind::Added,
            "M" => ChangeKind::Modified,
            "D" => ChangeKind::Deleted,
            "T" => ChangeKind::TypeChanged,
            // Unmerged entries are represented completely by the separately
            // parsed `ls-files --unmerged` stage set below. Raw diff's `U`
            // marker carries no safe conflict classification on its own.
            "U" => continue,
            _ => return Err(GitError::StatusMalformed),
        };
        changes.push(StagedIndexChange {
            path,
            old_mode: (old_mode != RepositoryMode::Absent).then_some(old_mode),
            new_mode: (new_mode != RepositoryMode::Absent).then_some(new_mode),
            status,
        });
    }
    Ok(changes)
}

fn parse_untracked_paths_z(bytes: &[u8]) -> Result<Vec<RepositoryRelativePath>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let mut paths = HashSet::new();
    let mut result = Vec::new();
    for raw in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let path = parse_repository_relative_path(raw)?;
        if !paths.insert(path.0.clone()) || result.len() == MAX_STATUS_RECORDS {
            return Err(GitError::StatusTooManyRecords);
        }
        result.push(path);
    }
    Ok(result)
}

type UnmergedPath = (
    RepositoryRelativePath,
    ConflictKind,
    Option<RepositoryMode>,
    Option<RepositoryMode>,
);

fn parse_unmerged_paths_z(bytes: &[u8]) -> Result<Vec<UnmergedPath>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let mut stages: HashMap<String, (RepositoryRelativePath, [Option<RepositoryMode>; 3])> =
        HashMap::new();
    for raw in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let mut record = raw.splitn(2, |byte| *byte == b'\t');
        let (Some(header), Some(path)) = (record.next(), record.next()) else {
            return Err(GitError::StatusMalformed);
        };
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) || !valid_oid(fields[1])
        {
            return Err(GitError::StatusMalformed);
        }
        let mode = parse_mode(fields[0])?;
        let stage = match fields[2] {
            b"1" => 0,
            b"2" => 1,
            b"3" => 2,
            _ => return Err(GitError::StatusMalformed),
        };
        let path = parse_repository_relative_path(path)?;
        let entry = stages
            .entry(path.0.clone())
            .or_insert_with(|| (path.clone(), [None, None, None]));
        if entry.1[stage].replace(mode).is_some() || stages.len() > MAX_STATUS_RECORDS {
            return Err(GitError::StatusTooManyRecords);
        }
    }
    let mut result = Vec::with_capacity(stages.len());
    for (_, (path, stages)) in stages {
        let present = stages.map(|stage| stage.is_some());
        let conflict = match present {
            [true, false, false] => ConflictKind::BothDeleted,
            [false, true, false] => ConflictKind::AddedByUs,
            [true, false, true] => ConflictKind::DeletedByThem,
            [false, false, true] => ConflictKind::AddedByThem,
            [true, true, false] => ConflictKind::DeletedByUs,
            [false, true, true] => ConflictKind::BothAdded,
            [true, true, true] => ConflictKind::BothModified,
            [false, false, false] => return Err(GitError::StatusMalformed),
        };
        result.push((path, conflict, stages[0], stages[1].or(stages[2])));
    }
    Ok(result)
}

/// Strict parser for the fixed Phase 3C-A porcelain-v2, NUL-delimited status
/// command. Empty output is the sole valid representation of a clean tree.
pub fn parse_status_porcelain_v2_z(bytes: &[u8]) -> Result<Vec<ChangedFile>, GitError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let mut files = Vec::new();
    let mut paths = HashSet::new();
    for record in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if record.is_empty() {
            return Err(GitError::StatusMalformed);
        }
        let file = match record.first().copied() {
            Some(b'1') if record.get(1) == Some(&b' ') => parse_status_ordinary(record)?,
            Some(b'u') if record.get(1) == Some(&b' ') => parse_status_unmerged(record)?,
            Some(b'?') if record.get(1) == Some(&b' ') => ChangedFile {
                path: parse_repository_relative_path(&record[2..])?,
                index_change: None,
                worktree_change: None,
                conflict: None,
                untracked: true,
                submodule: None,
                mode_head: None,
                mode_index: None,
                mode_worktree: None,
            },
            Some(b'2') | Some(b'!') | Some(b'#') => return Err(GitError::StatusMalformed),
            _ => return Err(GitError::StatusMalformed),
        };
        if !paths.insert(file.path.0.clone()) {
            return Err(GitError::StatusMalformed);
        }
        files.push(file);
        if files.len() > MAX_STATUS_RECORDS {
            return Err(GitError::StatusTooManyRecords);
        }
    }
    files.sort_by(|left, right| left.path.0.as_bytes().cmp(right.path.0.as_bytes()));
    Ok(files)
}

fn fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>, GitError> {
    let values: Vec<_> = record.splitn(count, |byte| *byte == b' ').collect();
    if values.len() != count || values.iter().any(|value| value.is_empty()) {
        return Err(GitError::StatusMalformed);
    }
    Ok(values)
}

fn parse_status_ordinary(record: &[u8]) -> Result<ChangedFile, GitError> {
    // 1 XY SUB MHEAD MINDEX MWORKTREE HHEAD HINDEX PATH
    let fields = fields(record, 9)?;
    let (tag, xy, sub, head_mode, index_mode, worktree_mode, head_oid, index_oid, path) = (
        fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7],
        fields[8],
    );
    if tag != b"1" || !valid_oid(head_oid) || !valid_oid(index_oid) {
        return Err(GitError::StatusMalformed);
    }
    let (index_change, worktree_change) = parse_xy(xy)?;
    // `status --porcelain=v2` emits no ordinary record for a clean path.  A
    // syntactically valid `..` record is therefore malformed rather than a
    // harmless empty change: accepting it would make a clean inventory appear
    // dirty.
    if index_change.is_none() && worktree_change.is_none() {
        return Err(GitError::StatusMalformed);
    }
    Ok(ChangedFile {
        path: parse_repository_relative_path(path)?,
        index_change,
        worktree_change,
        conflict: None,
        untracked: false,
        submodule: parse_submodule(sub)?,
        mode_head: Some(parse_mode(head_mode)?),
        mode_index: Some(parse_mode(index_mode)?),
        mode_worktree: Some(parse_mode(worktree_mode)?),
    })
}

fn parse_status_unmerged(record: &[u8]) -> Result<ChangedFile, GitError> {
    // u XY SUB M1 M2 M3 MWORKTREE H1 H2 H3 PATH
    let fields = fields(record, 11)?;
    let (tag, xy, sub, mode1, mode2, mode3, worktree_mode, oid1, oid2, oid3, path) = (
        fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7],
        fields[8], fields[9], fields[10],
    );
    if tag != b"u" || !valid_oid(oid1) || !valid_oid(oid2) || !valid_oid(oid3) {
        return Err(GitError::StatusMalformed);
    }
    let conflict = match xy {
        b"DD" => ConflictKind::BothDeleted,
        b"AU" => ConflictKind::AddedByUs,
        b"UD" => ConflictKind::DeletedByThem,
        b"UA" => ConflictKind::AddedByThem,
        b"DU" => ConflictKind::DeletedByUs,
        b"AA" => ConflictKind::BothAdded,
        b"UU" => ConflictKind::BothModified,
        _ => return Err(GitError::StatusMalformed),
    };
    Ok(ChangedFile {
        path: parse_repository_relative_path(path)?,
        index_change: None,
        worktree_change: None,
        conflict: Some(conflict),
        untracked: false,
        submodule: parse_submodule(sub)?,
        mode_head: Some(parse_mode(mode1)?),
        mode_index: Some(parse_mode(mode2)?),
        mode_worktree: Some(parse_mode(mode3)?),
    })
    .and_then(|file| {
        let _ = parse_mode(worktree_mode)?;
        Ok(file)
    })
}

fn parse_xy(value: &[u8]) -> Result<(Option<ChangeKind>, Option<ChangeKind>), GitError> {
    if value.len() != 2 {
        return Err(GitError::StatusMalformed);
    }
    Ok((parse_change(value[0])?, parse_change(value[1])?))
}
fn parse_change(value: u8) -> Result<Option<ChangeKind>, GitError> {
    Ok(match value {
        b'.' => None,
        b'A' => Some(ChangeKind::Added),
        b'M' => Some(ChangeKind::Modified),
        b'D' => Some(ChangeKind::Deleted),
        b'T' => Some(ChangeKind::TypeChanged),
        _ => return Err(GitError::StatusMalformed),
    })
}
fn parse_submodule(value: &[u8]) -> Result<Option<SubmoduleStatus>, GitError> {
    if value == b"N..." {
        return Ok(None);
    }
    if value.len() != 4 || value[0] != b'S' {
        return Err(GitError::StatusMalformed);
    }
    let valid = |actual: u8, expected: u8| actual == b'.' || actual == expected;
    if !valid(value[1], b'C') || !valid(value[2], b'M') || !valid(value[3], b'U') {
        return Err(GitError::StatusMalformed);
    }
    Ok(Some(SubmoduleStatus {
        commit_changed: value[1] == b'C',
        modified: value[2] == b'M',
        untracked: value[3] == b'U',
    }))
}
fn parse_mode(value: &[u8]) -> Result<RepositoryMode, GitError> {
    match value {
        b"000000" => Ok(RepositoryMode::Absent),
        b"100644" => Ok(RepositoryMode::Regular),
        b"100755" => Ok(RepositoryMode::Executable),
        b"120000" => Ok(RepositoryMode::SymbolicLink),
        b"160000" => Ok(RepositoryMode::Gitlink),
        _ => Err(GitError::StatusMalformed),
    }
}
fn valid_oid(value: &[u8]) -> bool {
    value.len() == 40
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupPathSemantics {
    Unix,
    Windows,
}

fn current_lookup_path_semantics() -> LookupPathSemantics {
    #[cfg(windows)]
    {
        LookupPathSemantics::Windows
    }
    #[cfg(not(windows))]
    {
        LookupPathSemantics::Unix
    }
}

/// Validates the lexical spelling before a repository-controlled path reaches
/// either the host filesystem or a Git path argument. Git tree paths always
/// use forward slashes; backslashes are retained on Unix as literal filename
/// bytes but are never safe lookup bytes on Windows.
fn validate_inventory_lookup_path(
    text: &str,
    semantics: LookupPathSemantics,
) -> Result<(), GitError> {
    if text.is_empty()
        || text.len() > MAX_REPOSITORY_RELATIVE_PATH_BYTES
        || text.contains('\0')
        || text.starts_with('/')
        || text.starts_with('\\')
        || (text.len() >= 2
            && text.as_bytes()[0].is_ascii_alphabetic()
            && text.as_bytes()[1] == b':')
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(GitError::StatusMalformed);
    }
    if semantics == LookupPathSemantics::Windows && text.contains('\\') {
        return Err(GitError::StatusMalformed);
    }
    Ok(())
}

fn parse_repository_relative_path(value: &[u8]) -> Result<RepositoryRelativePath, GitError> {
    if value.is_empty() || value.len() > MAX_REPOSITORY_RELATIVE_PATH_BYTES {
        return Err(GitError::StatusMalformed);
    }
    let text = std::str::from_utf8(value).map_err(|_| GitError::StatusMalformed)?;
    validate_inventory_lookup_path(text, current_lookup_path_semantics())?;
    Ok(RepositoryRelativePath(text.to_owned()))
}

/// `git ls-files -t -z` emits one `<tag><space><path>` NUL record per index
/// entry. `S` is the documented skip-worktree tag. Any unfamiliar framing or
/// tag is unsafe because a sparse index cannot be represented by AH2 yet.
fn has_skip_worktree(bytes: &[u8]) -> Result<bool, GitError> {
    if bytes.is_empty() {
        return Ok(false);
    }
    if !bytes.ends_with(&[0]) {
        return Err(GitError::StatusMalformed);
    }
    let mut seen = HashSet::new();
    let mut skip = false;
    for record in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if record.len() < 3 || record[1] != b' ' {
            return Err(GitError::StatusMalformed);
        }
        let tag = record[0];
        if !matches!(tag, b'H' | b'S' | b'M' | b'R' | b'C' | b'K') {
            return Err(GitError::StatusMalformed);
        }
        let path = parse_repository_relative_path(&record[2..])?;
        if !seen.insert(path.0) || seen.len() > MAX_STATUS_RECORDS {
            return Err(GitError::StatusTooManyRecords);
        }
        skip |= tag == b'S';
    }
    Ok(skip)
}

/// Inspects exactly the configured entry without following symbolic links or
/// Windows reparse points.  Metadata failures deliberately remain distinct
/// from absence.
pub fn inspect_worktree_destination_no_follow(path: &Path) -> WorktreeDestinationState {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return WorktreeDestinationState::Missing;
        }
        Err(_) => return WorktreeDestinationState::MetadataUnavailable,
    };
    if metadata.file_type().is_symlink() {
        return WorktreeDestinationState::UnsafeLinkOrReparse;
    }
    #[cfg(windows)]
    if std::os::windows::fs::MetadataExt::file_attributes(&metadata) & 0x400 != 0 {
        return WorktreeDestinationState::UnsafeLinkOrReparse;
    }
    if metadata.is_dir() {
        WorktreeDestinationState::RealDirectory
    } else {
        WorktreeDestinationState::NonDirectory
    }
}

fn normalized_worktree_path(path: &Path) -> Result<ValidatedWorktreePath, GitError> {
    if !path.is_absolute() {
        return Err(GitError::MetadataInvalid);
    }

    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        let text = path.to_str().ok_or(GitError::MetadataInvalid)?;
        let separator_normalized = text.replace('\\', "/");
        let segments: Vec<_> = separator_normalized.split('/').collect();
        let ordinary_segments: &[&str] = if separator_normalized.starts_with("//") {
            if segments.len() < 4 || segments[2].is_empty() || segments[3].is_empty() {
                return Err(GitError::MetadataInvalid);
            }
            &segments[2..]
        } else {
            if segments.len() < 2 || segments[0].len() != 2 || !segments[0].ends_with(':') {
                return Err(GitError::MetadataInvalid);
            }
            &segments[1..]
        };
        if !(ordinary_segments.len() == 1 && ordinary_segments[0].is_empty())
            && ordinary_segments
                .iter()
                .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        {
            return Err(GitError::MetadataInvalid);
        }

        let mut components = path.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix)) => prefix,
            _ => return Err(GitError::MetadataInvalid),
        };
        match prefix.kind() {
            Prefix::Disk(_) | Prefix::UNC(_, _) => {}
            Prefix::Verbatim(_)
            | Prefix::VerbatimDisk(_)
            | Prefix::VerbatimUNC(_, _)
            | Prefix::DeviceNS(_) => {
                return Err(GitError::MetadataInvalid);
            }
        }
        if !matches!(components.next(), Some(Component::RootDir)) {
            return Err(GitError::MetadataInvalid);
        }
        let mut normalized = PathBuf::from(prefix.as_os_str());
        normalized.push(Path::new("\\"));
        for component in components {
            match component {
                Component::Normal(value) => normalized.push(value),
                Component::CurDir
                | Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_) => {
                    return Err(GitError::MetadataInvalid);
                }
            }
        }
        let comparison_key = normalized
            .to_str()
            .ok_or(GitError::MetadataInvalid)?
            .to_lowercase();
        return Ok(ValidatedWorktreePath {
            path: normalized,
            comparison_key,
        });
    }

    #[cfg(not(windows))]
    {
        use std::path::Component;

        let text = path.to_str().ok_or(GitError::MetadataInvalid)?;
        if text != "/"
            && (text
                .strip_prefix('/')
                .ok_or(GitError::MetadataInvalid)?
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == ".."))
        {
            return Err(GitError::MetadataInvalid);
        }
        let mut normalized = PathBuf::from(Path::new("/"));
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(value) => normalized.push(value),
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return Err(GitError::MetadataInvalid);
                }
            }
        }
        let comparison_key = normalized
            .to_str()
            .ok_or(GitError::MetadataInvalid)?
            .to_owned();
        Ok(ValidatedWorktreePath {
            path: normalized,
            comparison_key,
        })
    }
}

fn expected_worktree_path(destination: &Path) -> Result<ValidatedWorktreePath, GitError> {
    expected_worktree_path_with(destination, |path| path.canonicalize())
}

fn expected_worktree_path_with<F>(
    destination: &Path,
    canonicalize: F,
) -> Result<ValidatedWorktreePath, GitError>
where
    F: FnOnce(&Path) -> std::io::Result<PathBuf>,
{
    match inspect_worktree_destination_no_follow(destination) {
        WorktreeDestinationState::RealDirectory => {
            let canonical = canonicalize(destination).map_err(GitError::Io)?;
            normalized_worktree_path(&canonical)
        }
        // A missing configured leaf has no canonical target. Its
        // backend-provided spelling is still required to be absolute and
        // lexically normalized before it can be compared with Git metadata.
        WorktreeDestinationState::Missing => normalized_worktree_path(destination),
        WorktreeDestinationState::UnsafeLinkOrReparse
        | WorktreeDestinationState::NonDirectory
        | WorktreeDestinationState::MetadataUnavailable => Err(GitError::MetadataInvalid),
    }
}

/// Returns an exact metadata lookup result after fully parsing porcelain-v1
/// NUL records.  Command and parser failures are errors, never `Absent`.
pub async fn worktree_metadata_lookup(
    repository: &Path,
    destination: &Path,
) -> Result<WorktreeMetadataLookup, GitError> {
    let inspection = inspect_repository(repository).await?;
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git(
        &executable,
        &inspection.repository_root,
        ["worktree", "list", "--porcelain", "-z"],
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::MetadataInvalid);
    }
    let records = parse_worktree_porcelain_v1_z(&output.stdout)?;
    let expected = expected_worktree_path(destination)?;
    let matches = records
        .iter()
        .filter(|record| record.path.comparison_key() == expected.comparison_key())
        .count();
    match matches {
        0 => Ok(WorktreeMetadataLookup::Absent),
        1 => Ok(WorktreeMetadataLookup::Present),
        _ => Err(GitError::MetadataInvalid),
    }
}

/// Strict parser for Git's documented porcelain-v1 `worktree list -z` form.
/// Each record is terminated by an empty NUL field. Unknown fields are
/// rejected rather than ignored, so a newer or damaged format cannot make an
/// existing worktree appear absent.
pub fn parse_worktree_porcelain_v1_z(
    bytes: &[u8],
) -> Result<Vec<WorktreeMetadataRecord>, GitError> {
    if bytes.is_empty() || !bytes.ends_with(&[0, 0]) {
        return Err(GitError::MetadataInvalid);
    }
    let mut records = Vec::new();
    let mut fields: Vec<&[u8]> = Vec::new();
    // Discard only the framing NUL; the preceding NUL remains the explicit
    // terminator for the final record.
    for field in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if field.is_empty() {
            if fields.is_empty() {
                return Err(GitError::MetadataInvalid);
            }
            records.push(parse_worktree_record(&fields)?);
            fields.clear();
        } else {
            fields.push(field);
        }
    }
    if !fields.is_empty() || records.is_empty() {
        return Err(GitError::MetadataInvalid);
    }
    let mut comparison_keys = std::collections::HashSet::new();
    for record in &records {
        if !comparison_keys.insert(record.path.comparison_key().to_owned()) {
            return Err(GitError::MetadataInvalid);
        }
    }
    Ok(records)
}

fn parse_worktree_record(fields: &[&[u8]]) -> Result<WorktreeMetadataRecord, GitError> {
    let mut path = None;
    let mut head = None;
    let mut branch = None;
    let mut detached = false;
    let mut bare = false;
    let mut locked = None;
    let mut prunable = None;
    for field in fields {
        let field = std::str::from_utf8(field).map_err(|_| GitError::MetadataInvalid)?;
        if let Some(value) = field.strip_prefix("worktree ") {
            if value.is_empty()
                || path
                    .replace(normalized_worktree_path(Path::new(value))?)
                    .is_some()
            {
                return Err(GitError::MetadataInvalid);
            }
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            if value.len() != 40
                || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                || head.replace(value.to_owned()).is_some()
            {
                return Err(GitError::MetadataInvalid);
            }
        } else if let Some(value) = field.strip_prefix("branch ") {
            if value.is_empty() || branch.replace(value.to_owned()).is_some() {
                return Err(GitError::MetadataInvalid);
            }
        } else if field == "detached" {
            if detached {
                return Err(GitError::MetadataInvalid);
            }
            detached = true;
        } else if field == "bare" {
            if bare {
                return Err(GitError::MetadataInvalid);
            }
            bare = true;
        } else if field == "locked" || field.starts_with("locked ") {
            if locked
                .replace(field.strip_prefix("locked ").unwrap_or("").to_owned())
                .is_some()
            {
                return Err(GitError::MetadataInvalid);
            }
        } else if field == "prunable" || field.starts_with("prunable ") {
            if prunable
                .replace(field.strip_prefix("prunable ").unwrap_or("").to_owned())
                .is_some()
            {
                return Err(GitError::MetadataInvalid);
            }
        } else {
            return Err(GitError::MetadataInvalid);
        }
    }
    if path.is_none()
        || (branch.is_some() && detached)
        || (bare && (head.is_some() || branch.is_some() || detached))
    {
        return Err(GitError::MetadataInvalid);
    }
    Ok(WorktreeMetadataRecord {
        path: path.unwrap(),
        head,
        branch,
        detached,
        bare,
        locked,
        prunable,
    })
}

/// Removes only a clean, linked worktree. The caller supplies a path loaded
/// from trusted persistence, never frontend input.
pub async fn remove_detached_worktree(
    repository: &Path,
    destination: &Path,
) -> Result<(), GitError> {
    if !worktree_is_clean(destination).await? {
        return Err(GitError::DirtyWorktree(destination.to_path_buf()));
    }
    let inspection = inspect_repository(repository).await?;
    let executable = TrustedGitExecutableResolver.resolve()?;
    let destination = destination.to_str().ok_or(GitError::MetadataInvalid)?;
    let output = run_git(
        &executable,
        &inspection.repository_root,
        ["worktree", "remove", destination],
    )
    .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GitError::MetadataInvalid)
    }
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
    run_git_with_limits(
        executable,
        directory,
        args,
        timeout,
        MAX_GIT_OUTPUT,
        MAX_GIT_OUTPUT,
        false,
        false,
    )
    .await
}

#[cfg(test)]
async fn run_git_with_timeout_observed<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
    timeout: std::time::Duration,
    child_observer: std::sync::mpsc::Sender<u32>,
) -> Result<GitCommandOutput, GitError> {
    run_git_with_limits_observer(
        executable,
        directory,
        args,
        timeout,
        MAX_GIT_OUTPUT,
        MAX_GIT_OUTPUT,
        false,
        false,
        Some(child_observer),
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::unit_arg)]
async fn run_git_with_limits<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
    timeout: std::time::Duration,
    stdout_limit: usize,
    stderr_limit: usize,
    read_only: bool,
    literal_pathspecs: bool,
) -> Result<GitCommandOutput, GitError> {
    run_git_with_limits_observer(
        executable,
        directory,
        args,
        timeout,
        stdout_limit,
        stderr_limit,
        read_only,
        literal_pathspecs,
        no_child_spawn_observer(),
    )
    .await
}

#[cfg(test)]
type ChildSpawnObserver = Option<std::sync::mpsc::Sender<u32>>;

#[cfg(not(test))]
type ChildSpawnObserver = ();

#[cfg(test)]
fn no_child_spawn_observer() -> ChildSpawnObserver {
    None
}

#[cfg(not(test))]
fn no_child_spawn_observer() -> ChildSpawnObserver {}

#[cfg(test)]
fn observe_spawned_child(observer: &ChildSpawnObserver, child: &tokio::process::Child) {
    if let (Some(sender), Some(pid)) = (observer.as_ref(), child.id()) {
        let _ = sender.send(pid);
    }
}

#[cfg(not(test))]
fn observe_spawned_child(_: &ChildSpawnObserver, _: &tokio::process::Child) {}

#[allow(clippy::too_many_arguments)]
async fn run_git_with_limits_observer<const N: usize>(
    executable: &ResolvedGitExecutable,
    directory: &Path,
    args: [&str; N],
    timeout: std::time::Duration,
    stdout_limit: usize,
    stderr_limit: usize,
    read_only: bool,
    literal_pathspecs: bool,
    child_observer: ChildSpawnObserver,
) -> Result<GitCommandOutput, GitError> {
    let timeout = cap_timeout_to_inventory_deadline(timeout)?;
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
    if INVENTORY_DEADLINE.try_with(|_| ()).is_ok() {
        // This trusted directory is supplied by the desktop managed-leaf
        // validator. GIT_WORK_TREE overrides any repository-local
        // core.worktree and pins every authoritative Git path lookup to it.
        command.env("GIT_WORK_TREE", directory);
    }
    if let Ok((home, global_config)) = INVENTORY_GIT_ENVIRONMENT.try_with(|environment| {
        (
            environment.home().to_path_buf(),
            environment.global_config.clone(),
        )
    }) {
        // GIT_CONFIG_GLOBAL wins over HOME/XDG discovery. HOME and XDG are
        // also redirected into the same trusted empty directory so no account
        // configuration can become a fallback source.
        command
            .env("GIT_CONFIG_GLOBAL", global_config)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home);
    }
    if read_only {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    }
    let mut child = command.spawn().map_err(|_| GitError::GitDiscoveryFailed)?;
    observe_spawned_child(&child_observer, &child);
    let stdout = child.stdout.take().ok_or(GitError::MetadataInvalid)?;
    let stderr = child.stderr.take().ok_or(GitError::MetadataInvalid)?;
    let collected = time::timeout(timeout, async {
        let streams = tokio::try_join!(
            read_capped(stdout, GitError::StdoutTooLarge, stdout_limit),
            read_capped(stderr, GitError::StderrTooLarge, stderr_limit)
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
    limit: usize,
) -> Result<Vec<u8>, GitError> {
    let mut bytes = Vec::with_capacity(limit.min(1024));
    let mut buffer = [0_u8; 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| GitError::MetadataInvalid)?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > limit {
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
    let records = parse_worktree_porcelain_v1_z(&output.stdout)?;
    let primary = records.first().ok_or(GitError::MetadataInvalid)?;
    primary
        .path
        .path()
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
    let _ = worktree;
    Err(GitError::InventoryUnavailable)
}

pub fn remove_clean_worktree(repository: &Path, worktree: &Worktree) -> Result<(), GitError> {
    let _ = (repository, worktree);
    Err(GitError::CleanlinessUnavailable)
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

#[cfg(test)]
mod status_inventory_tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";
    fn ordinary(xy: &str, path: &str) -> Vec<u8> {
        format!("1 {xy} N... 100644 100644 100644 {OID} {OID} {path}\0").into_bytes()
    }

    #[test]
    fn status_inventory_accepts_clean_and_separate_staged_unstaged_changes() {
        assert!(parse_status_porcelain_v2_z(b"").unwrap().is_empty());
        let files = parse_status_porcelain_v2_z(&ordinary("MM", "src/one file.rs")).unwrap();
        assert_eq!(files[0].index_change, Some(ChangeKind::Modified));
        assert_eq!(files[0].worktree_change, Some(ChangeKind::Modified));
        assert_eq!(files[0].path.as_str(), "src/one file.rs");
    }

    #[test]
    fn status_inventory_supports_untracked_unicode_newline_and_gitlink_metadata() {
        let mut bytes = "? - ünicode\nname\0".to_owned().into_bytes();
        bytes.extend_from_slice(
            format!("1 M. SC.. 160000 160000 160000 {OID} {OID} module\0").as_bytes(),
        );
        let files = parse_status_porcelain_v2_z(&bytes).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .any(|file| file.untracked && file.path.as_str() == "- ünicode\nname"));
        assert!(files
            .iter()
            .any(|file| file.mode_head == Some(RepositoryMode::Gitlink)));
    }

    #[test]
    fn status_inventory_supports_all_documented_unmerged_codes() {
        for code in ["DD", "AU", "UD", "UA", "DU", "AA", "UU"] {
            let bytes =
                format!("u {code} N... 100644 100644 100644 100644 {OID} {OID} {OID} conflict\0");
            let files = parse_status_porcelain_v2_z(bytes.as_bytes()).unwrap();
            assert!(files[0].conflict.is_some());
        }
    }

    #[test]
    fn status_inventory_rejects_damage_aliases_and_ambiguous_records() {
        for bytes in [
            b"? ../escape\0".as_slice(),
            b"? /absolute\0".as_slice(),
            b"? C:/drive\0".as_slice(),
            b"? C:\\drive\0".as_slice(),
            b"! ignored\0".as_slice(),
            b"2 R. N... 100644 100644 100644 100644 100644 ".as_slice(),
            b"? truncated".as_slice(),
            b"? duplicate\0? duplicate\0".as_slice(),
        ] {
            assert!(parse_status_porcelain_v2_z(bytes).is_err());
        }
        let invalid = format!("1 ZZ N... 100644 100644 100644 {OID} {OID} bad\0");
        assert!(parse_status_porcelain_v2_z(invalid.as_bytes()).is_err());
    }

    #[test]
    fn status_inventory_rejects_rooted_windows_spellings_but_preserves_internal_backslashes() {
        for path in [
            "\\Windows\\file",
            "\\rooted",
            "\\device\\name",
            "\\??\\C:\\file",
            "\\\\server\\share",
            "\\\\?\\C:\\file",
            "\\\\.\\PhysicalDrive0",
        ] {
            assert!(parse_repository_relative_path(path.as_bytes()).is_err());
            let record = format!("? {path}\0");
            assert!(parse_status_porcelain_v2_z(record.as_bytes()).is_err());
        }
        let files = parse_status_porcelain_v2_z(b"? folder\\name.txt\0").unwrap();
        assert_eq!(files[0].path.as_str(), "folder\\name.txt");
    }

    #[test]
    fn status_inventory_rejects_clean_ordinary_records_without_returning_partial_files() {
        let clean_record = ordinary("..", "not-a-change");
        assert!(parse_status_porcelain_v2_z(&clean_record).is_err());

        let mut mixed = ordinary("M.", "valid-change");
        mixed.extend_from_slice(&clean_record);
        assert!(parse_status_porcelain_v2_z(&mixed).is_err());
        assert!(parse_status_porcelain_v2_z(b"").unwrap().is_empty());
    }

    #[test]
    fn status_inventory_rejects_oversized_and_invalid_utf8_paths() {
        let large = format!("? {}\0", "x".repeat(MAX_REPOSITORY_RELATIVE_PATH_BYTES + 1));
        assert!(parse_status_porcelain_v2_z(large.as_bytes()).is_err());
        assert!(parse_status_porcelain_v2_z(&[b'?', b' ', 0xff, 0]).is_err());
    }
}

#[cfg(test)]
mod numstat_tests {
    use super::*;

    #[test]
    fn staged_raw_parser_is_strict_and_preserves_literal_paths() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let bytes = format!(":000000 100644 {oid} {oid} A\0:(glob)file\nname\0");
        let changes = parse_staged_raw_z(bytes.as_bytes()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, ChangeKind::Added);
        assert_eq!(changes[0].path.as_str(), ":(glob)file\nname");
        assert_eq!(changes[0].old_mode, None);
        assert_eq!(changes[0].new_mode, Some(RepositoryMode::Regular));
        for malformed in [
            b":100644 100644 short short M\0file\0".as_slice(),
            b":100644 100644 0123456789abcdef0123456789abcdef01234567 0123456789abcdef0123456789abcdef01234567 R\0file\0".as_slice(),
            b":100644 100644 0123456789abcdef0123456789abcdef01234567 0123456789abcdef0123456789abcdef01234567 M\0file".as_slice(),
        ] {
            assert!(parse_staged_raw_z(malformed).is_err());
        }
    }

    #[test]
    fn filter_attribute_parser_is_strict_and_never_returns_driver_details() {
        assert_eq!(
            parse_filter_attribute_z(b"file.txt\0filter\0unspecified\0", "file.txt").unwrap(),
            FilterAttributeState::SafeUnspecified
        );
        assert_eq!(
            parse_filter_attribute_z(b"file.txt\0filter\0unset\0", "file.txt").unwrap(),
            FilterAttributeState::SafeUnset
        );
        assert_eq!(
            parse_filter_attribute_z(b"file.txt\0filter\0sentinel-marker\0", "file.txt").unwrap(),
            FilterAttributeState::Deferred
        );
        for bytes in [
            b"file.txt\0filter\0".as_slice(),
            b"file.txt\0filter\0\0".as_slice(),
            b"other.txt\0filter\0unspecified\0".as_slice(),
            b"file.txt\0other\0unspecified\0".as_slice(),
            b"file.txt\0filter\0unspecified\0extra\0".as_slice(),
            b"file.txt\0filter\0\xff\0".as_slice(),
        ] {
            assert!(parse_filter_attribute_z(bytes, "file.txt").is_err());
        }
    }

    #[test]
    fn numstat_parser_accepts_one_text_or_binary_record_with_literal_paths() {
        let path = " :(glob)*.txt\nname";
        let text = format!("12\t3\t{path}\0");
        assert_eq!(
            parse_numstat_z(text.as_bytes(), path).unwrap(),
            NumstatClassification::TextEligible(TextEligibleMetadata {
                additions: 12,
                deletions: 3,
            })
        );
        let binary = b"-\t-\tfolder\\name.bin\0";
        assert_eq!(
            parse_numstat_z(binary, "folder\\name.bin").unwrap(),
            NumstatClassification::Binary
        );
        assert_eq!(
            parse_numstat_z(b"", "file.txt").unwrap(),
            NumstatClassification::NoChanges
        );
    }

    #[test]
    fn numstat_parser_rejects_malformed_or_non_single_output_without_partial_results() {
        for bytes in [
            b"1\t2\tfile.txt".as_slice(),
            b"1\t2\t\0".as_slice(),
            b"-\t2\tfile.txt\0".as_slice(),
            b"+1\t2\tfile.txt\0".as_slice(),
            b"1\t2\t../escape\0".as_slice(),
            b"1\t2\tfile.txt\x001\t2\tother.txt\0".as_slice(),
        ] {
            assert!(parse_numstat_z(bytes, "file.txt").is_err());
        }
        assert!(parse_numstat_z(b"1\t2\tother.txt\0", "file.txt").is_err());
    }
}

#[cfg(all(test, unix))]
mod inspection_tests {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io::{BufRead, BufReader, Read, Write},
        os::unix::{fs::PermissionsExt, process::CommandExt},
        sync::{mpsc, Arc, Mutex},
        thread,
    };

    fn fixture_git(directory: &Path, args: &[&str]) {
        let mut command = Command::new("/usr/bin/git");
        command.args(args).current_dir(directory);
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ASKPASS",
            "PAGER",
        ] {
            command.env_remove(key);
        }
        let output = command.output().expect("fixture git process");
        assert!(
            output.status.success(),
            "fixture git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

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

    fn fixture_python() -> PathBuf {
        let output = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .expect("test-only Python resolver");
        assert!(output.status.success(), "test-only Python resolver status");
        let path = std::str::from_utf8(&output.stdout)
            .expect("test-only Python resolver UTF-8")
            .trim();
        fs::canonicalize(path).expect("canonical test-only Python executable")
    }

    async fn spawn_group_leader() -> std::process::Child {
        Command::new(fixture_python())
            .args(["-c", "import os; os.setpgrp();\nwhile True: pass"])
            .spawn()
            .expect("disposable group leader")
    }

    async fn wait_for_group_leader(pid: u32) -> bool {
        time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if process_group_of(pid) == Some(pid) && process_is_alive(pid) {
                    return true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    fn terminate_disposable(child: &mut std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    const MAX_SUPERVISOR_EVENT_FRAME_BYTES: usize = 96;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FixtureEvent {
        GroupReady { direct_pid: u32, pgid: u32 },
        DescendantLaunchStarted,
        DescendantLaunched { pid: u32 },
        TermIgnoreReady { pid: u32 },
        ReadyPublished,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SupervisorProtocolError {
        FrameDeadlineExpired,
        FrameTooLarge,
        UnterminatedFrame,
        InvalidUtf8,
        InvalidNumericField,
        UnknownEvent,
        InvalidFieldCount,
        TrailingData,
        UnexpectedWhitespace,
        UnexpectedEof,
        InvalidSequence,
        DuplicateEvent,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum GroupProbe {
        Alive,
        Absent,
        PermissionDenied,
        Failed(i32),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum GroupSignal {
        Sent,
        Absent,
        PermissionDenied,
        Failed(i32),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct SignalAttempt {
        pgid: u32,
        signal: libc::c_int,
        outcome: GroupSignal,
    }

    /// Unix-only, test-only containment boundary for hostile fixture modes.
    /// This deliberately does not change the production trusted runner: tests
    /// place their disposable direct helper in a dedicated process group and
    /// own that group before allowing it to launch a descendant.
    struct FixtureProcessSupervisor {
        direct_pid: Option<u32>,
        pgid: Option<u32>,
        descendant: Option<std::process::Child>,
        extra_descendants: Vec<std::process::Child>,
        descendant_pid: Option<u32>,
        descendant_ignores_term: bool,
        gate_path: PathBuf,
        event_writer: File,
        events: Arc<Mutex<mpsc::Receiver<Result<FixtureEvent, SupervisorProtocolError>>>>,
        observed: Vec<FixtureEvent>,
        reader: Option<thread::JoinHandle<()>>,
        signals: Arc<Mutex<Vec<SignalAttempt>>>,
        post_sigterm_probe: Option<GroupProbe>,
        protocol_error: Option<SupervisorProtocolError>,
    }

    impl FixtureProcessSupervisor {
        fn new(fixture: &Path) -> Result<Self, ()> {
            let event_path = fixture.join("supervisor.events");
            let event_c = std::ffi::CString::new(event_path.as_os_str().as_encoded_bytes())
                .map_err(|_| ())?;
            // SAFETY: fixture is a unique temporary directory, the pathname is
            // NUL-free, and this test-only FIFO is never production-reachable.
            if unsafe { libc::mkfifo(event_c.as_ptr(), 0o600) } != 0 {
                return Err(());
            }
            // O_RDWR keeps the reader available before the helper opens its
            // write end. The reader exits on the explicit terminal marker.
            let event_writer = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&event_path)
                .map_err(|_| ())?;
            let reader_file = event_writer.try_clone().map_err(|_| ())?;
            let (sender, receiver) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut reader = BufReader::new(reader_file);
                loop {
                    let mut frame = Vec::new();
                    let result = reader
                        .by_ref()
                        .take((MAX_SUPERVISOR_EVENT_FRAME_BYTES + 1) as u64)
                        .read_until(b'\n', &mut frame)
                        .map_err(|_| SupervisorProtocolError::UnexpectedEof)
                        .and_then(|count| {
                            if count == 0 {
                                Err(SupervisorProtocolError::UnexpectedEof)
                            } else if frame.len() > MAX_SUPERVISOR_EVENT_FRAME_BYTES {
                                Err(SupervisorProtocolError::FrameTooLarge)
                            } else {
                                parse_fixture_event(&frame)
                            }
                        });
                    match result {
                        Ok(None) => return,
                        Ok(Some(event)) => {
                            if sender.send(Ok(event)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                    }
                }
            });
            Ok(Self {
                direct_pid: None,
                pgid: None,
                descendant: None,
                extra_descendants: Vec::new(),
                descendant_pid: None,
                descendant_ignores_term: false,
                gate_path: fixture.join("launch.descendant"),
                event_writer,
                events: Arc::new(Mutex::new(receiver)),
                observed: Vec::new(),
                reader: Some(reader),
                signals: Arc::new(Mutex::new(Vec::new())),
                post_sigterm_probe: None,
                protocol_error: None,
            })
        }

        async fn next_event(&mut self) -> Result<FixtureEvent, SupervisorProtocolError> {
            let received = time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let event = self
                        .events
                        .lock()
                        .map_err(|_| SupervisorProtocolError::UnexpectedEof)?
                        .try_recv();
                    match event {
                        Ok(Ok(event)) => {
                            self.validate_event_sequence(event)?;
                            self.observed.push(event);
                            return Ok(event);
                        }
                        Ok(Err(error)) => {
                            self.protocol_error = Some(error);
                            return Err(error);
                        }
                        Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err(SupervisorProtocolError::UnexpectedEof)
                        }
                    }
                }
            })
            .await;
            match received {
                Ok(result) => result,
                Err(_) => {
                    self.protocol_error = Some(SupervisorProtocolError::FrameDeadlineExpired);
                    Err(SupervisorProtocolError::FrameDeadlineExpired)
                }
            }
        }

        fn validate_event_sequence(
            &mut self,
            event: FixtureEvent,
        ) -> Result<(), SupervisorProtocolError> {
            let seen_group = self
                .observed
                .iter()
                .any(|event| matches!(event, FixtureEvent::GroupReady { .. }));
            let started = self
                .observed
                .iter()
                .filter(|event| matches!(event, FixtureEvent::DescendantLaunchStarted))
                .count();
            let launched = self
                .observed
                .iter()
                .filter(|event| matches!(event, FixtureEvent::DescendantLaunched { .. }))
                .count();
            let ready = self
                .observed
                .iter()
                .any(|event| matches!(event, FixtureEvent::ReadyPublished));
            let term_ignore = self
                .observed
                .iter()
                .filter(|event| matches!(event, FixtureEvent::TermIgnoreReady { .. }))
                .count();
            match event {
                FixtureEvent::GroupReady { .. } if seen_group => {
                    Err(SupervisorProtocolError::DuplicateEvent)
                }
                FixtureEvent::GroupReady { .. } if started != 0 || launched != 0 || ready => {
                    Err(SupervisorProtocolError::InvalidSequence)
                }
                FixtureEvent::DescendantLaunchStarted
                    if !seen_group || started != 0 || launched != 0 =>
                {
                    Err(SupervisorProtocolError::InvalidSequence)
                }
                FixtureEvent::DescendantLaunched { .. } if started != 1 || launched != 0 => {
                    Err(SupervisorProtocolError::InvalidSequence)
                }
                FixtureEvent::ReadyPublished if launched != 1 || ready => {
                    Err(SupervisorProtocolError::InvalidSequence)
                }
                FixtureEvent::TermIgnoreReady { .. }
                    if launched != 1 || term_ignore != 0 || ready =>
                {
                    Err(SupervisorProtocolError::InvalidSequence)
                }
                _ => Ok(()),
            }
        }

        fn descendant_launch_count(&self) -> usize {
            self.observed
                .iter()
                .filter(|event| matches!(event, FixtureEvent::DescendantLaunched { .. }))
                .count()
        }

        fn verify_descendant_membership(&self, pid: u32) -> Result<(), ()> {
            let (Some(direct), Some(pgid), Some(owned)) =
                (self.direct_pid, self.pgid, self.descendant_pid)
            else {
                return Err(());
            };
            if pid == 0 || pid == direct || pid != owned || !process_is_alive(pid) {
                return Err(());
            }
            (process_group_of(pid) == Some(pgid))
                .then_some(())
                .ok_or(())
        }

        fn launch_owned_descendant(&mut self, term_ignore: bool) -> Result<u32, ()> {
            let Some(pgid) = self.pgid else {
                return Err(());
            };
            self.emit_event("DESCENDANT_LAUNCH_STARTED")?;
            let events = self.event_path()?;
            let mut command = Command::new(fixture_python());
            if term_ignore {
                command.args([
                    "-c",
                    "import os,signal,sys; signal.signal(signal.SIGTERM, signal.SIG_IGN); f=open(sys.argv[1], 'w'); f.write(f'TERM_IGNORE_READY {os.getpid()}\\n'); f.flush(); exec('while True: pass')",
                    events.to_str().ok_or(())?,
                ]);
            } else {
                command.args(["-c", "while True: pass"]);
            }
            // SAFETY: test-only child joins the already verified disposable
            // fixture process group before it begins its loop.
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, pgid as libc::pid_t) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
            let child = command.spawn().map_err(|_| ())?;
            let pid = child.id();
            self.descendant_pid = Some(pid);
            self.descendant = Some(child);
            self.descendant_ignores_term = term_ignore;
            self.verify_descendant_membership(pid)?;
            self.emit_event(&format!("DESCENDANT_LAUNCHED {pid}"))?;
            Ok(pid)
        }

        fn launch_same_group_decoy(&mut self) -> Result<u32, ()> {
            let Some(pgid) = self.pgid else {
                return Err(());
            };
            let mut command = Command::new(fixture_python());
            command.args(["-c", "while True: pass"]);
            // SAFETY: this disposable test child joins the already verified
            // fixture group solely to exercise wrong-same-group PID rejection.
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, pgid as libc::pid_t) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
            let child = command.spawn().map_err(|_| ())?;
            let pid = child.id();
            self.extra_descendants.push(child);
            Ok(pid)
        }

        fn event_path(&self) -> Result<PathBuf, ()> {
            // The FIFO is fixture-local and `event_writer` was opened from it.
            // Store its stable path via the launch gate's fixture directory.
            self.gate_path
                .parent()
                .map(|parent| parent.join("supervisor.events"))
                .ok_or(())
        }

        fn emit_event(&mut self, event: &str) -> Result<(), ()> {
            self.event_writer
                .write_all(event.as_bytes())
                .map_err(|_| ())?;
            self.event_writer.write_all(b"\n").map_err(|_| ())?;
            self.event_writer.flush().map_err(|_| ())
        }

        fn arm_group(
            &mut self,
            trusted_direct_pid: u32,
            event_direct_pid: u32,
            event_pgid: u32,
        ) -> Result<(), ()> {
            if trusted_direct_pid == 0
                || trusted_direct_pid != event_direct_pid
                || event_pgid != trusted_direct_pid
                || self.pgid.is_some()
                || current_process_group() == event_pgid
            {
                return Err(());
            }
            // The direct PID comes from the trusted runner's Child::id(), not
            // from the fixture FIFO. The FIFO merely corroborates its group.
            if process_group_of(trusted_direct_pid) != Some(event_pgid)
                || !process_is_alive(trusted_direct_pid)
            {
                return Err(());
            }
            self.direct_pid = Some(trusted_direct_pid);
            self.pgid = Some(event_pgid);
            Ok(())
        }

        fn authorize_descendant_launch(&self) -> Result<(), ()> {
            if self.pgid.is_none() {
                return Err(());
            }
            release_descendant_launch_gate(&self.gate_path)
        }

        async fn cleanup_group(&mut self) -> Result<(), ()> {
            let Some(pgid) = self.pgid.take() else {
                return Ok(());
            };
            if !valid_fixture_group(pgid, self.direct_pid) {
                self.pgid = Some(pgid);
                return Err(());
            }
            match self.signal_group(pgid, libc::SIGTERM) {
                GroupSignal::Sent => {}
                GroupSignal::Absent => {
                    self.direct_pid = None;
                    return Ok(());
                }
                GroupSignal::PermissionDenied | GroupSignal::Failed(_) => {
                    self.pgid = Some(pgid);
                    return Err(());
                }
            }
            if !self.descendant_ignores_term {
                self.reap_owned_descendant();
            }
            match wait_for_group_exit(pgid).await {
                Ok(()) => {}
                Err(GroupProbe::Alive) => {
                    self.post_sigterm_probe = Some(GroupProbe::Alive);
                    match self.signal_group(pgid, libc::SIGKILL) {
                        GroupSignal::Sent | GroupSignal::Absent => {
                            self.reap_owned_descendant();
                            if wait_for_group_exit(pgid).await.is_err() {
                                self.pgid = Some(pgid);
                                return Err(());
                            }
                        }
                        GroupSignal::PermissionDenied | GroupSignal::Failed(_) => {
                            self.pgid = Some(pgid);
                            return Err(());
                        }
                    }
                }
                Err(_) => {
                    self.pgid = Some(pgid);
                    return Err(());
                }
            }
            self.reap_owned_descendant();
            self.direct_pid = None;
            Ok(())
        }

        fn reap_owned_descendant(&mut self) {
            if let Some(mut descendant) = self.descendant.take() {
                let _ = descendant.wait();
            }
            for mut descendant in self.extra_descendants.drain(..) {
                let _ = descendant.wait();
            }
            self.descendant_pid = None;
            self.descendant_ignores_term = false;
        }

        fn signal_group(&self, pgid: u32, signal: libc::c_int) -> GroupSignal {
            // SAFETY: valid_fixture_group protects against the harness group;
            // a negative target is POSIX process-group signalling.
            let outcome = classify_group_signal(pgid, signal);
            if let Ok(mut signals) = self.signals.lock() {
                signals.push(SignalAttempt {
                    pgid,
                    signal,
                    outcome,
                });
            }
            outcome
        }

        fn signal_sequence(&self) -> Vec<SignalAttempt> {
            self.signals
                .lock()
                .map(|signals| signals.clone())
                .unwrap_or_default()
        }

        fn stop_reader(&mut self) {
            let _ = self.event_writer.write_all(b"SUPERVISOR_STOP\n");
            let _ = self.event_writer.flush();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    impl Drop for FixtureProcessSupervisor {
        fn drop(&mut self) {
            if let Some(pgid) = self.pgid.take() {
                if valid_fixture_group(pgid, self.direct_pid) {
                    // SAFETY: panic-safe test-only best effort for the armed
                    // disposable fixture group only.
                    let _ = unsafe { libc::kill(-(pgid as libc::pid_t), libc::SIGKILL) };
                }
            }
            self.stop_reader();
        }
    }

    fn current_process_group() -> u32 {
        // SAFETY: getpgrp has no preconditions.
        unsafe { libc::getpgrp() as u32 }
    }

    fn process_group_of(pid: u32) -> Option<u32> {
        // SAFETY: getpgid accepts a process identifier and has no Rust aliasing
        // requirements. A negative return reports a non-existent process.
        let group = unsafe { libc::getpgid(pid as libc::pid_t) };
        (group > 0).then_some(group as u32)
    }

    fn valid_fixture_group(pgid: u32, direct: Option<u32>) -> bool {
        pgid != 0 && Some(pgid) == direct && pgid != current_process_group()
    }

    fn probe_process_group(pgid: u32) -> GroupProbe {
        // SAFETY: a negative PID targets exactly the requested process group.
        if unsafe { libc::kill(-(pgid as libc::pid_t), 0) } == 0 {
            GroupProbe::Alive
        } else {
            classify_group_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))
        }
    }

    fn classify_group_errno(errno: i32) -> GroupProbe {
        match errno {
            libc::ESRCH => GroupProbe::Absent,
            libc::EPERM => GroupProbe::PermissionDenied,
            _ => GroupProbe::Failed(errno),
        }
    }

    fn classify_group_signal(pgid: u32, signal: libc::c_int) -> GroupSignal {
        // SAFETY: a negative PID targets exactly the requested process group.
        if unsafe { libc::kill(-(pgid as libc::pid_t), signal) } == 0 {
            GroupSignal::Sent
        } else {
            classify_group_signal_errno(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
            )
        }
    }

    fn classify_group_signal_errno(errno: i32) -> GroupSignal {
        match errno {
            libc::ESRCH => GroupSignal::Absent,
            libc::EPERM => GroupSignal::PermissionDenied,
            _ => GroupSignal::Failed(errno),
        }
    }

    fn process_group_is_alive(pgid: u32) -> bool {
        matches!(probe_process_group(pgid), GroupProbe::Alive)
    }

    async fn wait_for_group_exit(pgid: u32) -> Result<(), GroupProbe> {
        let mut saw_permission_denied = false;
        let waited = time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match probe_process_group(pgid) {
                    GroupProbe::Absent => return Ok(()),
                    GroupProbe::Alive => tokio::task::yield_now().await,
                    // A zombie group leader can briefly yield EPERM while the
                    // owned runner is still reaping it. EPERM never proves
                    // absence: keep polling until ESRCH or the bounded
                    // deadline, then retain it as a cleanup failure.
                    GroupProbe::PermissionDenied => {
                        saw_permission_denied = true;
                        tokio::task::yield_now().await
                    }
                    error => return Err(error),
                }
            }
        })
        .await;
        match waited {
            Ok(result) => result,
            Err(_) if saw_permission_denied => Err(GroupProbe::PermissionDenied),
            Err(_) => Err(GroupProbe::Alive),
        }
    }

    fn parse_fixture_pid(value: &[u8]) -> Result<u32, SupervisorProtocolError> {
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(SupervisorProtocolError::InvalidNumericField);
        }
        let value = std::str::from_utf8(value).map_err(|_| SupervisorProtocolError::InvalidUtf8)?;
        let pid = value
            .parse::<u32>()
            .map_err(|_| SupervisorProtocolError::InvalidNumericField)?;
        (pid != 0)
            .then_some(pid)
            .ok_or(SupervisorProtocolError::InvalidNumericField)
    }

    fn parse_fixture_event(frame: &[u8]) -> Result<Option<FixtureEvent>, SupervisorProtocolError> {
        if frame.len() > MAX_SUPERVISOR_EVENT_FRAME_BYTES {
            return Err(SupervisorProtocolError::FrameTooLarge);
        }
        let Some(body) = frame.strip_suffix(b"\n") else {
            return Err(SupervisorProtocolError::UnterminatedFrame);
        };
        if body.contains(&b'\n') || body.contains(&b'\r') || body.contains(&0) {
            return Err(SupervisorProtocolError::TrailingData);
        }
        if body.ends_with(b" ") || body.ends_with(b"\t") {
            return Err(SupervisorProtocolError::UnexpectedWhitespace);
        }
        match body {
            b"DESCENDANT_LAUNCH_STARTED" => Ok(Some(FixtureEvent::DescendantLaunchStarted)),
            b"READY_PUBLISHED" => Ok(Some(FixtureEvent::ReadyPublished)),
            b"SUPERVISOR_STOP" => Ok(None),
            _ => {
                let fields: Vec<_> = body.split(|byte| *byte == b' ').collect();
                if fields.iter().any(|field| field.is_empty()) {
                    return Err(SupervisorProtocolError::UnexpectedWhitespace);
                }
                match fields.as_slice() {
                    [b"GROUP_READY", direct, pgid] => Ok(Some(FixtureEvent::GroupReady {
                        direct_pid: parse_fixture_pid(direct)?,
                        pgid: parse_fixture_pid(pgid)?,
                    })),
                    [b"DESCENDANT_LAUNCHED", pid] => Ok(Some(FixtureEvent::DescendantLaunched {
                        pid: parse_fixture_pid(pid)?,
                    })),
                    [b"TERM_IGNORE_READY", pid] => Ok(Some(FixtureEvent::TermIgnoreReady {
                        pid: parse_fixture_pid(pid)?,
                    })),
                    [b"GROUP_READY", ..]
                    | [b"DESCENDANT_LAUNCHED", ..]
                    | [b"TERM_IGNORE_READY", ..] => Err(SupervisorProtocolError::InvalidFieldCount),
                    _ if body.starts_with(b"GROUP_READY")
                        || body.starts_with(b"DESCENDANT_LAUNCHED") =>
                    {
                        Err(SupervisorProtocolError::TrailingData)
                    }
                    _ => Err(SupervisorProtocolError::UnknownEvent),
                }
            }
        }
    }

    fn recorded_pid(path: &Path) -> Result<Option<u32>, ()> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        let Some(value) = bytes.strip_suffix(b"\n") else {
            return Err(());
        };
        if value.is_empty() || !value.iter().all(|byte| byte.is_ascii_digit()) {
            return Err(());
        }
        let value = std::str::from_utf8(value)
            .map_err(|_| ())?
            .parse::<u32>()
            .map_err(|_| ())?;
        (value != 0).then_some(value).ok_or(()).map(Some)
    }

    fn recorded_cleanup_manifest(path: &Path) -> Result<Option<(u32, u32)>, ()> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        let Some(bytes) = bytes.strip_suffix(b"\n") else {
            return Err(());
        };
        let mut lines = bytes.split(|byte| *byte == b'\n');
        let Some(direct) = lines.next() else {
            return Err(());
        };
        let Some(descendant) = lines.next() else {
            return Err(());
        };
        if lines.next().is_some() {
            return Err(());
        }
        fn parse(value: &[u8]) -> Result<u32, ()> {
            if value.is_empty() || !value.iter().all(|byte| byte.is_ascii_digit()) {
                return Err(());
            }
            let pid = std::str::from_utf8(value)
                .map_err(|_| ())?
                .parse::<u32>()
                .map_err(|_| ())?;
            (pid != 0).then_some(pid).ok_or(())
        }
        let direct = parse(direct)?;
        let descendant = parse(descendant)?;
        (direct != descendant)
            .then_some((direct, descendant))
            .ok_or(())
            .map(Some)
    }

    fn process_is_alive(pid: u32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn signal_process(pid: u32, signal: &str) {
        let _ = Command::new("/bin/kill")
            .args([signal, &pid.to_string()])
            .output();
    }

    async fn wait_for_process_exit(pid: u32) -> bool {
        time::timeout(std::time::Duration::from_secs(1), async {
            while process_is_alive(pid) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    struct DescendantCleanup {
        direct_child_pid: Option<u32>,
        descendant_pid: Option<u32>,
        signals: Arc<Mutex<Vec<(u32, &'static str)>>>,
    }

    impl DescendantCleanup {
        fn new() -> Self {
            Self {
                direct_child_pid: None,
                descendant_pid: None,
                signals: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn arm_unique_pid(&mut self, pid: u32) -> Result<bool, ()> {
            if self.direct_child_pid == Some(pid) || self.descendant_pid == Some(pid) {
                return Ok(false);
            }
            if self.direct_child_pid.is_none() {
                self.direct_child_pid = Some(pid);
                return Ok(true);
            }
            if self.descendant_pid.is_none() {
                self.descendant_pid = Some(pid);
                return Ok(true);
            }
            Err(())
        }

        fn signal_recorder(&self) -> Arc<Mutex<Vec<(u32, &'static str)>>> {
            Arc::clone(&self.signals)
        }

        fn signal(&self, pid: u32, signal: &'static str) {
            if let Ok(mut signals) = self.signals.lock() {
                signals.push((pid, signal));
            }
            signal_process(pid, signal);
        }

        async fn terminate_and_verify(&self, pid: u32) -> bool {
            self.signal(pid, "-TERM");
            if wait_for_process_exit(pid).await {
                return true;
            }
            self.signal(pid, "-KILL");
            wait_for_process_exit(pid).await
        }

        async fn finish(&mut self) -> Result<(), ()> {
            let direct_child = self.direct_child_pid.take();
            let descendant = self.descendant_pid.take();
            let direct_clean = match direct_child {
                Some(pid) => self.terminate_and_verify(pid).await,
                None => true,
            };
            let descendant_clean = match descendant {
                Some(pid) => self.terminate_and_verify(pid).await,
                None => true,
            };
            if !direct_clean {
                self.direct_child_pid = direct_child;
            }
            if !descendant_clean {
                self.descendant_pid = descendant;
            }
            if direct_clean && descendant_clean {
                Ok(())
            } else {
                Err(())
            }
        }
    }

    impl Drop for DescendantCleanup {
        fn drop(&mut self) {
            for pid in [self.direct_child_pid.take(), self.descendant_pid.take()]
                .into_iter()
                .flatten()
            {
                // Panic-safe best effort. The normal async path above waits for
                // exit; this fallback prevents a test assertion from leaking a
                // recorded direct child or descendant.
                self.signal(pid, "-KILL");
            }
        }
    }

    enum BoundedJoinOutcome<T> {
        Joined(Result<T, tokio::task::JoinError>),
        DeadlineExpired,
    }

    async fn join_runner_with_deadline<T>(
        runner: &mut tokio::task::JoinHandle<T>,
        deadline: std::time::Duration,
    ) -> BoundedJoinOutcome<T> {
        match time::timeout(deadline, runner).await {
            Ok(result) => BoundedJoinOutcome::Joined(result),
            Err(_) => BoundedJoinOutcome::DeadlineExpired,
        }
    }

    async fn abort_cleanup_and_join<T>(
        runner: &mut tokio::task::JoinHandle<T>,
        cleanup: &mut DescendantCleanup,
    ) -> (Result<(), ()>, BoundedJoinOutcome<T>) {
        if !runner.is_finished() {
            runner.abort();
        }
        let first_join = join_runner_with_deadline(runner, std::time::Duration::from_secs(2)).await;
        let cleanup_result = cleanup.finish().await;
        let final_join = match first_join {
            BoundedJoinOutcome::DeadlineExpired => join_runner_until_terminal(runner).await,
            outcome => outcome,
        };
        (cleanup_result, final_join)
    }

    async fn abort_supervised_runner<T>(
        runner: &mut tokio::task::JoinHandle<T>,
        supervisor: &mut FixtureProcessSupervisor,
    ) -> (Result<(), ()>, BoundedJoinOutcome<T>) {
        let cleanup_result = supervisor.cleanup_group().await;
        if !runner.is_finished() {
            runner.abort();
        }
        let first_join = join_runner_with_deadline(runner, std::time::Duration::from_secs(2)).await;
        let final_join = match first_join {
            BoundedJoinOutcome::DeadlineExpired => join_runner_until_terminal(runner).await,
            outcome => outcome,
        };
        (cleanup_result, final_join)
    }

    async fn join_runner_until_terminal<T>(
        runner: &mut tokio::task::JoinHandle<T>,
    ) -> BoundedJoinOutcome<T> {
        loop {
            runner.abort();
            match join_runner_with_deadline(runner, std::time::Duration::from_secs(2)).await {
                BoundedJoinOutcome::Joined(result) => return BoundedJoinOutcome::Joined(result),
                // Each diagnostic wait is bounded. Retaining the handle until
                // it reaches a terminal state prevents task detachment.
                BoundedJoinOutcome::DeadlineExpired => {}
            }
        }
    }

    async fn await_reaper<T>(reaper: &mut tokio::task::JoinHandle<T>) -> Result<T, ()> {
        loop {
            match join_runner_with_deadline(reaper, std::time::Duration::from_secs(2)).await {
                BoundedJoinOutcome::Joined(Ok(value)) => return Ok(value),
                BoundedJoinOutcome::Joined(Err(_)) => return Err(()),
                // The direct child was already verified dead before this is
                // called. Retain the handle and keep each wait bounded rather
                // than dropping a still-active reaper task.
                BoundedJoinOutcome::DeadlineExpired => {}
            }
        }
    }

    fn assert_cancelled_join<T>(outcome: BoundedJoinOutcome<T>, context: &str) {
        match outcome {
            BoundedJoinOutcome::Joined(Err(error)) if error.is_cancelled() => {}
            BoundedJoinOutcome::Joined(Err(_)) => panic!("{context}: runner task panicked"),
            BoundedJoinOutcome::Joined(Ok(_)) => {
                panic!("{context}: runner task completed instead of cancelling")
            }
            BoundedJoinOutcome::DeadlineExpired => {
                panic!("{context}: runner task remained active past its join deadline")
            }
        }
    }

    fn assert_terminal_join<T>(outcome: BoundedJoinOutcome<T>, context: &str) {
        if matches!(outcome, BoundedJoinOutcome::DeadlineExpired) {
            panic!("{context}: runner task remained active past its terminal join");
        }
    }

    async fn wait_for_live_pids(
        direct_pid_file: &Path,
        descendant_pid_file: &Path,
    ) -> Result<(u32, u32), ()> {
        time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match (
                    recorded_pid(direct_pid_file)?,
                    recorded_pid(descendant_pid_file)?,
                ) {
                    (Some(direct), Some(descendant)) if direct != descendant => {
                        if process_is_alive(direct) && process_is_alive(descendant) {
                            return Ok((direct, descendant));
                        }
                        return Err(());
                    }
                    _ => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .map_err(|_| ())?
    }

    async fn wait_for_cleanup_manifest(path: &Path) -> Result<(u32, u32), ()> {
        time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some((direct, descendant)) = recorded_cleanup_manifest(path)? {
                    if process_is_alive(direct) && process_is_alive(descendant) {
                        return Ok((direct, descendant));
                    }
                    return Err(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ())?
    }

    async fn wait_for_cleanup_pid(path: &Path) -> Result<u32, ()> {
        time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(pid) = recorded_pid(path)? {
                    if process_is_alive(pid) {
                        return Ok(pid);
                    }
                    return Err(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ())?
    }

    /// Acquires the fixture's independent cleanup authority before inspecting
    /// any deliberately fallible protocol files. `cleanup.direct.pid` is
    /// published before the helper may launch its descendant; the separate
    /// descendant authority is published immediately after launch, and the
    /// manifest cross-checks the pair. Every PID obtained here is armed before
    /// an authority error is returned.
    async fn arm_cleanup_authority(
        cleanup: &mut DescendantCleanup,
        direct_path: &Path,
        descendant_path: &Path,
        manifest_path: &Path,
    ) -> Result<(u32, u32), ()> {
        let direct = arm_direct_cleanup_authority(cleanup, direct_path).await?;
        let descendant = arm_descendant_cleanup_authority(cleanup, descendant_path).await?;
        corroborate_cleanup_manifest(direct, descendant, manifest_path).await?;
        Ok((direct, descendant))
    }

    async fn arm_gated_cleanup_authority(
        cleanup: &mut DescendantCleanup,
        direct_path: &Path,
        descendant_path: &Path,
        manifest_path: &Path,
        gate_path: &Path,
    ) -> Result<(u32, u32), ()> {
        let direct = arm_direct_cleanup_authority(cleanup, direct_path).await?;
        if !process_is_alive(direct) || release_descendant_launch_gate(gate_path).is_err() {
            return Err(());
        }
        let descendant = arm_descendant_cleanup_authority(cleanup, descendant_path).await?;
        corroborate_cleanup_manifest(direct, descendant, manifest_path).await?;
        Ok((direct, descendant))
    }

    async fn arm_direct_cleanup_authority(
        cleanup: &mut DescendantCleanup,
        direct_path: &Path,
    ) -> Result<u32, ()> {
        let direct = wait_for_cleanup_pid(direct_path).await?;
        cleanup.arm_unique_pid(direct)?;
        Ok(direct)
    }

    async fn arm_descendant_cleanup_authority(
        cleanup: &mut DescendantCleanup,
        descendant_path: &Path,
    ) -> Result<u32, ()> {
        let descendant = wait_for_cleanup_pid(descendant_path).await?;
        cleanup.arm_unique_pid(descendant)?;
        Ok(descendant)
    }

    async fn corroborate_cleanup_manifest(
        direct: u32,
        descendant: u32,
        manifest_path: &Path,
    ) -> Result<(), ()> {
        let (manifest_direct, manifest_descendant) =
            wait_for_cleanup_manifest(manifest_path).await?;
        ((direct, descendant) == (manifest_direct, manifest_descendant))
            .then_some(())
            .ok_or(())
    }

    fn release_descendant_launch_gate(path: &Path) -> Result<(), ()> {
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, b"release\n").map_err(|_| ())?;
        fs::rename(temporary, path).map_err(|_| ())
    }

    fn publication_fixture_script() -> &'static str {
        "#!/bin/sh\nfixture=$1\nmode=$2\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\nwhile [ ! -f \"$fixture/launch.descendant\" ]; do :; done\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nif [ \"$mode\" = duplicate-manifest-malformed ]; then printf 'bad-manifest\\n' > \"$fixture/cleanup.manifest.tmp\"; else printf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\"; fi\nmv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\ntrap - EXIT\ncase \"$mode\" in\n  malformed-desc) printf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\"; mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf 'not-a-pid\\n' > \"$fixture/descendant.pid.tmp\"; mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" ;;\n  partial-desc) printf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\"; mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf '12' > \"$fixture/descendant.pid\" ;;\n  malformed-direct) printf 'not-a-pid\\n' > \"$fixture/direct.pid.tmp\"; mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf '%s\\n' \"$descendant\" > \"$fixture/descendant.pid.tmp\"; mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" ;;\n  both-malformed) printf 'bad-direct\\n' > \"$fixture/direct.pid.tmp\"; mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf 'bad-descendant\\n' > \"$fixture/descendant.pid.tmp\"; mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" ;;\n  missing) : ;;\n  duplicate|duplicate-manifest-malformed) printf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\"; mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf '%s\\n' \"$$\" > \"$fixture/descendant.pid.tmp\"; mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" ;;\n  *) exit 64 ;;\nesac\nprintf complete > \"$fixture/protocol.complete\"\nwhile :; do :; done\n"
    }

    fn supervised_fixture_script() -> &'static str {
        "#!/bin/sh\nfixture=$1\nevents=$2\nmode=$3\npython=$4\nif [ \"${SENTINEL_FIXTURE_GROUPED:-}\" != 1 ]; then\n  exec \"$python\" -c 'import os,sys; os.setpgrp(); os.environ[\"SENTINEL_FIXTURE_GROUPED\"]=\"1\"; os.execv(sys.argv[1], [sys.argv[1], *sys.argv[2:]])' \"$0\" \"$@\"\nfi\nexec 3>\"$events\"\nemit() { printf '%s\\n' \"$1\" >&3; }\nif [ \"$mode\" = forged-group-ready ]; then emit \"GROUP_READY 1 1\"; else emit \"GROUP_READY $$ $$\"; fi\nwhile [ ! -f \"$fixture/launch.descendant\" ]; do :; done\ncase \"$mode\" in\n  normal) printf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\" && mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\"; printf ready > \"$fixture/ready\" ;;\n  post-gate-publication-failure|forged-group-ready) printf 'malformed-manifest\\n' > \"$fixture/cleanup.manifest\" ;;\n  duplicate-protocol) printf '%s\\n' \"$$\" > \"$fixture/direct.pid\"; printf '%s\\n' \"$$\" > \"$fixture/descendant.pid\" ;;\n  ignore-term) trap : TERM ;;\n  *) exit 64 ;;\nesac\nwhile :; do :; done\n"
    }

    async fn supervised_fixture_runner(
        mode: &'static str,
        timeout: std::time::Duration,
    ) -> (
        tempfile::TempDir,
        tokio::task::JoinHandle<Result<GitCommandOutput, GitError>>,
        FixtureProcessSupervisor,
        mpsc::Receiver<u32>,
    ) {
        let (directory, executable) = fixture_executable(supervised_fixture_script());
        let fixture_path = directory.path().to_path_buf();
        let supervisor =
            FixtureProcessSupervisor::new(&fixture_path).expect("test-only fixture supervisor");
        let event_path = fixture_path.join("supervisor.events");
        let python = fixture_python();
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let (pid_sender, pid_receiver) = mpsc::channel();
        let runner = tokio::spawn(async move {
            run_git_with_timeout_observed(
                &executable_for_task,
                &directory_for_task,
                [
                    directory_for_task.to_str().expect("UTF-8 fixture path"),
                    event_path.to_str().expect("UTF-8 event FIFO path"),
                    mode,
                    python.to_str().expect("UTF-8 test-only Python path"),
                ],
                timeout,
                pid_sender,
            )
            .await
        });
        (directory, runner, supervisor, pid_receiver)
    }

    async fn observed_runner_child(receiver: &mpsc::Receiver<u32>) -> Result<u32, ()> {
        time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match receiver.try_recv() {
                    Ok(pid) if pid != 0 => return Ok(pid),
                    Ok(_) | Err(mpsc::TryRecvError::Disconnected) => return Err(()),
                    Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .map_err(|_| ())?
    }

    async fn arm_supervised_group(
        supervisor: &mut FixtureProcessSupervisor,
        observed_child: &mpsc::Receiver<u32>,
    ) -> Result<u32, ()> {
        let trusted_direct_pid = observed_runner_child(observed_child).await?;
        let FixtureEvent::GroupReady { direct_pid, pgid } =
            supervisor.next_event().await.map_err(|_| ())?
        else {
            return Err(());
        };
        supervisor.arm_group(trusted_direct_pid, direct_pid, pgid)?;
        Ok(trusted_direct_pid)
    }

    async fn wait_for_invalid_protocol_publication(
        direct_pid_file: &Path,
        descendant_pid_file: &Path,
        completion_file: &Path,
    ) -> bool {
        let completed = time::timeout(std::time::Duration::from_secs(2), async {
            while !completion_file.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        if !completed {
            return false;
        }
        !matches!(
            (recorded_pid(direct_pid_file), recorded_pid(descendant_pid_file)),
            (Ok(Some(direct)), Ok(Some(descendant))) if direct != descendant
        )
    }

    async fn assert_invalid_publication_is_cleaned(mode: &str) {
        let (directory, executable) = fixture_executable(publication_fixture_script());
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let launch_gate = fixture_path.join("launch.descendant");
        let completion_file = fixture_path.join("protocol.complete");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mode_for_task = mode.to_owned();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [
                    directory_for_task.to_str().expect("UTF-8 fixture path"),
                    mode_for_task.as_str(),
                ],
                std::time::Duration::from_secs(30),
            )
            .await
        });
        let mut cleanup = DescendantCleanup::new();
        let authority = arm_gated_cleanup_authority(
            &mut cleanup,
            &cleanup_direct_pid_file,
            &cleanup_descendant_pid_file,
            &cleanup_manifest,
            &launch_gate,
        )
        .await;
        let observed = authority.as_ref().ok().copied();
        let protocol_invalid = wait_for_invalid_protocol_publication(
            &direct_pid_file,
            &descendant_pid_file,
            &completion_file,
        )
        .await;
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let both_dead = observed.is_some_and(|(direct, descendant)| {
            !process_is_alive(direct) && !process_is_alive(descendant)
        });

        assert!(
            authority.is_ok(),
            "trusted cleanup authority must arm both PIDs"
        );
        assert!(protocol_invalid, "the protocol publication must be invalid");
        assert!(runner_active, "the runner must remain active before abort");
        assert!(
            cleanup_result.is_ok(),
            "invalid-publication cleanup must complete"
        );
        assert_cancelled_join(final_join, "invalid-publication cleanup");
        assert!(both_dead, "cleanup manifest processes must not survive");
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
    async fn inventory_operation_uses_its_fixed_read_only_arguments_and_optional_lock_guard() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\n[ \"$GIT_OPTIONAL_LOCKS\" = 0 ] || exit 10\n[ \"$#\" = 11 ] || exit 11\n[ \"$1\" = --no-optional-locks ] || exit 12\n[ \"$2\" = -c ] || exit 13\n[ \"$3\" = core.fsmonitor=false ] || exit 14\n[ \"$4\" = -c ] || exit 15\n[ \"$5\" = core.untrackedCache=false ] || exit 16\n[ \"$6\" = status ] || exit 17\n[ \"$7\" = --porcelain=v2 ] || exit 18\n[ \"$8\" = -z ] || exit 19\n[ \"$9\" = --untracked-files=all ] || exit 20\n[ \"${10}\" = --ignore-submodules=none ] || exit 21\n[ \"${11}\" = --no-renames ] || exit 22\n",
        );
        let output = run_git_with_limits(
            &executable,
            directory.path(),
            STATUS_INVENTORY_ARGS,
            GIT_TIMEOUT,
            MAX_STATUS_STDOUT,
            MAX_STATUS_STDERR,
            true,
            false,
        )
        .await
        .expect("fixed inventory command");
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[tokio::test]
    async fn numstat_operation_uses_fixed_literal_pathspec_arguments_and_environment() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\n[ \"$GIT_OPTIONAL_LOCKS\" = 0 ] || exit 10\n[ \"$GIT_LITERAL_PATHSPECS\" = 1 ] || exit 11\n[ \"$1\" = --no-optional-locks ] || exit 12\n[ \"$2\" = --literal-pathspecs ] || exit 13\n[ \"$3\" = -c ] || exit 14\n[ \"$4\" = core.fsmonitor=false ] || exit 15\n[ \"$5\" = -c ] || exit 16\n[ \"$6\" = core.untrackedCache=false ] || exit 17\n[ \"$7\" = -c ] || exit 18\n[ \"$8\" = color.ui=false ] || exit 19\n[ \"$9\" = diff ] || exit 20\n[ \"${10}\" = --numstat ] || exit 21\n[ \"${11}\" = -z ] || exit 22\n[ \"${12}\" = --no-ext-diff ] || exit 23\n[ \"${13}\" = --no-textconv ] || exit 24\n[ \"${14}\" = --no-color ] || exit 25\n[ \"${15}\" = --no-renames ] || exit 26\n[ \"${16}\" = -- ] || exit 27\n[ \"${17}\" = ':(glob)*.txt' ] || exit 28\nprintf '1\\t2\\t:(glob)*.txt\\0'\n",
        );
        let output = run_git_with_limits(
            &executable,
            directory.path(),
            [
                "--no-optional-locks",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "color.ui=false",
                "diff",
                "--numstat",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--",
                ":(glob)*.txt",
            ],
            GIT_TIMEOUT,
            MAX_NUMSTAT_STDOUT,
            MAX_NUMSTAT_STDERR,
            true,
            true,
        )
        .await
        .expect("fixed numstat command");
        assert!(output.status.success());
        assert_eq!(
            parse_numstat_z(&output.stdout, ":(glob)*.txt").unwrap(),
            NumstatClassification::TextEligible(TextEligibleMetadata {
                additions: 1,
                deletions: 2,
            })
        );
    }

    #[tokio::test]
    async fn numstat_operation_treats_pathspec_looking_fixture_names_as_one_literal_path() {
        let fixture = tempfile::tempdir().expect("fixture");
        let primary = fixture.path().join("primary");
        let worktree = fixture.path().join("linked");
        fs::create_dir(&primary).expect("primary directory");
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        for name in [
            ":(glob)*.txt",
            ":(literal)exact.txt",
            ":!excluded.txt",
            ":^excluded.txt",
            ":/rooted.txt",
            "*.txt",
            "file?.txt",
            "file[1].txt",
            "-leading.txt",
            "name with spaces.txt",
            "ünicode.txt",
            "newline\nname.txt",
            "folder\\name.txt",
        ] {
            let path = primary.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("fixture parent");
            }
            fs::write(path, "before\n").expect("fixture file");
        }
        fixture_git(&primary, &["add", "."]);
        fixture_git(&primary, &["commit", "-m", "fixture"]);
        let base = resolve_exact_head(&primary).await.expect("base commit");
        add_detached_worktree(&primary, &worktree, &base)
            .await
            .expect("linked worktree");
        for name in [
            ":(glob)*.txt",
            ":(literal)exact.txt",
            ":!excluded.txt",
            ":^excluded.txt",
            ":/rooted.txt",
            "*.txt",
            "file?.txt",
            "file[1].txt",
            "-leading.txt",
            "name with spaces.txt",
            "ünicode.txt",
            "newline\nname.txt",
            "folder\\name.txt",
        ] {
            let path = worktree.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("changed fixture parent");
            }
            fs::write(path, "after\n").expect("changed fixture file");
        }
        fixture_git(&worktree, &["add", "."]);
        let inventory = legacy_unsafe_status_control(&worktree)
            .await
            .expect("trusted inventory");
        assert_eq!(inventory.len(), 13);
        for name in [
            ":(glob)*.txt",
            ":(literal)exact.txt",
            ":!excluded.txt",
            ":^excluded.txt",
            ":/rooted.txt",
            "*.txt",
            "file?.txt",
            "file[1].txt",
            "-leading.txt",
            "name with spaces.txt",
            "ünicode.txt",
            "newline\nname.txt",
            "folder\\name.txt",
        ] {
            let path = inventory
                .iter()
                .find(|file| file.path.as_str() == name)
                .expect("exact inventory path")
                .path
                .clone();
            assert_eq!(
                inspect_worktree_numstat(&worktree, &path, Some(&base))
                    .await
                    .expect("literal numstat"),
                NumstatClassification::TextEligible(TextEligibleMetadata {
                    additions: 1,
                    deletions: 1,
                })
            );
        }
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

    #[tokio::test]
    async fn fixture_supervisor_gates_descendant_launch_and_group_cleans_protocol_failure() {
        let (directory, mut runner, mut supervisor, child_pid) = supervised_fixture_runner(
            "post-gate-publication-failure",
            std::time::Duration::from_secs(30),
        )
        .await;
        let fixture = directory.path().to_path_buf();
        let direct_pid = arm_supervised_group(&mut supervisor, &child_pid).await;
        let zero_launches_before_release = direct_pid.is_ok_and(process_is_alive)
            && supervisor.descendant_launch_count() == 0
            && !fixture.join("descendant.pid").exists()
            && !fixture.join("cleanup.manifest").exists();
        let release = supervisor.authorize_descendant_launch();
        let owned_descendant = if release.is_ok() {
            supervisor.launch_owned_descendant(false)
        } else {
            Err(())
        };
        let started = supervisor.next_event().await;
        let launched = supervisor.next_event().await;
        let descendant_membership = match launched {
            Ok(FixtureEvent::DescendantLaunched { pid }) => {
                supervisor.verify_descendant_membership(pid).is_ok()
            }
            _ => false,
        };
        let one_launch_after_release = matches!(started, Ok(FixtureEvent::DescendantLaunchStarted))
            && owned_descendant.is_ok()
            && descendant_membership
            && supervisor.descendant_launch_count() == 1;
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) =
            abort_supervised_runner(&mut runner, &mut supervisor).await;
        let group_dead = direct_pid.is_ok_and(|pid| !process_group_is_alive(pid));
        let publication_failed = !fixture.join("descendant.pid").exists()
            && !matches!(
                recorded_cleanup_manifest(&fixture.join("cleanup.manifest")),
                Ok(Some(_))
            );

        assert!(
            direct_pid.is_ok(),
            "supervisor must own the direct group before release"
        );
        assert!(
            zero_launches_before_release,
            "no descendant launch may precede authorization"
        );
        assert!(
            release.is_ok(),
            "group ownership must authorize this fixture launch"
        );
        assert!(
            one_launch_after_release,
            "one supervisor-observed descendant launch is required"
        );
        assert!(
            runner_active,
            "runner must remain owned until group cleanup"
        );
        assert!(
            publication_failed,
            "protocol publication failure must be observable"
        );
        assert!(
            cleanup_result.is_ok(),
            "group cleanup must not depend on protocol PID files"
        );
        assert_terminal_join(final_join, "supervisor protocol-failure cleanup");
        assert!(
            group_dead,
            "the fixture process group must be gone after cleanup"
        );
        assert_eq!(
            supervisor.signal_sequence(),
            vec![SignalAttempt {
                pgid: direct_pid.expect("verified direct PID"),
                signal: libc::SIGTERM,
                outcome: GroupSignal::Sent,
            }],
            "SIGTERM must clean the fixture group"
        );
        supervisor.stop_reader();
    }

    #[tokio::test]
    async fn fixture_supervisor_observes_duplicate_protocol_without_manifest_authority() {
        let (directory, mut runner, mut supervisor, child_pid) =
            supervised_fixture_runner("duplicate-protocol", std::time::Duration::from_secs(30))
                .await;
        let fixture = directory.path().to_path_buf();
        let direct_pid = arm_supervised_group(&mut supervisor, &child_pid).await;
        let release = supervisor.authorize_descendant_launch();
        let owned_descendant = if release.is_ok() {
            supervisor.launch_owned_descendant(false)
        } else {
            Err(())
        };
        let started = supervisor.next_event().await;
        let launched = supervisor.next_event().await;
        let descendant_membership = matches!(launched, Ok(FixtureEvent::DescendantLaunched { pid }) if supervisor.verify_descendant_membership(pid).is_ok());
        let duplicate_protocol =
            wait_for_live_pids(&fixture.join("direct.pid"), &fixture.join("descendant.pid"))
                .await
                .is_err();
        let manifest_absent = !fixture.join("cleanup.manifest").exists();
        let (cleanup_result, final_join) =
            abort_supervised_runner(&mut runner, &mut supervisor).await;
        let group_dead = direct_pid.is_ok_and(|pid| !process_group_is_alive(pid));

        assert!(direct_pid.is_ok());
        assert!(release.is_ok());
        assert!(owned_descendant.is_ok());
        assert!(matches!(started, Ok(FixtureEvent::DescendantLaunchStarted)));
        assert!(descendant_membership);
        assert_eq!(supervisor.descendant_launch_count(), 1);
        assert!(
            duplicate_protocol,
            "duplicate ordinary protocol PIDs must be rejected"
        );
        assert!(
            manifest_absent,
            "cleanup manifest must not be required for group cleanup"
        );
        assert!(cleanup_result.is_ok());
        assert_terminal_join(final_join, "supervisor duplicate-protocol cleanup");
        assert!(group_dead);
        supervisor.stop_reader();
    }

    #[tokio::test]
    async fn fixture_supervisor_preserves_the_real_timeout_audit() {
        let (directory, mut runner, mut supervisor, child_pid) =
            supervised_fixture_runner("normal", std::time::Duration::from_secs(5)).await;
        let fixture = directory.path().to_path_buf();
        let direct_pid = arm_supervised_group(&mut supervisor, &child_pid).await;
        let release = supervisor.authorize_descendant_launch();
        let owned_descendant = if release.is_ok() {
            supervisor.launch_owned_descendant(false)
        } else {
            Err(())
        };
        let started = supervisor.next_event().await;
        let launched = supervisor.next_event().await;
        let descendant_membership = matches!(launched, Ok(FixtureEvent::DescendantLaunched { pid }) if supervisor.verify_descendant_membership(pid).is_ok());
        let ready = fixture.join("ready").exists();
        let result =
            join_runner_with_deadline(&mut runner, std::time::Duration::from_secs(7)).await;
        let direct_dead = direct_pid.is_ok_and(|pid| !process_is_alive(pid));
        let group_was_live = direct_pid.is_ok_and(process_group_is_alive);
        let cleanup_result = supervisor.cleanup_group().await;
        let group_dead = direct_pid.is_ok_and(|pid| !process_group_is_alive(pid));

        assert!(direct_pid.is_ok());
        assert!(release.is_ok());
        assert!(owned_descendant.is_ok());
        assert!(matches!(started, Ok(FixtureEvent::DescendantLaunchStarted)));
        assert!(descendant_membership);
        assert!(ready);
        assert_eq!(supervisor.descendant_launch_count(), 1);
        assert!(matches!(
            result,
            BoundedJoinOutcome::Joined(Ok(Err(GitError::TimedOut)))
        ));
        assert!(
            direct_dead,
            "the production timeout path must reap its direct child"
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert!(
            group_was_live,
            "the documented macOS/aarch64 descendant survival changed"
        );
        assert!(
            cleanup_result.is_ok(),
            "test-only group cleanup must remove the survivor"
        );
        assert!(group_dead);
        supervisor.stop_reader();
        assert!(fixture.join("direct.pid").exists());
    }

    #[tokio::test]
    async fn fixture_supervisor_uses_sigkill_when_group_ignores_sigterm() {
        let (_directory, mut runner, mut supervisor, child_pid) =
            supervised_fixture_runner("ignore-term", std::time::Duration::from_secs(30)).await;
        let direct_pid = arm_supervised_group(&mut supervisor, &child_pid).await;
        let release = supervisor.authorize_descendant_launch();
        let owned_descendant = if release.is_ok() {
            supervisor.launch_owned_descendant(true)
        } else {
            Err(())
        };
        let started = supervisor.next_event().await;
        let launched = supervisor.next_event().await;
        let term_ignore_ready = supervisor.next_event().await;
        let descendant_membership = matches!(launched, Ok(FixtureEvent::DescendantLaunched { pid }) if supervisor.verify_descendant_membership(pid).is_ok());
        let term_ignore_identity = matches!(
            term_ignore_ready,
            Ok(FixtureEvent::TermIgnoreReady { pid })
                if owned_descendant == Ok(pid) && supervisor.verify_descendant_membership(pid).is_ok()
        );
        let (cleanup_result, final_join) =
            abort_supervised_runner(&mut runner, &mut supervisor).await;
        let group_dead = direct_pid.is_ok_and(|pid| !process_group_is_alive(pid));

        assert!(direct_pid.is_ok());
        assert!(release.is_ok());
        assert!(owned_descendant.is_ok());
        assert!(matches!(started, Ok(FixtureEvent::DescendantLaunchStarted)));
        assert!(descendant_membership);
        assert!(term_ignore_identity);
        assert!(cleanup_result.is_ok());
        assert_terminal_join(final_join, "supervisor SIGKILL fallback cleanup");
        assert!(group_dead);
        assert_eq!(
            supervisor.signal_sequence(),
            vec![
                SignalAttempt {
                    pgid: direct_pid.expect("verified direct PID"),
                    signal: libc::SIGTERM,
                    outcome: GroupSignal::Sent,
                },
                SignalAttempt {
                    pgid: direct_pid.expect("verified direct PID"),
                    signal: libc::SIGKILL,
                    outcome: GroupSignal::Sent,
                },
            ],
            "the SIGTERM-ignoring fixture must remain live until SIGKILL"
        );
        assert_eq!(
            supervisor.pgid, None,
            "verified cleanup must disarm the group"
        );
        assert_eq!(
            supervisor.post_sigterm_probe,
            Some(GroupProbe::Alive),
            "the SIGTERM-ignoring owned descendant must keep the group alive before SIGKILL"
        );
        supervisor.stop_reader();
    }

    #[tokio::test]
    async fn fixture_supervisor_rejects_a_same_group_pid_that_is_not_its_owned_descendant() {
        let (_directory, mut runner, mut supervisor, child_pid) =
            supervised_fixture_runner("normal", std::time::Duration::from_secs(30)).await;
        let direct_pid = arm_supervised_group(&mut supervisor, &child_pid).await;
        let release = supervisor.authorize_descendant_launch();
        let owned_descendant = if release.is_ok() {
            supervisor.launch_owned_descendant(false)
        } else {
            Err(())
        };
        let started = supervisor.next_event().await;
        let launched = supervisor.next_event().await;
        let decoy_pid = supervisor.launch_same_group_decoy();
        let wrong_same_group = matches!(
            (owned_descendant, decoy_pid, direct_pid),
            (Ok(owned), Ok(decoy), Ok(pgid))
                if owned != decoy
                    && process_is_alive(owned)
                    && process_is_alive(decoy)
                    && process_group_of(owned) == Some(pgid)
                    && process_group_of(decoy) == Some(pgid)
                    && supervisor.verify_descendant_membership(decoy).is_err()
        );
        let forged_rejected = if let Ok(decoy) = decoy_pid {
            supervisor
                .validate_event_sequence(FixtureEvent::DescendantLaunched { pid: decoy })
                .is_err()
                && supervisor.verify_descendant_membership(decoy).is_err()
        } else {
            false
        };
        let (cleanup_result, final_join) =
            abort_supervised_runner(&mut runner, &mut supervisor).await;
        let group_dead = direct_pid.is_ok_and(|pid| !process_group_is_alive(pid));

        assert!(matches!(started, Ok(FixtureEvent::DescendantLaunchStarted)));
        assert!(
            matches!(launched, Ok(FixtureEvent::DescendantLaunched { pid }) if owned_descendant == Ok(pid))
        );
        assert!(
            wrong_same_group,
            "a live same-group decoy must not satisfy owned-child identity"
        );
        assert!(
            forged_rejected,
            "a forged second launch event must be rejected"
        );
        assert!(cleanup_result.is_ok());
        assert_terminal_join(final_join, "same-group wrong descendant PID cleanup");
        assert!(group_dead);
        supervisor.stop_reader();
    }

    #[test]
    fn fixture_supervisor_event_frames_are_bounded_and_strict() {
        assert_eq!(
            parse_fixture_event(b"GROUP_READY 42 42\n"),
            Ok(Some(FixtureEvent::GroupReady {
                direct_pid: 42,
                pgid: 42,
            }))
        );
        assert_eq!(
            parse_fixture_event(b"TERM_IGNORE_READY 43\n"),
            Ok(Some(FixtureEvent::TermIgnoreReady { pid: 43 }))
        );
        assert_eq!(
            parse_fixture_event(&[b'x'; MAX_SUPERVISOR_EVENT_FRAME_BYTES + 1]),
            Err(SupervisorProtocolError::FrameTooLarge)
        );
        assert_eq!(
            parse_fixture_event(b"GROUP_READY 42 42"),
            Err(SupervisorProtocolError::UnterminatedFrame)
        );
        assert_eq!(
            parse_fixture_event(b"GROUP_READY 42 42 \n"),
            Err(SupervisorProtocolError::UnexpectedWhitespace)
        );
        assert_eq!(
            parse_fixture_event(b"GROUP_READY 42 42\r\n"),
            Err(SupervisorProtocolError::TrailingData)
        );
        assert_eq!(
            parse_fixture_event(b"DESCENDANT_LAUNCHED 42 extra\n"),
            Err(SupervisorProtocolError::InvalidFieldCount)
        );
    }

    #[test]
    fn fixture_supervisor_errno_classification_only_accepts_esrch_as_absence() {
        assert_eq!(
            classify_group_errno(libc::ESRCH),
            GroupProbe::Absent,
            "only ESRCH proves a process group absent"
        );
        assert_eq!(
            classify_group_errno(libc::EPERM),
            GroupProbe::PermissionDenied
        );
        assert_eq!(
            classify_group_errno(libc::EINVAL),
            GroupProbe::Failed(libc::EINVAL)
        );
        assert_eq!(
            classify_group_signal_errno(libc::ESRCH),
            GroupSignal::Absent
        );
        assert_eq!(
            classify_group_signal_errno(libc::EPERM),
            GroupSignal::PermissionDenied
        );
    }

    #[tokio::test]
    async fn fixture_supervisor_rejects_an_unrelated_group_ready_without_signalling() {
        let fixture = tempfile::tempdir().expect("fixture");
        let mut trusted = spawn_group_leader().await;
        let mut unrelated = spawn_group_leader().await;
        let trusted_pid = trusted.id();
        let unrelated_pid = unrelated.id();
        let leaders_ready =
            wait_for_group_leader(trusted_pid).await && wait_for_group_leader(unrelated_pid).await;
        let mut supervisor = FixtureProcessSupervisor::new(fixture.path()).expect("supervisor");
        let rejected = supervisor
            .arm_group(trusted_pid, unrelated_pid, unrelated_pid)
            .is_err();
        terminate_disposable(&mut trusted);
        terminate_disposable(&mut unrelated);

        assert!(
            leaders_ready,
            "both disposable leaders must establish their groups"
        );
        assert!(
            rejected,
            "an unrelated FIFO PID must not replace Child::id authority"
        );
        assert_eq!(supervisor.pgid, None);
        assert!(supervisor.signal_sequence().is_empty());
        supervisor.stop_reader();
    }

    #[tokio::test]
    async fn fixture_supervisor_rejects_forged_group_ready_without_signalling() {
        let (_directory, mut runner, mut supervisor, child_pid) =
            supervised_fixture_runner("forged-group-ready", std::time::Duration::from_secs(30))
                .await;
        let binding = arm_supervised_group(&mut supervisor, &child_pid).await;
        if !runner.is_finished() {
            runner.abort();
        }
        let terminal = join_runner_until_terminal(&mut runner).await;

        assert!(
            binding.is_err(),
            "FIFO identity must not replace Child::id authority"
        );
        assert_eq!(
            supervisor.pgid, None,
            "a forged GroupReady must not arm a PGID"
        );
        assert!(
            supervisor.signal_sequence().is_empty(),
            "a rejected handshake must not signal an untrusted group"
        );
        assert_cancelled_join(terminal, "forged GroupReady runner cleanup");
        supervisor.stop_reader();
    }

    #[tokio::test]
    async fn descendant_launch_gate_requires_direct_cleanup_ownership() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\nwhile [ ! -f \"$fixture/launch.descendant\" ]; do :; done\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nprintf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_path = fixture_path.join("cleanup.direct.pid");
        let descendant_path = fixture_path.join("cleanup.descendant.pid");
        let manifest_path = fixture_path.join("cleanup.manifest");
        let gate = fixture_path.join("launch.descendant");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut cleanup = DescendantCleanup::new();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [directory_for_task.to_str().expect("UTF-8 fixture path")],
                std::time::Duration::from_secs(30),
            )
            .await
        });
        let direct = arm_direct_cleanup_authority(&mut cleanup, &direct_path).await;
        let descendant_absent_before_release = !descendant_path.exists();
        let gate_released =
            direct.is_ok_and(process_is_alive) && release_descendant_launch_gate(&gate).is_ok();
        let descendant = if gate_released {
            arm_descendant_cleanup_authority(&mut cleanup, &descendant_path).await
        } else {
            Err(())
        };
        let manifest_matches = match (direct, descendant) {
            (Ok(direct), Ok(descendant)) => {
                corroborate_cleanup_manifest(direct, descendant, &manifest_path).await
            }
            _ => Err(()),
        };
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;

        assert!(
            descendant_absent_before_release,
            "the descendant must not launch before gate release"
        );
        assert!(
            gate_released,
            "the gate must release only after direct cleanup ownership exists"
        );
        assert!(
            manifest_matches.is_ok(),
            "post-release descendant authority must corroborate the manifest"
        );
        assert!(
            runner_active,
            "gate fixture runner must remain active until cleanup"
        );
        assert!(cleanup_result.is_ok(), "gate fixture cleanup must complete");
        assert_cancelled_join(final_join, "gate fixture cleanup");
    }

    #[tokio::test]
    async fn unreleased_descendant_launch_gate_cleans_the_direct_helper_without_a_descendant() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\nwhile [ ! -f \"$fixture/launch.descendant\" ]; do :; done\n/bin/sh -c 'while :; do :; done' &\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_path = fixture_path.join("cleanup.direct.pid");
        let descendant_path = fixture_path.join("cleanup.descendant.pid");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut cleanup = DescendantCleanup::new();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [directory_for_task.to_str().expect("UTF-8 fixture path")],
                std::time::Duration::from_secs(30),
            )
            .await
        });
        let direct = arm_direct_cleanup_authority(&mut cleanup, &direct_path).await;
        let descendant_absent = !descendant_path.exists();
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let direct_dead = direct.is_ok_and(|pid| !process_is_alive(pid));

        assert!(
            direct.is_ok(),
            "direct authority must be armed while the gate remains closed"
        );
        assert!(
            descendant_absent,
            "an unreleased gate must prevent descendant launch"
        );
        assert!(
            runner_active,
            "gate-blocked runner must remain owned until cleanup"
        );
        assert!(
            cleanup_result.is_ok(),
            "gate-blocked direct helper cleanup must complete"
        );
        assert_cancelled_join(final_join, "gate-blocked cleanup");
        assert!(
            direct_dead,
            "gate-blocked direct helper must not survive cleanup"
        );
    }

    #[tokio::test]
    async fn normal_timeout_authority_failure_still_cleans_both_processes() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nprintf 'malformed-manifest\\n' > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_authority = fixture_path.join("cleanup.direct.pid");
        let descendant_authority = fixture_path.join("cleanup.descendant.pid");
        let manifest = fixture_path.join("cleanup.manifest");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut cleanup = DescendantCleanup::new();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [directory_for_task.to_str().expect("UTF-8 fixture path")],
                std::time::Duration::from_secs(30),
            )
            .await
        });

        let independent = match (
            arm_direct_cleanup_authority(&mut cleanup, &direct_authority).await,
            arm_descendant_cleanup_authority(&mut cleanup, &descendant_authority).await,
        ) {
            (Ok(direct), Ok(descendant)) => Ok((direct, descendant)),
            _ => Err(()),
        };
        let manifest_result = wait_for_cleanup_manifest(&manifest).await;
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let both_dead = independent.is_ok_and(|(direct, descendant)| {
            !process_is_alive(direct) && !process_is_alive(descendant)
        });

        assert!(
            independent.is_ok(),
            "independent authority must arm both fixture PIDs"
        );
        assert!(
            manifest_result.is_err(),
            "the manifest failure must be observed as an authority error"
        );
        assert!(
            runner_active,
            "runner must remain active until authority-failure cleanup begins"
        );
        assert!(
            cleanup_result.is_ok(),
            "authority-failure cleanup must complete"
        );
        assert_cancelled_join(final_join, "authority-failure cleanup");
        assert!(
            both_dead,
            "authority failure must not leave either fixture process alive"
        );
    }

    #[tokio::test]
    async fn runner_readiness_failure_aborts_joins_and_cleans_recorded_processes() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\nprintf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\" && mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\" || exit 71\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 72\nprintf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 73\nprintf '%s\\n' \"$descendant\" > \"$fixture/descendant.pid.tmp\" && mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" || exit 74\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let ready_file = fixture_path.join("never-ready");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut cleanup = DescendantCleanup::new();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [directory_for_task.to_str().expect("UTF-8 fixture path")],
                std::time::Duration::from_secs(30),
            )
            .await
        });

        let authority = arm_cleanup_authority(
            &mut cleanup,
            &cleanup_direct_pid_file,
            &cleanup_descendant_pid_file,
            &cleanup_manifest,
        )
        .await;
        let protocol_pids = wait_for_live_pids(&direct_pid_file, &descendant_pid_file).await;
        let ready_withheld = !ready_file.exists();
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let authority_pids_dead = authority.is_ok_and(|(direct, descendant)| {
            !process_is_alive(direct) && !process_is_alive(descendant)
        });
        let protocol_matches_authority = matches!(
            (&authority, &protocol_pids),
            (Ok(authority), Ok(protocol)) if authority == protocol
        );
        assert!(
            authority.is_ok(),
            "cleanup authority must be available before readiness failure is reported"
        );
        assert!(
            protocol_matches_authority,
            "protocol PIDs must match already-armed authority PIDs"
        );
        assert!(
            ready_withheld,
            "the fixture must deliberately withhold readiness"
        );
        assert!(
            runner_active,
            "the runner must still be active immediately before abort"
        );
        assert!(
            cleanup_result.is_ok(),
            "readiness-failure cleanup must complete"
        );
        assert_cancelled_join(final_join, "forced readiness failure");
        assert!(
            authority_pids_dead,
            "readiness-failure cleanup must remove both authority processes"
        );
    }

    #[tokio::test]
    async fn malformed_descendant_pid_publication_cleans_manifest_processes() {
        assert_invalid_publication_is_cleaned("malformed-desc").await;
    }

    #[tokio::test]
    async fn partial_descendant_pid_publication_cleans_manifest_processes() {
        assert_invalid_publication_is_cleaned("partial-desc").await;
    }

    #[tokio::test]
    async fn malformed_direct_pid_publication_cleans_manifest_processes() {
        assert_invalid_publication_is_cleaned("malformed-direct").await;
    }

    #[tokio::test]
    async fn both_malformed_pid_publications_clean_manifest_processes() {
        assert_invalid_publication_is_cleaned("both-malformed").await;
    }

    #[tokio::test]
    async fn missing_pid_publications_clean_manifest_processes() {
        assert_invalid_publication_is_cleaned("missing").await;
    }

    #[tokio::test]
    async fn duplicate_protocol_pids_have_one_cleanup_owner_and_no_second_signal() {
        let (directory, executable) = fixture_executable(publication_fixture_script());
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let launch_gate = fixture_path.join("launch.descendant");
        let completion_file = fixture_path.join("protocol.complete");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [
                    directory_for_task.to_str().expect("UTF-8 fixture path"),
                    "duplicate",
                ],
                std::time::Duration::from_secs(30),
            )
            .await
        });
        let mut cleanup = DescendantCleanup::new();
        let authority = arm_gated_cleanup_authority(
            &mut cleanup,
            &cleanup_direct_pid_file,
            &cleanup_descendant_pid_file,
            &cleanup_manifest,
            &launch_gate,
        )
        .await;
        let observed = authority.as_ref().ok().copied();
        let duplicate_deduplicated =
            observed.map(|(direct_pid, _)| cleanup.arm_unique_pid(direct_pid));
        let signals = cleanup.signal_recorder();
        let protocol_invalid = wait_for_invalid_protocol_publication(
            &direct_pid_file,
            &descendant_pid_file,
            &completion_file,
        )
        .await;
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let direct_signals = observed.map(|(direct_pid, _)| {
            signals
                .lock()
                .expect("signal recorder")
                .iter()
                .filter(|(pid, _)| *pid == direct_pid)
                .count()
        });

        assert!(
            authority.is_ok(),
            "independent cleanup authority must publish both PIDs"
        );
        assert!(duplicate_deduplicated.is_some_and(|result| result.is_ok_and(|armed| !armed)));
        assert!(protocol_invalid, "duplicate protocol PIDs must be rejected");
        assert!(runner_active, "the runner must remain active before abort");
        assert!(
            cleanup_result.is_ok(),
            "duplicate-publication cleanup must complete"
        );
        assert_cancelled_join(final_join, "duplicate-publication cleanup");
        assert_eq!(
            direct_signals,
            Some(1),
            "one numeric PID must retain exactly one cleanup signal path"
        );
        assert_eq!(cleanup.direct_child_pid, None);
        assert_eq!(cleanup.descendant_pid, None);
        drop(cleanup);
        assert_eq!(
            observed.map(|(direct_pid, _)| {
                signals
                    .lock()
                    .expect("signal recorder")
                    .iter()
                    .filter(|(pid, _)| *pid == direct_pid)
                    .count()
            }),
            direct_signals,
            "Drop must not signal a successfully cleaned duplicate PID"
        );
        assert!(
            observed.is_some_and(|(direct_pid, descendant_pid)| {
                !process_is_alive(direct_pid) && !process_is_alive(descendant_pid)
            }),
            "duplicate-publication cleanup must remove both manifest processes"
        );
    }

    #[tokio::test]
    async fn duplicate_protocol_pids_keep_independent_authority_when_manifest_is_malformed() {
        let (directory, executable) = fixture_executable(publication_fixture_script());
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let launch_gate = fixture_path.join("launch.descendant");
        let completion_file = fixture_path.join("protocol.complete");
        let executable_for_task = executable.clone();
        let directory_for_task = fixture_path.clone();
        let mut runner = tokio::spawn(async move {
            run_git_with_timeout(
                &executable_for_task,
                &directory_for_task,
                [
                    directory_for_task.to_str().expect("UTF-8 fixture path"),
                    "duplicate-manifest-malformed",
                ],
                std::time::Duration::from_secs(30),
            )
            .await
        });
        let mut cleanup = DescendantCleanup::new();
        let direct = arm_direct_cleanup_authority(&mut cleanup, &cleanup_direct_pid_file).await;
        let gate_released = direct.is_ok_and(process_is_alive)
            && release_descendant_launch_gate(&launch_gate).is_ok();
        let descendant = if gate_released {
            arm_descendant_cleanup_authority(&mut cleanup, &cleanup_descendant_pid_file).await
        } else {
            Err(())
        };
        let authority_pids = match (direct, descendant) {
            (Ok(direct), Ok(descendant)) => Some((direct, descendant)),
            _ => None,
        };
        let manifest_result = match authority_pids {
            Some((direct, descendant)) => {
                corroborate_cleanup_manifest(direct, descendant, &cleanup_manifest).await
            }
            None => Err(()),
        };
        let protocol_invalid = wait_for_invalid_protocol_publication(
            &direct_pid_file,
            &descendant_pid_file,
            &completion_file,
        )
        .await;
        let runner_active = !runner.is_finished();
        let (cleanup_result, final_join) = abort_cleanup_and_join(&mut runner, &mut cleanup).await;
        let both_dead = authority_pids.is_some_and(|(direct, descendant)| {
            !process_is_alive(direct) && !process_is_alive(descendant)
        });

        assert!(
            authority_pids.is_some(),
            "individual authority files must retain both real PIDs"
        );
        assert!(
            gate_released,
            "duplicate fixture must release the descendant only after direct authority is armed"
        );
        assert!(
            manifest_result.is_err(),
            "malformed manifest must be captured without erasing authority"
        );
        assert!(
            protocol_invalid,
            "duplicate ordinary protocol PIDs must be rejected"
        );
        assert!(
            runner_active,
            "runner must remain active before duplicate-manifest cleanup"
        );
        assert!(
            cleanup_result.is_ok(),
            "duplicate-manifest cleanup must complete"
        );
        assert_cancelled_join(final_join, "duplicate-manifest cleanup");
        assert!(
            both_dead,
            "duplicate-manifest cleanup must remove both real fixture processes"
        );
        assert_eq!(cleanup.direct_child_pid, None);
        assert_eq!(cleanup.descendant_pid, None);
    }

    #[tokio::test]
    async fn descendant_cleanup_disarms_verified_pids_before_drop() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nprintf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\nprintf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\" && mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\" || exit 73\nprintf '%s\\n' \"$descendant\" > \"$fixture/descendant.pid.tmp\" && mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" || exit 74\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let mut cleanup = DescendantCleanup::new();
        let mut child = Command::new(&executable.0)
            .arg(&fixture_path)
            .spawn()
            .expect("test helper child");
        let mut reaper = tokio::task::spawn_blocking(move || child.wait());
        let authority = arm_cleanup_authority(
            &mut cleanup,
            &cleanup_direct_pid_file,
            &cleanup_descendant_pid_file,
            &cleanup_manifest,
        )
        .await;
        let protocol_pids = wait_for_live_pids(&direct_pid_file, &descendant_pid_file).await;
        let startup_ok = matches!((&authority, &protocol_pids), (Ok(authority), Ok(protocol)) if authority == protocol);
        if !startup_ok {
            let cleanup_result = cleanup.finish().await;
            let reaper_result = await_reaper(&mut reaper).await;
            assert!(cleanup_result.is_ok(), "startup cleanup must complete");
            assert!(
                reaper_result.is_ok(),
                "startup reaper must reach terminal completion"
            );
            panic!("both test PIDs must be completely published and live after authority ownership exists");
        }
        let (direct_pid, descendant_pid) =
            authority.expect("startup was checked after cleanup ownership");
        let signals = cleanup.signal_recorder();

        let cleanup_result = cleanup.finish().await;
        let reaper_result = await_reaper(&mut reaper).await;
        assert!(cleanup_result.is_ok(), "verified process cleanup");
        assert!(reaper_result.is_ok(), "direct-child reaper");
        assert_eq!(cleanup.direct_child_pid, None);
        assert_eq!(cleanup.descendant_pid, None);
        let signals_after_finish = signals.lock().expect("signal recorder").len();
        drop(cleanup);
        assert_eq!(
            signals.lock().expect("signal recorder").len(),
            signals_after_finish,
            "Drop must not signal PIDs that successful cleanup disarmed"
        );
        assert!(
            !process_is_alive(direct_pid) && !process_is_alive(descendant_pid),
            "verified cleanup must remove both helper processes"
        );
    }

    async fn reaper_startup_publication_failure_is_cleaned(context: &str) {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nprintf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let mut cleanup = DescendantCleanup::new();
        let mut child = Command::new(&executable.0)
            .arg(&fixture_path)
            .spawn()
            .expect("test helper child");
        let mut reaper = tokio::task::spawn_blocking(move || child.wait());
        let authority = arm_cleanup_authority(
            &mut cleanup,
            &fixture_path.join("cleanup.direct.pid"),
            &fixture_path.join("cleanup.descendant.pid"),
            &fixture_path.join("cleanup.manifest"),
        )
        .await;
        let protocol = wait_for_live_pids(
            &fixture_path.join("direct.pid"),
            &fixture_path.join("descendant.pid"),
        )
        .await;
        let cleanup_result = cleanup.finish().await;
        let reaper_result = await_reaper(&mut reaper).await;
        let both_dead = authority.is_ok_and(|(direct, descendant)| {
            !process_is_alive(direct) && !process_is_alive(descendant)
        });

        assert!(
            authority.is_ok(),
            "{context}: cleanup authority must be established before protocol failure"
        );
        assert!(
            protocol.is_err(),
            "{context}: missing ordinary PID publication must be captured"
        );
        assert!(
            cleanup_result.is_ok(),
            "{context}: startup cleanup must complete"
        );
        assert!(
            reaper_result.is_ok(),
            "{context}: reaper must reach terminal completion"
        );
        assert!(
            both_dead,
            "{context}: startup publication failure must not leak a process"
        );
    }

    #[tokio::test]
    async fn no_second_signal_startup_publication_failure_cleans_and_joins_reaper() {
        reaper_startup_publication_failure_is_cleaned("no-second-signal startup").await;
    }

    #[tokio::test]
    async fn multi_deadline_startup_publication_failure_cleans_and_joins_reaper() {
        reaper_startup_publication_failure_is_cleaned("multi-deadline startup").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn join_deadline_preserves_runner_ownership_until_post_cleanup_join() {
        let (directory, executable) = fixture_executable(
            "#!/bin/sh\nfixture=$1\nprintf '%s\\n' \"$$\" > \"$fixture/cleanup.direct.pid.tmp\" && mv \"$fixture/cleanup.direct.pid.tmp\" \"$fixture/cleanup.direct.pid\" || exit 70\n/bin/sh -c 'while :; do :; done' &\ndescendant=$!\ntrap 'kill \"$descendant\" 2>/dev/null || true' EXIT\nprintf '%s\\n' \"$descendant\" > \"$fixture/cleanup.descendant.pid.tmp\" && mv \"$fixture/cleanup.descendant.pid.tmp\" \"$fixture/cleanup.descendant.pid\" || exit 71\nprintf '%s\\n%s\\n' \"$$\" \"$descendant\" > \"$fixture/cleanup.manifest.tmp\" && mv \"$fixture/cleanup.manifest.tmp\" \"$fixture/cleanup.manifest\" || exit 72\nprintf '%s\\n' \"$$\" > \"$fixture/direct.pid.tmp\" && mv \"$fixture/direct.pid.tmp\" \"$fixture/direct.pid\" || exit 73\nprintf '%s\\n' \"$descendant\" > \"$fixture/descendant.pid.tmp\" && mv \"$fixture/descendant.pid.tmp\" \"$fixture/descendant.pid\" || exit 74\ntrap - EXIT\nwhile :; do :; done\n",
        );
        let fixture_path = directory.path().to_path_buf();
        let direct_pid_file = fixture_path.join("direct.pid");
        let descendant_pid_file = fixture_path.join("descendant.pid");
        let cleanup_direct_pid_file = fixture_path.join("cleanup.direct.pid");
        let cleanup_descendant_pid_file = fixture_path.join("cleanup.descendant.pid");
        let cleanup_manifest = fixture_path.join("cleanup.manifest");
        let mut cleanup = DescendantCleanup::new();
        let mut child = Command::new(&executable.0)
            .arg(&fixture_path)
            .spawn()
            .expect("test helper child");
        let mut reaper = tokio::task::spawn_blocking(move || child.wait());
        let authority = arm_cleanup_authority(
            &mut cleanup,
            &cleanup_direct_pid_file,
            &cleanup_descendant_pid_file,
            &cleanup_manifest,
        )
        .await;
        let protocol_pids = wait_for_live_pids(&direct_pid_file, &descendant_pid_file).await;
        let startup_ok = matches!((&authority, &protocol_pids), (Ok(authority), Ok(protocol)) if authority == protocol);
        if !startup_ok {
            let cleanup_result = cleanup.finish().await;
            let reaper_result = await_reaper(&mut reaper).await;
            assert!(
                cleanup_result.is_ok(),
                "deadline-fixture startup cleanup must complete"
            );
            assert!(
                reaper_result.is_ok(),
                "deadline-fixture startup reaper must reach terminal completion"
            );
            panic!(
                "deadline fixture PID publication must succeed after authority ownership exists"
            );
        }
        let (direct_pid, descendant_pid) =
            authority.expect("startup was checked after cleanup ownership");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (first_release_sender, first_release_receiver) = mpsc::channel();
        let (second_entered_sender, second_entered_receiver) = mpsc::channel();
        let (second_release_sender, second_release_receiver) = mpsc::channel();
        let mut runner = tokio::spawn(async move {
            tokio::task::block_in_place(|| {
                let _ = entered_sender.send(());
                let _ = first_release_receiver.recv();
            });
            tokio::task::block_in_place(|| {
                let _ = second_entered_sender.send(());
                let _ = second_release_receiver.recv();
            });
            tokio::task::yield_now().await;
        });
        let entered = tokio::task::spawn_blocking(move || {
            entered_receiver.recv_timeout(std::time::Duration::from_secs(2))
        })
        .await
        .is_ok_and(|result| result.is_ok());
        if !entered {
            runner.abort();
            let first_join =
                join_runner_with_deadline(&mut runner, std::time::Duration::from_secs(2)).await;
            let cleanup_result = cleanup.finish().await;
            let _ = first_release_sender.send(());
            let _ = second_release_sender.send(());
            let final_join = match first_join {
                BoundedJoinOutcome::DeadlineExpired => {
                    join_runner_until_terminal(&mut runner).await
                }
                outcome => outcome,
            };
            assert!(
                await_reaper(&mut reaper).await.is_ok(),
                "deadline-fixture direct-child reaper"
            );
            assert!(
                cleanup_result.is_ok(),
                "deadline-fixture cleanup must complete"
            );
            assert_cancelled_join(final_join, "deadline-fixture startup cleanup");
            panic!("join-deadline fixture runner did not reach its state-local barrier");
        }

        let runner_active_before_abort = !runner.is_finished();
        runner.abort();
        let first_join =
            join_runner_with_deadline(&mut runner, std::time::Duration::from_millis(20)).await;
        let first_deadline_expired = matches!(first_join, BoundedJoinOutcome::DeadlineExpired);
        let cleanup_result = cleanup.finish().await;
        let _ = first_release_sender.send(());
        let (second_entered, second_deadline_expired, final_join) = match first_join {
            BoundedJoinOutcome::DeadlineExpired => {
                let second_entered = tokio::task::spawn_blocking(move || {
                    second_entered_receiver.recv_timeout(std::time::Duration::from_secs(2))
                })
                .await
                .is_ok_and(|result| result.is_ok());
                let second_join = if second_entered {
                    join_runner_with_deadline(&mut runner, std::time::Duration::from_millis(20))
                        .await
                } else {
                    BoundedJoinOutcome::DeadlineExpired
                };
                let second_deadline_expired =
                    matches!(second_join, BoundedJoinOutcome::DeadlineExpired);
                let _ = second_release_sender.send(());
                let final_join = match second_join {
                    BoundedJoinOutcome::DeadlineExpired => {
                        join_runner_until_terminal(&mut runner).await
                    }
                    BoundedJoinOutcome::Joined(result) => BoundedJoinOutcome::Joined(result),
                };
                (second_entered, second_deadline_expired, final_join)
            }
            BoundedJoinOutcome::Joined(result) => {
                let _ = second_release_sender.send(());
                (false, false, BoundedJoinOutcome::Joined(result))
            }
        };
        assert!(
            await_reaper(&mut reaper).await.is_ok(),
            "deadline-fixture direct-child reaper"
        );
        assert!(
            cleanup_result.is_ok(),
            "deadline-fixture cleanup must complete"
        );
        assert!(
            runner_active_before_abort,
            "join-deadline fixture runner must be active before abort"
        );
        assert!(
            first_deadline_expired,
            "the first bounded join must expire while the same runner handle remains owned"
        );
        assert!(
            second_entered,
            "the post-release barrier must hold the cancelled runner before its second deadline"
        );
        assert!(
            second_deadline_expired,
            "the second bounded join must expire while the same runner handle remains owned"
        );
        assert_cancelled_join(final_join, "post-deadline runner join");
        assert!(
            !process_is_alive(direct_pid) && !process_is_alive(descendant_pid),
            "deadline-fixture cleanup must remove both helper processes"
        );
    }

    #[test]
    fn unavailable_fingerprint_fails_closed() {
        assert!(matches!(
            repository_fingerprint(Path::new("/private/agent-sentinel-no-such-git-directory")),
            Err(GitError::FingerprintUnavailable)
        ));
    }

    #[test]
    fn existing_destination_canonicalization_failure_never_uses_raw_path() {
        let directory = tempfile::tempdir().expect("existing directory");
        assert!(matches!(
            expected_worktree_path_with(directory.path(), |_| {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            }),
            Err(GitError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }
    #[test]
    fn authoritative_inventory_parsers_reject_truncation_and_merge_unmerged_stages() {
        assert_eq!(
            parse_untracked_paths_z(b"nested/file\0name with space\0")
                .expect("strict untracked paths")
                .len(),
            2
        );
        assert!(parse_untracked_paths_z(b"unterminated").is_err());
        let conflicts = parse_unmerged_paths_z(
            b"100644 1111111111111111111111111111111111111111 1\tconflict\x00100644 2222222222222222222222222222222222222222 2\tconflict\x00100644 3333333333333333333333333333333333333333 3\tconflict\0",
        )
        .expect("strict unmerged records");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].1, ConflictKind::BothModified);
        assert!(parse_unmerged_paths_z(b"100644 bad 1\tconflict\0").is_err());
    }

    #[test]
    fn inventory_lookup_paths_apply_target_specific_windows_semantics() {
        for path in [
            r"tracked\..\outside",
            r"dir\file",
            r"..\outside",
            r"C:\outside",
            r"C:outside",
            r"\\server\share\file",
            r"\\?\C:\outside",
            "/absolute",
            "../outside",
            "a/../../outside",
        ] {
            assert!(validate_inventory_lookup_path(path, LookupPathSemantics::Windows).is_err());
        }
        assert!(
            validate_inventory_lookup_path("nested/file", LookupPathSemantics::Windows).is_ok()
        );
        assert!(validate_inventory_lookup_path("folder\\name", LookupPathSemantics::Unix).is_ok());
    }

    #[test]
    fn skip_worktree_records_are_strict_and_never_normalized_to_deletions() {
        assert!(!has_skip_worktree(b"H tracked.txt\0").expect("ordinary tracked record"));
        assert!(has_skip_worktree(b"S sparse/file.txt\0").expect("skip-worktree record"));
        for bytes in [
            b"S missing terminator".as_slice(),
            b"? unknown\0".as_slice(),
            b"S ../escape\0".as_slice(),
            b"S duplicate\0S duplicate\0".as_slice(),
        ] {
            assert!(has_skip_worktree(bytes).is_err());
        }
    }

    #[tokio::test]
    async fn inventory_environment_isolates_global_config_from_host_process() {
        let environment = InventoryGitEnvironment::create().expect("trusted empty environment");
        let home = environment.home().to_path_buf();
        let config = environment.global_config.clone();
        INVENTORY_GIT_ENVIRONMENT
            .scope(environment, async move {
                let scoped = INVENTORY_GIT_ENVIRONMENT
                    .try_with(|current| {
                        current.home() == home.as_path() && current.global_config == config
                    })
                    .expect("inventory environment scope");
                assert!(
                    scoped,
                    "inventory commands use the trusted empty home/config context"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn inventory_environment_ignores_a_hostile_global_config() {
        let fixture = tempfile::tempdir().expect("fixture");
        let hostile_home = fixture.path().join("hostile-home");
        fs::create_dir(&hostile_home).expect("hostile home");
        let outside = fixture.path().join("outside");
        let included = fixture.path().join("included-hostile.gitconfig");
        fs::write(
            &included,
            format!(
                "[core]\nworktree = {}\n[diff]\nexternal = /definitely/not/a-helper\n",
                outside.display()
            ),
        )
        .expect("hostile included config");
        fs::write(
            hostile_home.join(".gitconfig"),
            format!("[include]\npath = {}\n", included.display()),
        )
        .expect("hostile global config");
        let control = Command::new("git")
            .env("HOME", &hostile_home)
            .args(["config", "--global", "--includes", "--get", "core.worktree"])
            .output()
            .expect("unisolated control");
        assert!(
            control.status.success(),
            "control must load hostile included global config"
        );

        let environment = InventoryGitEnvironment::create().expect("trusted empty environment");
        let executable = TrustedGitExecutableResolver.resolve().expect("trusted git");
        let isolated = INVENTORY_GIT_ENVIRONMENT
            .scope(environment, async {
                run_git_with_limits(
                    &executable,
                    fixture.path(),
                    ["config", "--global", "--get", "core.worktree"],
                    GIT_TIMEOUT,
                    MAX_GIT_OUTPUT,
                    MAX_GIT_OUTPUT,
                    true,
                    false,
                )
                .await
            })
            .await
            .expect("isolated config query");
        assert!(
            !isolated.status.success() && isolated.stdout.is_empty(),
            "inventory isolation must not load an account-global core.worktree"
        );
    }

    #[tokio::test]
    async fn inventory_deadline_caps_commands_and_fails_closed_when_expired() {
        let now = std::time::Instant::now();
        let capped = INVENTORY_DEADLINE
            .scope(now + std::time::Duration::from_millis(5), async {
                cap_timeout_to_inventory_deadline(std::time::Duration::from_secs(3))
            })
            .await
            .expect("remaining operation budget");
        assert!(capped <= std::time::Duration::from_millis(5));
        let expired = INVENTORY_DEADLINE
            .scope(now, async {
                cap_timeout_to_inventory_deadline(GIT_TIMEOUT)
            })
            .await;
        assert!(matches!(expired, Err(GitError::TimedOut)));
    }

    #[tokio::test]
    async fn sparse_skip_worktree_fails_closed_before_filesystem_comparison() {
        let fixture = tempfile::tempdir().expect("fixture");
        let primary = fixture.path().join("primary");
        let worktree = fixture.path().join("linked");
        fs::create_dir(&primary).expect("primary directory");
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        fs::write(primary.join("sparse.txt"), "base\n").expect("tracked file");
        fixture_git(&primary, &["add", "sparse.txt"]);
        fixture_git(&primary, &["commit", "-m", "base"]);
        let base = resolve_exact_head(&primary).await.expect("base");
        add_detached_worktree(&primary, &worktree, &base)
            .await
            .expect("linked worktree");
        fixture_git(
            &worktree,
            &["update-index", "--skip-worktree", "sparse.txt"],
        );
        fs::remove_file(worktree.join("sparse.txt")).expect("intentionally absent sparse path");
        assert!(matches!(
            inspect_worktree_changes_at_base(&worktree, &base).await,
            Err(GitError::InventoryUnavailable)
        ));
    }

    #[tokio::test]
    async fn authoritative_inventory_pins_the_managed_worktree_against_local_core_worktree() {
        let fixture = tempfile::tempdir().expect("fixture");
        let primary = fixture.path().join("primary");
        let worktree = fixture.path().join("linked");
        let external = fixture.path().join("external");
        fs::create_dir(&primary).expect("primary directory");
        fs::create_dir(&external).expect("external directory");
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        fs::write(primary.join("tracked.txt"), "base\n").expect("tracked file");
        fixture_git(&primary, &["add", "tracked.txt"]);
        fixture_git(&primary, &["commit", "-m", "base"]);
        let base = resolve_exact_head(&primary).await.expect("base");
        add_detached_worktree(&primary, &worktree, &base)
            .await
            .expect("linked worktree");
        fs::write(external.join("tracked.txt"), "base\n").expect("external index match");
        fs::write(worktree.join("tracked.txt"), "managed modification\n")
            .expect("managed modification");
        fixture_git(
            &worktree,
            &[
                "config",
                "core.worktree",
                external.to_str().expect("external path"),
            ],
        );
        let inventory = inspect_worktree_changes_at_base(&worktree, &base)
            .await
            .expect("pinned inventory");
        assert!(inventory.iter().any(|file| {
            file.path.as_str() == "tracked.txt"
                && file.worktree_change == Some(ChangeKind::Modified)
        }));
    }

    #[tokio::test]
    async fn authoritative_inventory_merges_staged_unstaged_untracked_and_ignores_filters() {
        let fixture = tempfile::tempdir().expect("fixture");
        let primary = fixture.path().join("primary");
        let worktree = fixture.path().join("linked");
        fs::create_dir(&primary).expect("primary directory");
        fixture_git(&primary, &["init"]);
        fixture_git(&primary, &["config", "user.email", "tests@example.invalid"]);
        fixture_git(&primary, &["config", "user.name", "Tests"]);
        fs::write(primary.join("tracked.txt"), "base\n").expect("tracked file");
        fixture_git(&primary, &["add", "tracked.txt"]);
        fixture_git(&primary, &["commit", "-m", "base"]);
        let base = resolve_exact_head(&primary).await.expect("base");
        add_detached_worktree(&primary, &worktree, &base)
            .await
            .expect("linked worktree");
        fs::write(worktree.join("tracked.txt"), "staged\n").expect("staged change");
        fixture_git(&worktree, &["add", "tracked.txt"]);
        fs::write(worktree.join("tracked.txt"), "unstaged\n").expect("unstaged change");
        fs::write(worktree.join("untracked name.txt"), "new\n").expect("untracked file");
        let marker = fixture.path().join("filter-ran");
        let helper = fixture.path().join("filter-helper");
        fs::write(
            &helper,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .expect("helper");
        let mut permissions = fs::metadata(&helper)
            .expect("helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper, permissions).expect("helper permissions");
        fs::write(worktree.join(".gitattributes"), "*.txt filter=hostile\n").expect("attributes");
        fixture_git(
            &worktree,
            &[
                "config",
                "filter.hostile.clean",
                helper.to_str().expect("helper path"),
            ],
        );
        fixture_git(
            &worktree,
            &[
                "config",
                "filter.hostile.smudge",
                helper.to_str().expect("helper path"),
            ],
        );
        let inventory = inspect_worktree_changes_at_base(&worktree, &base)
            .await
            .expect("authoritative inventory");
        let tracked = inventory
            .iter()
            .find(|file| file.path.as_str() == "tracked.txt")
            .expect("tracked change");
        assert_eq!(tracked.index_change, Some(ChangeKind::Modified));
        assert_eq!(tracked.worktree_change, Some(ChangeKind::Modified));
        assert!(inventory
            .iter()
            .any(|file| file.path.as_str() == "untracked name.txt" && file.untracked));
        assert!(
            !marker.exists(),
            "safe inventory must not launch configured filters"
        );
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
