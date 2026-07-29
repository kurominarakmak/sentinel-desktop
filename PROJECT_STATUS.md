# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 3 — IN PROGRESS (Phase 2, Phase 3A, and Phase 3B complete; Phase 3C-A reopened and Phase 3C-B1 blocked; Phase 3C-B2 not started)**

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

Phase 3 is **IN PROGRESS**. Phase 3A and Phase 3B remain **COMPLETE**. Phase 3C is **IN PROGRESS**. Phase 3C-A is **REOPENED**: the former porcelain-status inventory and Phase 3B stock-status clean check can execute repository-configured clean/process filters. AH1 is **COMPLETE** and freezes the production fail-closed boundary: it removes those production paths, returns fixed unavailable outcomes while the filter-free replacement is incomplete, and preserves only marker-proven staged raw metadata foundation work. Corrected disposable attribution confirms the direct staged `diff-index` control and its sentinel-git wrapper leave clean/process markers absent; the prior staged marker was created by a test-only stock-status snapshot after configuration, not by `diff-index`. A disposable macOS/aarch64 runner audit confirms timeout kills/reaps the direct child but not its independently spawned descendant; the test cleans that descendant, and the open generic containment issue does not block AH1 because production prevents repository-selected helpers from launching. The Unix process-group supervisor is test-only; it is not a production containment feature, and production runner containment remains direct-child-only. Phase 3C-AH2 is about to begin and must supply the complete authoritative filter-free inventory. Until its holistic review, staged `diff-index` remains non-authoritative and Phase 3C-B is **IN PROGRESS but BLOCKED**. Phase 3C-B1 remains committed at `c7a18cd` but is **REOPENED/BLOCKED**; B1H and B2 are **NOT STARTED**. Phase 3C-C and 3C-D are **NOT STARTED**. Phase 4 is **NOT STARTED**. Windows reparse/path tests remain target-gated; Linux strong-fingerprint registration remains fail-closed; same-user filesystem TOCTOU and the absence of descriptor-relative atomic protection remain documented. No production repository or user-owned worktree was inspected or mutated, and no sentinel-probe or real Codex/Claude product-agent execution occurred.

Phase 0 Codex validation remains **PASS**. Claude Code remains **DEFERRED**, not failed; cross-agent validation is incomplete. See the preserved [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md).

Interactive macOS end-to-end verification completed through the real desktop application. Real Codex and Claude execution remain disabled; Phase 0 Codex validation remains feasibility-only and Claude remains deferred.
