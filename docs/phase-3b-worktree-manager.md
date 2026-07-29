# Phase 3B — Sentinel-Owned Worktree Manager

Phase 3 in [PLAN.md](../PLAN.md) is Git isolation: a project registry, a worktree manager, and diffs. Phase 3B supplies the backend-owned worktree-manager foundation only; Phase 3C remains responsible for diff inspection and Phase 4 remains responsible for real-agent execution.

Managed worktrees have an Agent Sentinel `WorktreeId`, a registered `ProjectId`, an exact 40-character base commit, lifecycle state, timestamps, and internal repository identity/fingerprint. States are `creating`, `ready`, `removing`, `removed`, `failed`, `missing`, `identity_changed`, and `retained_dirty`. Internal locations and identity details never cross the Tauri DTO boundary.

The desktop derives the only worktree root from application data: `worktrees/<project-id>/<worktree-id>`. The frontend supplies only a project or worktree ID. Each configured existing component, including the exact persisted managed-worktree leaf, is checked with `symlink_metadata` before canonicalization or any path-following Git operation. Unix symlinks, Windows junctions/reparse points, files, missing/ambiguous leaves, and containment changes fail closed. `Path::exists()` is not used as ownership or absence proof: only a no-follow `NotFound` result means the configured leaf is missing. Dangling links, reparse points, files, permission failures, and other metadata errors remain unresolved. Missing root components are created one backend-named directory at a time and rechecked. The resulting canonical root is the containment anchor. Leaf validation is repeated before repository inspection, metadata lookup, and removal; unsafe replacements are retained as recovery-required and are never adopted or removed. A residual same-user filesystem TOCTOU remains between validation and Git invocation; the per-project lock, immediate revalidation, and post-verification minimize it, but this is not descriptor-relative atomic protection.

Creation requires a strong, strictly revalidated project and resolves `HEAD^{commit}` to a full OID. It persists `creating`, then invokes the Phase 3A trusted Git boundary with exactly `git worktree add --detach <backend-owned-path> <exact-oid>`. Post-create inspection verifies linked-worktree status, identity/fingerprint, canonical path, and exact HEAD before the row becomes `ready`. Per-project in-memory serialization prevents overlapping create/remove Git metadata changes without holding a database transaction across Git awaits.

Historical Phase 3B removal accepted a stored ready worktree ID, verified the
backend-generated containment relationship, no-follow leaf, linked identity,
exact metadata entry, and stock-status cleanliness, then performed a
non-force `git worktree remove <stored-path>` only after revalidation. Git
worktree-list output remains parsed as strict bounded porcelain-v1
NUL-delimited records: complete records require exactly one `worktree` field,
valid singleton fields, valid OIDs, and no contradictory branch/detached state.
Worktree paths must be absolute, lexically normalized native paths with no `.`
or `..` components; aliases and normalized duplicates fail closed. Windows
comparison normalizes supported drive/UNC separators and case while rejecting
device and ambiguous prefixes. Canonicalization failures never fall back to raw
paths. The compatibility policy rejects unknown fields rather than silently
ignoring them. Metadata command, parse, normalization, canonicalization, or
ambiguity failure is distinct from a successfully parsed exact-entry absence.
`removed` requires both a no-follow-proven missing leaf and that successful
exact absence result. Partial outcomes remain registered as recovery-required
failures and continue blocking project unregister; reconciliation never prunes
or force-removes them. Dirty worktrees become `retained_dirty`; no reset,
clean, stash, checkout, force remove, or deletion occurs. Native Windows
reparse validation is target-gated and was not executed locally.

### Current AH1 security override

While Phase 3C-A is reopened, stock-status cleanliness verification is retired
from production because repository-configured filters can execute through that
path. A removal request therefore receives `CleanlinessUnavailable`; unknown
cleanliness is never treated as clean, and automatic clean removal does not
proceed. The managed worktree remains present and the existing typed lifecycle
state machine retains or transitions the row to `RetainedDirty` (`retained_dirty`). No force
removal, direct filesystem deletion, reset, clean, stash, checkout, or content
mutation occurs. Complete safe automatic removal is blocked pending AH2.

The `managed_worktrees` migration has a restricting foreign key to projects, unique backend location, typed lifecycle constraint, and deterministic project listing. The explicit reconciliation command never adopts or deletes unknown directories: it promotes a fully verified interrupted `creating` row, marks missing/identity-changed paths safely, and records an interrupted removal as removed only when its directory is absent. If a `creating` or `ready` leaf initially appears to be a real directory but full ownership validation fails, reconciliation conditionally records a durable `failed`/`recovery_required` state instead of leaving the row active. These expected-current-state transitions preserve newer lifecycle outcomes, leave the row registered and unregister-blocking, and allow independent rows to continue reconciling. Desktop integration fixtures invoke the actual reconciliation orchestration with deterministic canonicalization failure for both `creating` and `ready`, verify guarded stale-transition preservation, reopen persistence and unregister blocking, and distinguish cross-project continuation from same-project lock release: after a failed row releases its project operation lock, a deterministically later row for that same ProjectId reconciles through the shared lock registry. Historical focused fixtures also proved detached exact-commit creation, two-worktree isolation, primary-directory protection, dirty retention, clean removal, and persistence/reopen lifecycle state; the AH1 override above supersedes current clean-removal capability. Linux registration remains fail-closed under the Phase 3A strong-fingerprint policy. Windows-native fingerprint limitations remain as documented in Phase 3A.

Deferred: richer recovery diagnostics, Phase 3C diff inspection, and all Codex/Claude execution. No production repository, primary worktree, or user-owned worktree is mutated by this slice beyond Git's administrative metadata for a verified Sentinel-owned detached worktree.

Phase 3B completion evidence on aarch64-apple-darwin: 115 Rust tests and 92 frontend tests passed; `cargo fmt --all --check`, workspace Clippy with warnings denied, the frontend production build, and the Tauri no-bundle build passed; and focused reviews found no remaining P1/P2 issues. Production reconciliation integration tests exercise Creating and Ready recovery transitions, stale guarded transitions, cross-project continuation, and same-project lock release through one `DesktopState` and its real lock registry. Windows reparse-point code and native tests remain target-gated and were not run locally. Linux strong-fingerprint registration remains intentionally fail-closed. The residual same-user filesystem TOCTOU is documented above; descriptor-relative atomic path protection is not claimed. No production repository or user-owned worktree was mutated, and no sentinel-probe or real Codex/Claude product-agent execution occurred.
