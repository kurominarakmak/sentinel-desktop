# Phase 3C-B1 — Trusted Per-File Diff Classification

Phase 3C-B1 and B1H are complete. The final holistic B1 re-review is clean,
with no remaining production P1/P2 findings. B1 activates the existing
internal, read-only classification step for exactly one
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
It collects three complete classification-evidence passes: authoritative
inventories A, B, and C must match, independently collected C1/C2 evidence
must match, and a final C3 pass from Inventory C must exactly match C2.
Evidence includes path/surface identity, numstat counts or binary state, mode
evidence, filter deferral, and final classification. This rejects a post-C2
content change that leaves coarse inventory state unchanged when C3 observes
different evidence. After C3, B1 reloads the persisted project and
worktree records, requires unchanged ownership/path/repository/fingerprint/base
identity and eligible lifecycle state, and performs final Git validation using
those refreshed records. No partial result is returned. Only
`ready` and `retained_dirty` rows are eligible. The existing ProjectId lock is
released on success, omission, and every error path; no cache, migration,
Tauri command, React control, or persistence is added.

The active bounded request-local `cfg(test)` regressions consume the
production-path observation stream rather than inferring coherence from the
final result. Final-validation events now bracket the refreshed acceptance
validation after Inventory C and C3: start is immediately before the real
repository/worktree validation using refreshed persisted records, and
completion is emitted only after those checks succeed. The former synthetic
outer event pair was removed. The stable companion observes matching C1/C2/C3 2-addition/1-deletion
evidence, InventoryCompleted(C), refreshed persisted-state acceptance, real
final validation start/completion, and only then the successful metadata-only
DTO. A post-C2 content-race regression changes a disposable file after C2
equality and proves C3's real numstat evidence differs and returns
`change_stale`; a late disposable Ready-to-Removing transition is observed by
the refreshed persisted-state check and fails before final-validation
completion. The transient test directly
observes matching A/B inventory observations, two independent real numstat
observations with differing C1/C2 counts, comparison failure, and
`change_stale`; fail-fast mismatch intentionally stops before C and its final
validation. A disposable validation-failure regression reaches the refreshed
final validation boundary and proves that failure emits no completion and
returns no DTO.

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

Phase 3C-B remains in progress. The prior B1 baseline remains in history and
the completed B1H boundary consumes only fresh authoritative inventory under
the same operation-scoped Git isolation and deadline context. The formerly
prerequisite-blocked tests and C3/lifecycle coherence regressions are active.
Phase 3C-B1H is **COMPLETE**; the final holistic B1 re-review is clean with no
remaining production P1/P2 findings. On
aarch64-apple-darwin, 178 Rust tests and 92 frontend tests passed, along with
`cargo fmt --all --check`, workspace Clippy with warnings denied, the frontend
production build, and the Tauri no-bundle build. Focused reviews found no
remaining P1/P2 issues. Phase 3C-B2, Phase 3C-C, and Phase 3C-D are **NOT
STARTED**.
