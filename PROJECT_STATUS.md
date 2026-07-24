# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 1 — Foundation complete**

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

Phase 1 establishes the verified Rust workspace, Tauri 2 desktop shell, React/TypeScript/Vite frontend, local validation scripts, non-interactive CI, and a manifest-level dependency-boundary check that keeps reusable crates independent from Tauri. The macOS no-bundle Tauri build succeeds.

Phase 0 Codex validation remains **PASS**. Claude Code remains **DEFERRED**, not failed; cross-agent validation is incomplete. See the preserved [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md). Interactive macOS tray, shortcut, focus, and close-to-hide checks remain manual and are not claimed as completed. Phase 2 has not begun.
