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

### COMPLETE WITH KNOWN LIMITATIONS

- V3 Phase 2 native Codex adapter: the Sentinel-owned desktop App Server manager lazily owns one process, starts and resumes task-scoped threads, streams normalized events into durable V3 state, persists thread/turn/session metadata, and supports cancellation without treating `turn/completed` as Sentinel task completion. Capability negotiation and deterministic protocol/lifecycle fixtures pass.
- Real authenticated local validation in a disposable Git repository passed for App Server initialization, thread/turn start, normalized durable streaming, and clean-shutdown thread resume. Deterministic in-flight interruption and crash/recreation coverage pass.
- A real turn could not be held in flight reliably for interruption, and a real `thread/resume` after abrupt owned-App-Server death currently returns `RpcError`; both are documented provider limitations, deferred to Phase 8 recovery hardening rather than Phase 2 blockers.

### COMPLETE WITH AUTH-LIMITED VALIDATION

- V3 Phase 3 native Claude Code adapter: Sentinel owns a supported non-interactive `--print --output-format stream-json` child, persists the opaque Claude session ID, streams normalized V3 events, cancels only its owned process, resumes only Sentinel-owned session records, and reconciles persisted records after restart. Executable/version detection, supported stream-json/resume interfaces, and real CLI JSON framing were verified. Deterministic fake-Claude start/stream/cancel/resume/malformed/exit/restart fixtures pass. The local `claude 2.1.201` binary was not logged in, so authenticated provider lifecycle validation is deferred to Phase 8 or when credentials become available; no login or account change was attempted.

### PARTIAL
- V3 Phase 4 is **PARTIAL**. Phase 4A1 creates one task-namespaced branch and
  worktree from a pinned base commit, persists its ready metadata, and leaves
  the primary working tree untouched.
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

Phase 2 is complete with known provider limitations recorded in
[Phase 2D validation](docs/v3-phase2d-validation.md). Phase 3 is COMPLETE WITH
AUTH-LIMITED VALIDATION; authenticated Claude smoke/reconciliation is deferred
to Phase 8 or available credentials. Phase 4 remains PARTIAL: 4A2 conflict,
retry/idempotency, and restart-recovery handling; 4B diff and merge preparation;
and 4C approval-gated discard, rollback, cleanup, and retention remain.
