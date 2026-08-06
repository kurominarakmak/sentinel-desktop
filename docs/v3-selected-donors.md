# Sentinel V3 Selected Donors

## Selected set

The selected set is deliberately small. Sentinel will not import a desktop application, provider implementation, terminal multiplexer, or UI framework.

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
| `jean` | Reference quality is high, but it is a large competing Tauri app and its detached singleton server ownership conflicts with Sentinel's recovery/ownership boundary. |
| `OpenCovibe` | Strong Rust session actor/event examples, but it is a complete Svelte/Tauri app; transcript import and provider permission-mode mapping are not V3 authority models. |
| `threadline` | Good overlay inspiration, but a Swift-native observational transcript scanner; it cannot become the supervisor. |
| `parallel-code` | Electron/Solid app; useful worktree/diff edge cases but includes unsafe permission-bypass command definitions. |
| `agetor` | Electrobun/daemon application with Claude-specific permission storage; imports would duplicate authority. |
| `tuicommander` | Large PTY application with vendored terminal patches and terminal-output parsing. |
| `elves` | Similar stack but weaker session/recovery and direct Git action ownership; Sentinel's own crates are the better base. |
| `codex-review` | Useful reference for finding coverage, but a Go CLI workflow/file ledger rather than reusable V3 supervisor code. |
| `architect-loop` | Markdown/shell operating discipline, not a reusable library; its single-review-cycle workflow differs from V3. |
| `dmux` | tmux/shell hooks own process and resume behavior, incompatible with managed direct sessions. |
| `agent-sessions` | Observational transcript/credential integrations and Hermes-related surface violate V3 core constraints. |

## Selection rule

Before any implementation imports code, every selected candidate must pass the license/provenance gate in [V3 third-party license plan](v3-third-party-license-plan.md). A failed compatibility spike converts the candidate to reference-only; no donor may force a change to Tauri, React, Rust, or Sentinel ownership.
