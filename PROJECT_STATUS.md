# Agent Sentinel Project Status

## Authority

V3 product, architecture, and workflow decisions are respectively defined by [V3 product scope](docs/v3-product-scope.md), [V3 target architecture](docs/v3-target-architecture.md), and [V3 workflow state machine](docs/v3-workflow-state-machine.md). [PLAN.md](PLAN.md) is the V3 delivery order. V1 documents and phase contracts are preserved historical records of the existing implementation; where they conflict with V3 future planning, the V3 source documents govern.

## Current status

### PRESENT

- V3 Phase 1 durable foundation in `sentinel-core`: provider-neutral task state, explicit transition guards, append-only normalized events, and additive SQLite migration `0010_v3_workflow_foundation.sql`.
- Durable opaque provider-session metadata, approvals, validation results, review findings, repair rounds, and artifact metadata. The Rust repository is the only write authority exposed by this phase.
- V3 restart posture: unfinished non-draft tasks restore as `Recovering` with `NoLiveProcessAssumed`; active sessions become `RecoveryRequired`. Completion is never inferred from a missing external process, and V1 rows remain intact.
- macOS Tauri 2 + React + Rust application foundation with SQLite persistence.
- Reusable `sentinel-core`, `sentinel-runtime`, `sentinel-process`, `sentinel-git`, and `sentinel-agent-api` crates, plus fake-agent fixtures and tests.
- Tray application, `Command+Shift+Space` global hotkey, hidden floating prompt, and status/settings window routes.
- Existing project/worktree, process-supervision, normalized-event, approval, drift, evidence, and restart-recovery foundations with automated regression coverage.

### PARTIAL

- Codex App Server has an isolated native JSON-RPC transport with a locked child writer, one stdout dispatcher, bounded request registry, ordered notification worker, exit fan-out, and deterministic shutdown. It remains a PARTIAL Phase 2 adapter: malformed-frame recovery, capability negotiation, production protocol-fixture breadth, and operational/recovery hardening remain before enablement.
- Codex has bounded structured-execution/App Server feasibility interfaces, but not a supported V3 session lifecycle.
- Claude Code has bounded stream/session records and fixtures, but not a supported V3 session lifecycle.
- Git worktrees, diffs, approvals, validation/evidence, and recovery exist as V1 foundations, but are not composed into V3 worktree transactions, final-action approval, or review/repair orchestration.
- The prompt, tray, hotkey, status, and settings surfaces exist, but are not yet the V3 Quick Prompt, Attention Widget, and Task Detail experience.

### PLANNED

- Supported Codex and Claude Code adapters with start, stream, cancel, resume/reconcile, and recovery capability reporting.
- Supervisor-owned worktree transaction lifecycle; configurable build/lint/test profiles; read-only review; confirmed-blocker repair loops; and human-gated final actions.
- Attention Widget, focus restoration, complete Task Detail, security/recovery/provenance hardening, and final UI redesign.

### DEFERRED

- Cloud synchronization, teams, remote execution, automatic pull requests, IDE/browser plugins, mobile clients, daemon extraction, autonomous commit/merge/push, and automatic worktree deletion.
- Optional adapters/packs: ACP, OpenCode, Gemini CLI, Hermes, and quant-development workflows.

## Working-tree note

Existing uncommitted desktop changes were present before this documentation reconciliation. They are outside this documentation-only commit and remain untouched.

## Next action

Phase 1 of [PLAN.md](PLAN.md) is complete. The next authorized implementation task is Phase 2 only: add a supported Codex adapter that feeds the supervisor-owned V3 store without changing active V1 user behavior. No later V3 phase has started.
