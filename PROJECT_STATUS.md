# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 0 — Technical Feasibility**

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

Run the Phase 0 feasibility spikes: one Tauri tray process, a fake agent, Codex and Claude Code structured CLI tasks, an isolated worktree, normalized events, and reliable cancellation.

See the current [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md). Phase 0 remains in progress until its mandatory exit criteria are directly tested.
