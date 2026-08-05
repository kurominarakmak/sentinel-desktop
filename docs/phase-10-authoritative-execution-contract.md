# Phase 10: Reliability authoritative execution contract

## Objective and exit criterion

PLAN.md defines Phase 10 as **Reliability — Recovery, process cleanup, logs,
migrations**. Its exit criterion is: **Restart preserves history and never
silently deletes work.**

Phase 10 is an integration and hardening boundary over the already persisted
runtime, managed-worktree, approval, Drift Guardian, and Evidence Gate
systems. It is not a release, packaging, notification, remediation, or runtime
approval-delivery phase; those capabilities remain outside this phase.

## Authoritative model

SQLite migration/open is authoritative for durable history. Existing persisted
run events, run/session contexts, managed worktree rows, approval/audit rows,
and immutable terminal outcomes remain durable facts. On startup, only
repository-defined reconciliation is permitted: nonterminal runtime contexts
may become unavailable when a live owned runtime cannot be proven; worktree
reconciliation validates managed identity before transition. No arbitrary PID
reconnection, process discovery, or deletion is permitted.

Process cleanup remains backend-owned, bounded, idempotent, and restricted to
the directly launched child/process group already proven by the supervisor.
It never broadens the direct-child containment claim. Failure to verify cleanup
is surfaced as a bounded, redacted failure; records and managed worktrees are
retained rather than silently removed.

Migrations are applied atomically at database open with foreign keys and a
bounded busy timeout. Migration, storage, reconciliation, cancellation, and
cleanup failures fail closed; public errors never reveal paths, raw database
diagnostics, commands, process IDs, output, environment, prompts, or private
events.

## Non-goals and boundary

Phase 10 does not introduce automatic worktree removal, log export, arbitrary
log inspection, process suspension/resume, approval delivery, agent protocol
changes, remediation, release packaging/signing/notarization/updating, or
Phase 11 behavior. Phase 11 owns macOS release work. Notifications and
autostart remain unavailable; Codex App Server remains unavailable/experimental.

## Acceptance

Reliability acceptance uses only deterministic fake processes, temporary
repositories, and temporary databases. It validates migration-on-open,
durable event/history replay, terminal immutability, restart reconciliation,
bounded redacted failures, and verified direct-child cleanup. Phase 9's EG001
remains required for any completion-like conclusion: no agent claim, raw
command, or stale frontend state can bypass it.
