# Phase 7 Authoritative Execution Contract

## Objective and boundary

[PLAN.md](../PLAN.md) defines Phase 7 as **Profiles, custom policy, audit queue**: sensitive actions cannot proceed without a valid decision. Phase 7 adds backend-authoritative approval decisions only. Phase 8 drift findings and evidence evaluation remain excluded.

## Authority and model

Profiles are Safe, Balanced, Autonomous, and Custom. Built-in hard denials (merge, push, force-push, silent CLI installation, unrestricted permissions) override every profile and cannot be approved. Every sensitive action becomes a bounded persisted request tied to exact ProjectId, WorktreeId, task key, and opaque backend reference. A decision is allow-once, allow-for-task, or deny; it is versioned, immutable after resolution, audited, and never becomes a global allow.

## Scope

1. Persist bounded policy profiles, approval requests, decisions, and audit timestamps with constraints/indexes and ownership-scoped lookup.
2. Enforce `pending → allowed_once|allowed_for_task|denied` transactionally with optimistic versioning; duplicate/stale/cross-owner decisions fail closed.
3. Expose minimal redacted Rust/Tauri/TypeScript DTOs for list/query/decide. No raw command, path, prompt, event, process, or policy file crosses the bridge.
4. Present a queue with backend-confirmed resolution and stale/unmount safety.

Phase 7 does not execute a requested action, change process behavior, create custom TOML parsing, detect drift, run evidence, alter OS posture, or remove worktrees. Restart preserves requests; no process is reconnected.

## Completion

All summaries are bounded/redacted; public lookup requires exact ownership and opaque reference. Temporary SQLite tests cover hard denial, ownership, stale versions, duplicate decisions, redaction, and frontend ordering, followed by all prior regressions. Phase 8 remains not started.
