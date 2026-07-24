# Agent Sentinel Project Status

Agent Sentinel is an open-source, local-first desktop application for supervising Codex and Claude Code tasks in isolated Git worktrees, with explicit approvals, deterministic drift detection, and evidence-based completion.

**Current phase: Phase 0 — Complete for the currently available agent environment**

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

Phase 0 validated the available Codex path: structured events, session capture and resume, exact output and unexpected-file validation, and cancellation. Claude Code is **deferred, not failed**, because no active authenticated Claude session is currently available. Cross-agent validation is therefore not complete.

See the current [Phase 0 feasibility report](docs/spikes/phase-0-feasibility.md). This status assumes the existing documented macOS interactive checklist remains the required verification record; no Phase 1 implementation has begun.
