# Phase 2C Codex Session Lifecycle Provenance

## Scope

This change wires Sentinel-owned Codex App Server sessions to the durable V3
task/session store. Sentinel remains the workflow authority: provider events
do not finalize a task, and restart recovery never infers completion.

## Read-only references inspected

| Source | Relevant surface | Reuse level | Sentinel adaptation |
| --- | --- | --- | --- |
| `openai/codex-plugin-cc` @ `db52e28` | `plugins/codex/scripts/lib/app-server.mjs`, `app-server-protocol.d.ts`, and `tests/fake-codex-fixture.mjs` | C — behavioral oracle | Preserved the supported initialize/thread start-resume/turn start-interrupt JSON-RPC lifecycle, one-owner request serialization, and deterministic fake-server fixtures. No TypeScript source or broker state model was imported. |
| `AnyiWang/OpenCovibe` @ `a8f8fdd` | `src-tauri/src/agent/{codex_appserver,session_actor}.rs` and `storage/codex_sessions.rs` | C — behavioral oracle | Adapted only the contract that a persisted thread ID may be resumed after a new process is created, and that unavailable recovery remains visible. Sentinel does not copy its actor, transcript discovery, UI, settings, or provider-authoritative state. |

## Sentinel-specific behavior

- `CodexSessionManager` owns only App Servers it starts and associates them with
  durable Sentinel V3 task/session records.
- Every recreated connection initializes its normalized-event sequence from the
  durable task log, preventing sequence reuse after restart or a dead child.
- Missing sessions and failed resume leave V3 records in `Recovering` /
  `RecoveryRequired`; no external session scan is attempted.
- `turn/completed` remains provider evidence. It cannot transition a Sentinel
  task to `Finalized`.
