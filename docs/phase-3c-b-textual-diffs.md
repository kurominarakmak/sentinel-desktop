# Phase 3C-B1 — Trusted Per-File Diff Classification

Phase 3C-B1 is reopened and blocked by the Phase 3C-A filter-free inventory
prerequisite. It will remain unavailable until AH2 provides a complete safe
inventory. Phase 3C-B1 is an internal, read-only classification step for exactly one
repository-relative path in a verified Sentinel-managed worktree. It does not
return a textual patch or any file content. Phase 3C-B2 remains responsible for
any future bounded textual extraction.

The internal caller supplies a typed `WorktreeId` and
`RepositoryRelativePath`, but the path is not authoritative. Under the existing
`ProjectId` operation lock, the service recomputes the complete Phase 3C-A
inventory and accepts only an exact matching backend-generated path. It then
compares staged state as persisted immutable base commit to current index, and
unstaged state as current index to working tree. There is no combined patch.

The only path-following metadata operations are fixed trusted commands
equivalent to `git --no-optional-locks --literal-pathspecs -c
core.fsmonitor=false -c core.untrackedCache=false -c color.ui=false diff
--numstat -z --no-ext-diff --no-textconv --no-color --no-renames`, adding
`--cached <persisted-base-commit>` for staged metadata. The selected validated
path is supplied as one argument after `--`. The child also receives
`GIT_OPTIONAL_LOCKS=0` and `GIT_LITERAL_PATHSPECS=1`, along with the inherited
trusted-runner protections: clean environment, null stdin, disabled prompts and
pagers, bounded concurrent output capture, timeout, child kill/reap, and no
shell. Repository external diff, textconv, pager, and fsmonitor configuration
cannot alter this operation.

Each metadata command is limited to three seconds, 32 KiB stdout, and 16 KiB
stderr. One record only is accepted for a selected section. Numstat parsing is
byte-oriented and complete-or-error: it accepts only empty output or one
NUL-terminated `<added> TAB <deleted> TAB <exact path>` record. Numeric records
become bounded `TextEligible` addition/deletion metadata; `- TAB -` becomes
`Binary`. Any malformed, mismatched, oversized, multiple, or partial output
fails the entire request without a partial classification.

Staged and unstaged sections are separately classified as `NotApplicable`,
`TextEligible`, `Binary`, `ModeOnly`, `SymlinkMetadataOnly`,
`SubmoduleMetadataOnly`, `UntrackedContentDeferred`,
`ConflictContentDeferred`, or `UnsupportedType`. Untracked paths, conflicts,
symlinks, and gitlinks are intentionally metadata-only: B1 neither reads nor
returns untracked content, symlink targets, submodule data, conflict stages,
binary bytes, or unified patches. Mode-only regular-file transitions are
accepted only when inventory modes prove the transition and numstat has no line
delta (the empty form or Git's `0`/`0` record).

Before inventory and immediately before each metadata command, the service
revalidates the exact no-follow managed leaf, trusted-root containment,
linked-worktree/non-primary ownership, project identity and fingerprint, exact
metadata presence, detached persisted base commit, and lifecycle eligibility.
After classification it repeats validation and rejects changed rows. Only
`ready` and `retained_dirty` rows are eligible. The existing ProjectId lock is
released on success, omission, and every error path; no cache, migration,
Tauri command, React control, or persistence is added.

Tests use disposable repositories and managed worktrees. They cover strict
numstat parsing, literal pathspec-looking names, fixed arguments/environment,
staged-plus-unstaged service classification, suppression of configured
fsmonitor, external-diff, diff-driver, and textconv helpers, linked and primary
index/HEAD/status/content preservation, and project-lock reacquisition.
The final post-command validation is covered by a deterministic
production-service test: after real numstat completes, a state-local test hook
pauses B1 before its final locked inventory/row validation; the same disposable
row is then moved through the typed conditional lifecycle API from `ready` to
`removing`. On resume, B1 returns no provisional classification, preserves the
newer lifecycle state, and releases the same `ProjectId` lock for reacquisition.
Windows-native path and reparse testing remains target-gated; Linux strong
fingerprint registration remains fail-closed. The documented same-user
filesystem TOCTOU boundary remains, and descriptor-relative atomic protection
is not claimed.

Phase 3C-B remains in progress. The prior B1 commit remains in history but is
not currently available: stock-status inventory can execute repository filters.
Phase 3C-B1H is deferred pending AH2. Previously recorded evidence was for the
now-retired prerequisite and does not close this execution boundary. On
aarch64-apple-darwin, 132 Rust tests and 92 frontend tests passed, along with
`cargo fmt --all --check`, workspace Clippy with warnings denied, the frontend
production build, and the Tauri no-bundle build. Focused reviews found no
remaining P1/P2 issues. Phase 3C-B2, Phase 3C-C, and Phase 3C-D are not
started.
