# Phase 2B Runtime Orchestration

## Scope And Crate Boundary

Phase 2B adds `sentinel-runtime`, a reusable Tokio orchestration crate. It is separate from `sentinel-core`: core owns the typed domain, transition validation, migrations, and SQLite repository, while runtime owns live process handles, fake-agent execution, subscriptions, and cancellation. Neither crate depends on Tauri, the desktop app, React, or a Codex/Claude executable.

`RunOrchestrator::submit_run` stores a queued run then starts a Tokio runner. That runner alone owns the fake-agent process and final status: it transitions through `queued → preparing → running`, normalizes structured stdout, assigns increasing per-run sequence numbers, persists each event, then publishes it. A fake `completed` event is not authoritative. A persisted `agent_failed` event is terminal failure evidence even if the process later exits zero. `get_active_run`, `subscribe_to_run_events`, `cancel_run`, and `wait_for_run` are reusable runtime APIs. Phase 2C will add their Tauri bridge and UI.

`RunOrchestrator` receives a validated `FakeAgentProgram` with one explicit executable path. It performs no PATH search, current-working-directory inference, or implicit Cargo fallback. A missing path is rejected as the typed safe `LaunchTargetMissing` configuration error. The optional `developer_cargo_workspace` constructor is deliberately opt-in for local development only. Phase 2C will resolve the bundled Tauri sidecar/executable path and pass it into this configuration; it does not begin that sidecar integration here.

## Event Bus Guarantees

Each active run has a bounded Tokio broadcast channel of 64 messages. Every live event includes its run ID and sequence number. SQLite is the source of truth: an event is published only after it is persisted. A disconnected or slow subscriber cannot fail or delay the run; a lagged receiver gets Tokio's explicit lag error and reloads ordered history from SQLite. There is no unbounded subscriber queue and no replay of events published before a subscription.

Malformed stdout produces a safe `parser_error` event without retaining the raw line, then fails the run deterministically. Oversized/unpersistable events produce a safe `event_persistence` failure, never a false completion. Payload strings are redacted before storage and Phase 2A size limits remain enforced.

The first normalized-event persistence failure is fatal. Runtime immediately stops accepting/publishing further agent events, marks cancellation in progress, terminates and waits for the owned process group, drops the output receiver, attempts a safe failed terminal record, and resolves all existing waiters with the typed storage error. Even if the terminal write also fails, the active entry and live/cancellation handles are removed, so no caller or child process is stranded.

Runtime accumulates terminal evidence rather than allowing output, cancellation, and exit branches to independently finish a run. The pure resolver gives storage/termination failures and lingering process groups priority, then uses an explicit first-observed terminal winner: an `agent_failed` event observed before cancellation fails the run; a confirmed cancellation observed first remains cancelled despite its later termination exit; and a parent exit observed first is evaluated for protocol failure, nonzero status, or clean zero-exit completion. Missing or contradictory evidence is failed conservatively. One finalizer is the only normal caller of `finish_run`, emits exactly one status-matched terminal lifecycle event, resolves completion watchers once, and removes the active entry.

## Fake Agent, Process, And Cancellation

The local `sentinel-fake-agent` binary supports deterministic success, failure, delayed, malformed, partial-output, cancellation-child, redacted-stderr, burst, and oversized-event scenarios. It receives only a fixed scenario value; user task text is not interpolated into a shell or exposed as a command. Its child scenario re-executes the fake binary.

`sentinel-process` owns a process group on Unix, consumes stdout/stderr on bounded channels, and cancels with group termination then escalation after a timeout. Runtime captures at most 4 KiB of redacted stderr. The first cancellation returns `cancellation_requested`; repeats return `already_cancelling`; terminal, absent, and cleanup-failed cases use typed results. There is no global kill command.

Reaping the direct parent is not completion: on Unix the runner checks the owned process group after parent exit. If a descendant still exists (including one retaining stdout or stderr), runtime treats that as an abnormal lifecycle failure, terminates and verifies the owned group, and drains output only until a bounded cleanup deadline. It continues to consume cancellation commands during that drain. A cancellation observed before parent exit follows the normal cancellation path; once parent exit has been observed, its exit-derived result wins, except that a lingering descendant always produces the safe failed `lingering_process_group` outcome and never a `run_completed` event. Failure to verify group termination resolves waiters with the typed termination error after a best-effort failed persistence write. This check is intentionally process-group scoped and never targets unrelated processes.

Stderr is redacted before capture and passed through a UTF-8-safe byte-bounded helper. It only slices at character boundaries, adds an ellipsis when the remaining byte budget permits, and never retains a partial secret value.

The runner checks cancellation before spawn and during execution. If it observes cancellation first it writes `cancelling → cancelled`; if it observes process exit first it writes the exit-derived terminal result, subject to earlier agent-failure/protocol evidence. A single runner owns terminal persistence, so completed and cancelled cannot both be persisted. `RunRepository::finish_run` atomically writes terminal status, exit code, safe error, and lifecycle event.

Every post-registration orchestration error uses one cleanup path. It removes the active entry, resolves all existing completion watchers with a typed safe storage error, and drops cancellation/event handles. If a safe failed transition is still possible it is attempted, but its failure cannot strand a caller or falsely report completion/cancellation. A process started before a running-transition failure is cancelled before this cleanup runs.

## Restart And Tests

Persisted runs/events survive restart. In-memory process handles, subscription channels, and pending cancellation requests do not; orphan reconciliation is deferred to the Reliability phase.

The runtime integration suite uses only temporary SQLite files, Tokio, and the local fake binary. It covers success/live ordering, failure including `agent_failed` with a zero exit, delayed running observation, malformed and partial output, child cancellation, cancellation races, lag, persistence failure, UTF-8-safe stderr redaction, and parent-exit scenarios where descendants retain stdout or stderr. Resolver unit tests cover the terminal-precedence table. No test invokes Codex, Claude Code, or `sentinel-probe`.
