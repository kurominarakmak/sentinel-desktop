# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 3 — IN PROGRESS (Phase 2, Phase 3A, Phase 3B, Phase 3C-A, and Phase 3C-B1 complete; Phase 3C-B2 not started)**

## Documentation Navigation

- [Operational implementation roadmap](PLAN.md)
- [Contributor implementation plan](docs/implementation-plan.md)
- [Product requirements](docs/product-requirements.md)
- [Architecture](docs/architecture.md)
- [Agent adapter contract](docs/agent-adapter-contract.md)
- [Security model](docs/security-model.md) and [threat model](docs/threat-model.md)
- [Contributing](docs/contributing.md)

Dedicated files under `docs/` are authoritative for their technical domains.

## Immediate Next Action

Phase 1 established the verified Rust workspace, Tauri 2 desktop shell, React/TypeScript/Vite frontend, local validation scripts, non-interactive CI, and the dependency boundary that keeps reusable crates independent from Tauri.

Phase 2A completed the typed run domain, validated state machine, and SQLite persistence layer.

Phase 2B completed the Tauri-independent fake-agent runtime, ordered event persistence, live event bus, process-group cancellation, storage-failure cleanup, and deterministic terminal-outcome resolution.

Phase 2C1 is **COMPLETE**: it delivered the typed Tauri desktop bridge, trusted fake-agent sidecar preparation and resolution, safe command DTOs, live-event forwarding with SQLite replay and lag recovery, and temporary Phase 0 frontend compatibility.

Phase 2C2 is **COMPLETE**: it delivered typed React/Tauri bridge integration; fake-run submission and history; live/persisted event reconciliation; lifecycle and terminal freshness protection; runtime-authoritative cancellation capability with RunId-scoped cancellation operations; scoped loading, error, and retry states; runtime-environment readiness; safe allowlisted event presentation; focus/listener lifecycle handling; error-boundary/bootstrap fallback; and comprehensive race/integration tests. Final evidence: 92 frontend tests passed, 76 Rust workspace tests passed in the final full Rust validation, frontend production and Tauri no-bundle builds passed, `cargo fmt --all --check` passed, workspace Clippy with warnings denied passed, and final review found no remaining P1/P2 issues.

Phase 2C3 is **COMPLETE**: manual macOS Tauri desktop acceptance verified success, failure, delayed child-process cancellation, burst/replay ordering, malformed/incomplete protocol handling, SQLite persistence across restart, detached persisted-run capability, and Escape/reopen focus preservation.

Phase 2 is **COMPLETE**. Its exit criterion is satisfied: a deterministic fake task traverses the desktop UI, typed Tauri bridge, Rust orchestrator and state machine, SQLite persistence, live/persisted event recovery, history, terminal handling, and cancellation.

Phase 3 is **IN PROGRESS**. Phase 3A is **COMPLETE**: it delivers the trusted local project registry, read-only Git validation, duplicate identity protection, protected-primary-worktree metadata, SQLite persistence, and typed backend commands. Phase 3B is **COMPLETE**: it delivers Sentinel-owned detached exact-commit worktree creation, lifecycle persistence, strict no-follow ownership verification, normalized Git worktree metadata parsing, clean-only removal with dirty retention, and durable reconciliation recovery. Final evidence: 115 Rust tests and 92 frontend tests passed on aarch64-apple-darwin; `cargo fmt --all --check`, workspace Clippy with warnings denied, the frontend production build, and the Tauri no-bundle build passed; focused reviews found no remaining P1/P2 issues; and production reconciliation tests cover Creating/Ready recovery, stale guards, and same-project lock release. Phase 3C is **IN PROGRESS**. Phase 3C-A is **COMPLETE**: it delivers backend-owned, complete-or-error read-only managed-worktree change inventory through fixed porcelain-v2 status; strict typed parsing of staged, unstaged, untracked, conflict, mode, and submodule metadata; cross-platform-safe relative paths including leading-backslash rejection; and full ownership, lifecycle, lock, and post-inspection validation. Final evidence: 124 Rust tests and 92 frontend tests passed on aarch64-apple-darwin; `cargo fmt --all --check`, workspace Clippy with warnings denied, frontend production build, and Tauri no-bundle build passed; focused reviews found no remaining P1/P2 issues; disposable production-service tests prove managed and primary index bytes, HEAD, branch, status, and contents remain unchanged; and configured repository fsmonitor helpers do not execute. Phase 3C-B is **IN PROGRESS**. Phase 3C-B1 is **COMPLETE**: it provides internal, one-path trusted numstat classification against a fresh locked Phase 3C-A inventory, separately comparing persisted base commit to index and index to worktree. Strict complete-or-error metadata classification returns only bounded text line counts or metadata-only binary, mode, symlink, submodule, untracked, conflict, and unsupported outcomes. Fixed literal-pathspec and helper-suppression controls prevent repository-configured helpers from running; no patch text, persistence, Tauri command, React UI, or agent integration was added. Final evidence: 132 Rust tests and 92 frontend tests passed on aarch64-apple-darwin; `cargo fmt --all --check`, workspace Clippy with warnings denied, frontend production build, and Tauri no-bundle build passed; focused reviews found no remaining P1/P2 issues; disposable production-service tests prove managed and primary HEAD, branch, index, status, and contents remain unchanged, and a deterministic post-numstat typed `ready`-to-`removing` transition rejects the stale classification, preserves the newer state, and releases the same ProjectId lock. Phase 3C-B2, Phase 3C-C, and Phase 3C-D are **NOT STARTED**. Full Phase 3 remains incomplete. Windows reparse-point and path tests remain target-gated and were not executed locally; Linux strong-fingerprint registration remains intentionally fail-closed; the documented same-user filesystem TOCTOU remains, and descriptor-relative atomic path protection is not claimed. No production repository or user-owned worktree was inspected or mutated, and no sentinel-probe or real Codex/Claude product-agent execution occurred. Phase 4 Codex integration is **NOT STARTED**.

Phase 0 Codex validation remains **PASS**. Claude Code remains **DEFERRED**, not failed; cross-agent validation is incomplete. See the preserved [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md).

Interactive macOS end-to-end verification completed through the real desktop application. Real Codex and Claude execution remain disabled; Phase 0 Codex validation remains feasibility-only and Claude remains deferred.
