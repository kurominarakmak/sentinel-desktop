# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 4 — COMPLETE / CLEAN. Phase 5 is NOT STARTED.**

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

Phase 3 is complete at `630a54f` and checkpointed by the immutable `phase-3-complete` tag. Phase 4 is **COMPLETE / CLEAN** under [its authoritative execution contract](docs/phase-4-authoritative-execution-contract.md): it adds bounded private Codex structured execution, a persisted ownership-bound run context with opaque public references, redacted start/query/cancel DTOs, direct-child cancellation, and conservative restart interruption reconciliation. App Server is explicitly unavailable/experimental, automatic worktree removal remains disabled, and Phase 5 is **NOT STARTED**. Phase 3 exact-byte identity, isolation, bounded worktree operations, and redacted public boundaries remain unchanged.

Phase 0 Codex validation remains **PASS**. Claude Code remains **DEFERRED**, not failed; cross-agent validation is incomplete. See the preserved [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md).

Interactive macOS end-to-end verification completed through the real desktop application. Real Codex and Claude execution remain disabled; Phase 0 Codex validation remains feasibility-only and Claude remains deferred.
