# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 2 — In progress (Phase 2A, Phase 2B, and Phase 2C1 complete)**

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

Phase 2C1 completed the typed Tauri desktop bridge, trusted fake-agent sidecar preparation and resolution, safe command DTOs, live-event forwarding with SQLite replay and lag recovery, and temporary Phase 0 frontend compatibility.

The immediate next action is Phase 2C2: replace the Phase 0 React screen with the typed fake-run task, active-run, event-stream, cancellation, and persisted-history UI.

Phase 2C3 will then perform the complete live desktop acceptance flow and final Phase 2 verification.

Phase 0 Codex validation remains **PASS**. Claude Code remains **DEFERRED**, not failed; cross-agent validation is incomplete. See the preserved [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md).

Interactive macOS end-to-end verification of the new Phase 2 UI remains pending and must not be claimed as complete. Phase 2 is not complete until a deterministic fake task traverses React → Tauri → runtime → SQLite → React successfully.
