# Phase 2B OSS provenance audit

This document audits Sentinel commit `5460de95f1292994b9ac72fb990e0bc600f0f711` (`refactor: split Codex transport dispatcher`) against the local, read-only OSS research checkout supplied for this review. It records behavioral provenance, not an assertion of code copying. No donor code was copied into this document or into Sentinel: the audited implementation is Rust/Tokio, while the relevant donor implementation is JavaScript/Node.

## Research revision and scope

All donor references below are to repository `codex-plugin-cc` at Git HEAD `db52e28f4d9ded852ab3942cea316258ae4ef346`:

- `tests/fake-codex-fixture.mjs`
- `plugins/codex/scripts/lib/app-server.mjs`
- `plugins/codex/scripts/lib/codex.mjs`
- `plugins/codex/scripts/lib/app-server-protocol.d.ts`

The audit compared the changed transport, fake server, and integration tests only. Pre-existing Phase 2 adapter behavior that happened to be reformatted in the commit is not represented as a new donor-derived change. “Ported behavior” means the same observed behavior was deliberately represented in Sentinel’s own architecture; it does not mean source code was directly reused. “Independently reimplemented” means the protocol behavior is also in the donor but the examined source does not establish that Sentinel adopted it from that donor. “Sentinel-only” means the examined OSS files do not provide that behavior.

| Sentinel file/symbol | OSS source and revision | Referenced behavior | Reuse type | Sentinel test |
| -------------------- | ----------------------- | ------------------- | ---------- | ------------- |
| `crates/sentinel-codex/src/lib.rs`: `Shared`, `CodexAppServer::request`, `dispatch_stdout` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server.mjs`, `AppServerClientBase.request`, `handleLine` | Allocate monotonically increasing JSON-RPC IDs; keep pending requests by ID; route a response by its ID rather than by request order. | Ported behavior; no code reuse. The donor does not supply Tokio locking, oneshots, or the bounded registry. | `concurrent_requests_are_dispatched_by_id_when_responses_are_reversed` |
| `crates/sentinel-codex/src/lib.rs`: `dispatch_stdout` remove-before-send branch | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server.mjs`, `AppServerClientBase.handleLine` | Delete the pending entry before settling the request; ignore a response for an ID no longer pending. This makes a duplicate response harmless. | Ported behavior; no code reuse. Exactly-once race semantics across timeout, child exit, and shutdown are Sentinel-specific. | `duplicate_response_is_ignored_after_remove_before_complete`; `one_timeout_does_not_complete_another_request` |
| `crates/sentinel-codex/src/lib.rs`: `dispatch_stdout`, `notification_worker`, `handle_notification` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server.mjs`, `AppServerClientBase.handleLine`; `plugins/codex/scripts/lib/codex.mjs`, `captureTurn`, `applyTurnNotification` | Distinguish JSON-RPC responses from method notifications and deliver notifications in input order. | Ported behavior for classification/order; no code reuse. A separate bounded Tokio notification channel and worker is Sentinel-only. | `notification_worker_preserves_provider_order_and_shutdown_reaps_tasks` |
| `crates/sentinel-codex/src/lib.rs`: `start_turn`, `NotificationCommand::Activate`/`Flush`, `flush_notifications` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/codex.mjs`, `captureTurn` | Buffer turn notifications that arrive before the `turn/start` response supplies the turn ID, then process them after the response. | Ported behavior; no code reuse. Sentinel buffers by pending thread and persists normalized events, which the donor does not do. | `app_server_starts_threads_turns_interrupts_and_persists_normalized_events`; `notification_worker_preserves_provider_order_and_shutdown_reaps_tasks` |
| `crates/sentinel-codex/src/lib.rs`: `watch_child`, `complete_all` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server.mjs`, `SpawnedCodexAppServerClient` exit/error handlers and `AppServerClientBase.handleExit` | Treat process exit as connection closure, fail all outstanding requests, and clear the pending registry. | Ported behavior; no code reuse. A sole child-owner watcher, notification-buffer clearing, and one/many-pending fan-out are Sentinel-specific. | `child_exit_fans_out_to_one_and_many_pending_requests` |
| `crates/sentinel-codex/src/lib.rs`: `CodexAppServer::shutdown`, `Drop`, `watch_child` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server.mjs`, `SpawnedCodexAppServerClient.close` | Close stdin, wait for child exit, then terminate a child that does not exit. | Independently reimplemented shutdown behavior. The donor uses Node streams/timer/SIGTERM; it does not establish Sentinel’s task-join or ownership design. | `notification_worker_preserves_provider_order_and_shutdown_reaps_tasks` |
| `crates/sentinel-codex/src/lib.rs`: `start_thread`, `resume_thread`, `start_turn`, `interrupt_turn` | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `plugins/codex/scripts/lib/app-server-protocol.d.ts`, `AppServerMethodMap`; `plugins/codex/scripts/lib/codex.mjs`, `startThread`, `resumeThread`, `runAppServerTurn`, `interruptAppServerTurn` | The app-server methods and the `threadId`/`turnId` request and response shape are supported protocol facts. | Independently reimplemented protocol use; no code reuse. The audited source does not support any claim that Sentinel’s persistence adapter was copied from it. | `app_server_starts_threads_turns_interrupts_and_persists_normalized_events` |
| `crates/sentinel-codex/src/bin/fake-codex-app-server.rs`: normal request loop and `turn/start` notifications | `codex-plugin-cc@db52e28f4d9ded852ab3942cea316258ae4ef346`, `tests/fake-codex-fixture.mjs`, `send`, `turn/start` case, `emitTurnCompleted` | A JSONL fixture can answer requests and emit `turn/started`, item lifecycle, and `turn/completed` notifications. | Independently reimplemented fixture behavior; no code reuse. The Sentinel fixture is intentionally smaller and has different deterministic scenarios. | `app_server_starts_threads_turns_interrupts_and_persists_normalized_events`; `notification_worker_preserves_provider_order_and_shutdown_reaps_tasks` |
| `crates/sentinel-codex/src/bin/fake-codex-app-server.rs`: `reverse`, `timeout-one`, `exit-during-request`, `duplicate-response`, `notification-order` scenarios | No supporting behavior in the four inspected donor files at the revision above. The donor fixture has regular, delayed, interruptible, and subagent scenarios, but not these transport-adversarial modes. | Designed only for Sentinel. | `concurrent_requests_are_dispatched_by_id_when_responses_are_reversed`; `one_timeout_does_not_complete_another_request`; `child_exit_fans_out_to_one_and_many_pending_requests`; `duplicate_response_is_ignored_after_remove_before_complete`; `notification_worker_preserves_provider_order_and_shutdown_reaps_tasks` |
| `crates/sentinel-codex/tests/adapter.rs`: concurrency, reverse-order, timeout isolation, exit fan-out, duplicate response, empty-registry, and clean-shutdown tests | No supporting test cases in the four inspected donor files at the revision above. | Designed only for Sentinel. | The named tests in this row. |
| `PROJECT_STATUS.md`: Phase 2 remains PARTIAL | No donor source. | Designed only for Sentinel. | Not applicable (status documentation). |

## Sentinel-specific Phase 2B behavior

The following important behavior is not supported as donor-derived by the inspected OSS files:

- A mutex-protected child stdin writer shared by all callers.
- One Tokio stdout dispatcher task as the exclusive stdout owner.
- A fixed-capacity (`MAX_PENDING_REQUESTS`) registry with Tokio oneshot completions.
- Exactly-once completion across response, timeout, shutdown, and process exit, including explicit remove-before-complete race handling.
- A separate ordered notification channel/worker, rather than calling a notification callback in the stdout parser.
- Child ownership by an exit watcher task and deterministic joining of dispatcher, notification worker, and watcher on `shutdown`.
- Clearing buffered turn notifications during exit/shutdown fan-out.
- The adversarial fake-server modes and all Phase 2B transport-race tests added in the commit.

## Unsupported claims deliberately excluded

- The inspected donor uses an unbounded JavaScript `Map`; it does not support claiming that the Sentinel pending-request capacity or its value was reused.
- The inspected donor has no locked shared stdin writer, no Tokio task topology, and no oneshot registry; it does not support claiming those mechanisms were copied or ported.
- The inspected donor’s normal fixture does not emit reversed responses, duplicate responses, or exit while multiple requests are pending; it does not support attributing those tests to the donor.
- The inspected donor’s protocol declarations establish method names and shapes, not Sentinel’s V3 persistence, normalized event mapping, safety limits, or adapter lifecycle policy.
