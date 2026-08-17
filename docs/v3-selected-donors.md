# Sentinel V3 Selected Donors

## Selected set

The selected set is deliberately small. Sentinel will not import a desktop application, provider implementation, terminal multiplexer, or UI framework.

## Reuse-level catalog

Every source below is assessed at module level. “Whole app: reject” means neither its runtime, state model, UI, nor process ownership is imported.

| Repository | Whole app | Level A — direct reuse | Level B — port with tests | Level C — behavioral oracle / fallback |
| --- | --- | --- | --- | --- |
| `agent-client-protocol` | n/a schema project | ACP v1 schema dependency candidate after stability/provenance gate | none | ACP v2 remains reference only because its module is marked experimental |
| `codex-plugin-cc` | reject plugin | none | none | App Server method/type contract guides locally authored JSON-RPC adapter tests |
| `cctop` | reject | none | `WorktreeRemovalService.swift` refresh-before-destruction/refusal algorithm | focus restoration and cleanup presentation |
| `ralph-review` | reject | none | finding-artifact schema/validation from `findings/{types,artifact}.ts` | bounded-cycle terminal-state edge cases |
| `OpenCovibe` | reject | none | actor cancellation and ordered-event/replay/fork fixtures from `session_actor.rs`, `session_protocol.rs`, `storage/events.rs` | Codex/Claude event mapping and session-import fixtures; transcript imports are fallback diagnostics only |
| `Jean` | reject | none | none | App Server recovery/thread routing, session/worktree models, diff/approval/notification mapping, restart test cases |
| `threadline` | reject | none | `WorkStatusResolver`, `StuckLoopDetector`, and `FileConflictDetector` algorithms with Sentinel-owned tests | overlay/focus behavior; transcript scanner is fallback reference only |
| `parallel-code` | reject | none | worktree setup/ignored-directory/dependency-linking, diff/coverage identity, and merge-readiness algorithms with its edge-case tests | review/diff presentation; unsafe permission-bypass definitions are rejected outright |
| `agetor` | reject | none | task/run/event migration and dedup/approval/question representation with Sentinel migration tests | pinned base-commit and recovery behavior; daemon/Claude authority rejected |
| `tuicommander` | reject | none | bounded rate-limit/error/waiting-question classifiers only, with false-positive tests | process/session discovery and status normalization; terminal output is diagnostic-only |
| `elves` | reject | none | task-worktree mapping and targeted process-cleanup behavior with lifecycle tests | ship/discard interaction and recovery cases; direct Git authority rejected |
| `codex-review` | reject | none | finding-ID/coverage validation with schema tests | review disposition workflow |
| `architect-loop` | reject | none | none | reviewer/implementer separation and evidence requirements |
| `dmux` | reject | none | none | no retained oracle: tmux ownership and hook-driven resume conflict with V3 |
| `agent-sessions` | reject | none | none | no retained oracle: credential/transcript observation and Hermes-related scope conflict with V3 |

### Threadline example

```yaml
repository: threadline
whole_app: reject
modules:
  status_resolver:
    classification: level_b_port_with_tests
  overlay_behavior:
    classification: level_c_behavioral_oracle
  transcript_scanner:
    classification: fallback_reference
```

For Threadline, Level B tests must establish deterministic status priority, repeated-error thresholds, stuck-loop scoring, and file-conflict ordering without reading provider transcripts. Level C tests must establish the overlay/focus contract against Sentinel-owned task state. The fallback scanner, if ever added, cannot transition a task, clear a blocker, or authorize a command.

### `agentclientprotocol/agent-client-protocol` — protocol/schema candidate

- **Why needed:** Its Rust schema models versioned session updates, tool-call patches, terminal updates, plan updates, elicitation, and unknown-event preservation. This is useful for an optional future ACP adapter boundary.
- **Exact scope:** Evaluate a pinned stable crate surface from `agent-client-protocol-schema/src/v1/`; use the inspected `src/v2/{client,tool_call,terminal,plan,elicitation}.rs` only as a design reference because `v2/mod.rs` declares it experimental.
- **Reuse:** Direct dependency candidate, subject to a later compatibility, security, and license gate. Sentinel's internal normalized event envelope stays independent and stores its own task sequence/causation fields.
- **Explicitly not imported:** Any workflow, task state, Git, approval, UI, or process management—ACP provides none.

### `openai/codex-plugin-cc` — Codex App Server protocol reference

- **Why needed:** It is the only audited official OpenAI repository and has a narrow App Server method/type map.
- **Exact scope:** `plugins/codex/scripts/lib/app-server-protocol.d.ts`, specifically the `thread/start`, `thread/resume`, `turn/start`, and `review/start` method mapping and notification typing.
- **Reuse:** External process or protocol adapter reference. Sentinel will author its Rust JSON-RPC transport/normalizer and validate it against the installed supported App Server.
- **Explicitly not imported:** Plugin commands, prompts, agents, scripts, CLI behavior, or any desktop workflow.

### `st0012/cctop` — destructive-cleanup preflight pattern

- **Why needed:** `WorktreeRemovalService.swift` demonstrates a small, testable sequence: determine a candidate action, refresh live Git evidence immediately before execution, refuse if identity/evidence changed, then run a narrow Git command.
- **Exact scope:** The preflight/refusal design from `menubar/CctopMenubar/Services/WorktreeRemovalService.swift` only.
- **Reuse:** Port selected module concept, reimplemented in `sentinel-git` Rust. It will be further restricted by Sentinel-owned task/worktree/approval version checks and will never make cleanup automatic.
- **Explicitly not imported:** SwiftUI/AppKit UI, session-file discovery, notification logic, session lifecycle, or force-removal policy.

### `kenryu42/ralph-review` — structured finding-artifact concept

- **Why needed:** The narrow finding artifact separates persisted findings, selection, schema validation, reviewer session output, and repair result history.
- **Exact scope:** Concepts and field-level test cases around `src/lib/review-workflow/findings/{types.ts,artifact.ts}`.
- **Reuse:** Port selected module concept, reimplemented as a Sentinel-owned Rust schema/migration. The V3 state machine remains the authority for severity/disposition/repair bounds.
- **Explicitly not imported:** Bun runtime, CLI, React/Ink UI, agent runner, Git checkpoint/worktree logic, prompt text, retries, or completion logic. In particular, Sentinel rejects ralph-review's behavior that can label max-iteration unresolved work as `completed`.

## Rejected as donors

| Repository | Reason |
| --- | --- |
| `codex-review` | Useful reference for finding coverage, but a Go CLI workflow/file ledger rather than reusable V3 supervisor code. |
| `architect-loop` | Markdown/shell operating discipline, not a reusable library; its single-review-cycle workflow differs from V3. |
| `dmux` | tmux/shell hooks own process and resume behavior, incompatible with managed direct sessions. |
| `agent-sessions` | Observational transcript/credential integrations and Hermes-related surface violate V3 core constraints. |

## Reference donors retained outside the code-donor set

### `OpenCovibe`

OpenCovibe is a high-value **reference donor**, not a rejected source. Use its actual Rust modules to derive adapter tests and local designs for: Codex App Server event mapping (`agent/codex_appserver.rs`), Claude `stream-json` parsing (`agent/claude_stream.rs`), one-actor-per-session process ownership and cancellation (`agent/session_actor.rs`), normalized protocol framing (`agent/session_protocol.rs`), ordered event replay/fork behavior (`storage/events.rs`), and session-import fixture cases (`storage/{cli_sessions,codex_sessions}.rs`).

Do not adopt its complete application state, Svelte/Tauri architecture, provider permission authority/settings mapping, transcript imports as Sentinel's source of truth, or bypass-permission behavior. Sentinel persists and reconciles its own authoritative task/session records; provider transcripts may be optional diagnostic artifacts only.

### `Jean`

Jean is a high-value **reference donor**, not a rejected source. Use it to derive test cases and design questions for Codex App Server thread/session routing and restart recovery (`chat/codex_server.rs`), session/worktree domain and persistence boundaries (`chat/storage.rs`, `projects/{types,git,storage}.rs`), CLI structured-stream configuration (`chat/claude.rs`), and user-facing repository, diff, approval, notification, and state mapping.

Do not adopt its detached singleton server ownership, complete desktop architecture, storage model, process lifetime, or provider-specific UI. Sentinel remains responsible for lifecycle ownership and may reattach only after its own durable task/worktree/session identity checks pass.

### `Threadline`

Threadline is a reference donor for overlay interaction (`Panel.swift`, `HotKey.swift`, `JumpBack.swift`), status resolution and attention priority (`WorkStatus.swift`), repeated-error/stuck-loop scoring (`StuckLoopDetector.swift`), and concurrent file-conflict ranking (`FileConflict.swift`). These are useful input to Sentinel's Attention Widget and deterministic observation catalog.

Do not use its Codex/Claude transcript scanning as authoritative task/session state. At most, a future optional observability integration may label transcript-derived signals as non-authoritative diagnostics that never advance workflow state or authorize action.

### `Parallel Code`

Parallel Code is a reference donor for worktree setup edge cases and tests (`electron/ipc/{git.ts,git-worktree.test.ts}`), ignored-directory/dependency linking and sandbox lifecycle, merge/discard/diff-base cases, diff grouping/freshness (`src/lib/{unified-diff-parser,diff-review-lifecycle}.ts`), and coverage visualization identity (`coverage.ts`, `coverage-comparison.ts`).

Do not import its Electron/Solid application or unsafe permission-bypass command definitions from `electron/ipc/agents.ts`. The worktree concepts must be reimplemented behind `sentinel-git` and Sentinel approval preflight.

### `Agetor`

Agetor is a reference donor for task/run/event relational shape and migrations (`src/bun/migrations/{002_worktree,018_run_events_dedup,030_runs_task_id_index}.sql`), pinned worktree/base-commit reasoning (`worktree.ts`), append/dedup event behavior (`task-events.ts`, `global-events.ts`), structured questions/approvals (`claude-questions.ts`, `approval.ts`), and recovery cases.

Do not import daemon ownership, Electrobun/Ink UI, or Claude-specific permission authority. Sentinel's Rust supervisor owns process/session state and translates provider requests into its own approval records.

### `TUICommander`

TUICommander is a reference donor for process/session detection, rate-limit/error classification, waiting/question detection, session discovery, and status normalization in `src-tauri/src/{agent_session.rs,output_parser.rs,repo_watcher.rs}`.

Do not import vendored terminal patches, PTY platform code, hooks, or terminal scraping as V3's primary integration. Structured supported Codex/Claude interfaces remain authoritative; any parser-derived signal is diagnostic-only.

### `Elves`

Elves is a reference donor for small Tauri process lifecycle and cleanup (`agents/process.rs`), session/event persistence (`db/{sessions,events}.rs`), task-to-worktree mapping (`commands/{sessions,workspace}.rs`), and ship/discard test cases (`commands/workspace.rs`).

Do not adopt its direct stage/commit/push operations, forced worktree removal, frontend-driven state, or stale-session-to-failed recovery decision. Sentinel's durable supervisor and explicit final approval remain authoritative.

## Selection rule

Before any implementation imports code, every selected candidate must pass the license/provenance gate in [V3 third-party license plan](v3-third-party-license-plan.md). A failed compatibility spike converts the candidate to reference-only; no donor may force a change to Tauri, React, Rust, or Sentinel ownership.
