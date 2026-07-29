use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Maximum bytes accepted from the read-only Phase 3C-A status operation.
pub const MAX_STATUS_STDOUT: usize = 256 * 1024;
pub const MAX_STATUS_STDERR: usize = 16 * 1024;
pub const MAX_STATUS_RECORDS: usize = 1_000;
pub const MAX_REPOSITORY_RELATIVE_PATH_BYTES: usize = 4 * 1024;
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
    let inspection = inspect_repository(worktree).await?;
    if inspection.is_primary {
        return Err(GitError::MetadataInvalid);
    }
    let executable = TrustedGitExecutableResolver.resolve()?;
    let output = run_git(
        &executable,
        &inspection.repository_root,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::MetadataInvalid);
    }
    Ok(output.stdout.is_empty())
}

/// Returns a complete, read-only porcelain-v2 inventory for a caller-verified
/// linked worktree.  The caller owns exact managed-leaf validation; this crate
/// deliberately accepts only a backend-provided directory and fixed arguments.
pub async fn inspect_worktree_changes(worktree: &Path) -> Result<Vec<ChangedFile>, GitError> {
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
fn parse_repository_relative_path(value: &[u8]) -> Result<RepositoryRelativePath, GitError> {
    if value.is_empty() || value.len() > MAX_REPOSITORY_RELATIVE_PATH_BYTES {
        return Err(GitError::StatusMalformed);
    }
    let text = std::str::from_utf8(value).map_err(|_| GitError::StatusMalformed)?;
    // A leading backslash is rooted on Windows, including device and verbatim
    // namespaces.  Reject it on every host so this bridge-safe relative-path
    // type can never be reinterpreted as absolute by a future Windows client.
    // Non-leading backslashes remain ordinary filename bytes under the
    // documented Unix-safe policy.
    if text.contains('\0')
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
    Ok(RepositoryRelativePath(text.to_owned()))
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

#[allow(clippy::too_many_arguments)]
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
    if read_only {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    }
    let mut child = command.spawn().map_err(|_| GitError::GitDiscoveryFailed)?;
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
    use std::os::unix::fs::PermissionsExt;

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
        let inventory = inspect_worktree_changes(&worktree)
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
