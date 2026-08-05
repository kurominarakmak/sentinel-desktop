# Phase 5 Authoritative Execution Contract

## Authority and objective

[PLAN.md](../PLAN.md) authorizes Phase 5 to make **Claude Code use the same
normalized interface** as Codex: bounded streaming parsing, follow-up input,
resume, permission events, and cancellation. This document decomposes that
objective only. PLAN remains authoritative and unmodified; Phase 3 and Phase 4
contracts remain authoritative for existing worktree, process, persistence,
and public-boundary guarantees.

## Boundary

Phase 5 adds a private Claude stream-json adapter and an ownership-bound
persisted Claude session context. It exposes only minimal redacted start,
query, follow-up, resume, and cancel DTOs. It does not add the normal desktop
prompt/tray/task-detail workflow (Phase 6), policies/approvals enforcement
(Phase 7), drift/evidence, remote execution, automatic worktree removal, App
Server execution, arbitrary shell/process/database access, or process
reconnection after restart.

## Dependency order

1. Define fixed Claude argv forms and a strict bounded UTF-8 JSON-lines parser
   that maps allowlisted stream events into `AgentEvent`, including an explicit
   permission-needed normalized event.
2. Persist a Claude context containing only internal run identity, opaque
   backend reference, exact ProjectId/WorktreeId/task key ownership, bounded
   opaque session token, adapter/protocol identity, versioned lifecycle,
   bounded public summaries, redacted failure category, follow-up sequence,
   and timestamps. Prompts, raw provider records/output, paths, argv,
   environment, PID, and credentials are never persisted.
3. Extend the private runtime registry with a Claude direct-child adapter.
   A turn owns one run/session context; follow-up and resume use fixed forms,
   only the persisted backend session token, and the same authoritative
   managed worktree. Cancellation wins all late terminal events.
4. Add ownership-scoped desktop commands and explicit redacted Rust/TypeScript
   DTOs. Public input never supplies a path, executable, PID, session token,
   argv, environment, internal ID, or lifecycle state.
5. Add deterministic fake-Claude tests, focused persistence/runtime/bridge
   tests, frontend contract/lifecycle tests, and Phase 3/4 regressions.

## State, authority, and failure behavior

Claude contexts use the closed Phase 4 lifecycle (`created`, `starting`,
`running`, `cancelling`, `cancelled`, `succeeded`, `failed`) plus a bounded
follow-up sequence. Project, worktree, task key, and opaque reference are
required for every public lookup or mutation; wrong ownership is externally
indistinguishable from an unknown reference. Session tokens are backend
received, validated opaque values and never public. Transitions use expected
state/version and terminal records atomically store a bounded redacted result.
Malformed, oversized, unknown, conflicting, persistence, startup, timeout, or
identity failures fail closed. Restart never reconnects an OS process or a
Claude session: nonterminal contexts become an interrupted failure.

The private active registry contains only the internal RunId, ownership tuple,
opaque public reference, cancellation coordination, and direct-child handle.
Locks are not held across awaits; duplicate insertion and cross-run mutation
reject; cleanup is idempotent. Direct-child termination is the only containment
claim.

## Resource and redaction limits

All inputs, JSON records, event counts, public summaries, session tokens,
stdout/stderr, startup, execution, and cancellation waits are bounded. UTF-8
is strict; no lossy path/text conversion is used. Children use fixed argv, no
shell, null stdin, fixed/sanitized environment, and only an authoritative
managed worktree. Public JSON is allowlisted and serialization tests inject
sensitive markers proving they cannot cross the bridge.

## Frontend and public interface

TypeScript mirrors the Rust DTOs exactly with opaque references, exhaustive
status/error handling, duplicate-action prevention, stale-response ordering,
bounded polling, terminal cleanup, and unmount cleanup. It must present Claude
capability truthfully but does not implement Phase 6 workflow UI. App Server
remains unavailable/experimental.

## Test environment and completion

Tests use only deterministic fake Claude executables, temporary SQLite
databases, and newly-created temporary Git repositories/managed worktrees. No
real Codex/Claude/product-agent workflow, production repository, user
worktree, network, or automatic worktree removal is permitted. Completion
requires focused parser/persistence/runtime/bridge/frontend tests; Phase 4
bounded-Codex and Phase 3 two-task isolation regressions; all configured Rust
and frontend checks; accurate documentation; a clean final P1/P2 review; one
Phase 5 commit and checkpoint tag. Phase 6 remains not started.
