# Agent Sentinel V3 Roadmap

## Authority and historical record

V3 scope is defined by [V3 product scope](docs/v3-product-scope.md). Its implementation boundaries are defined by [V3 target architecture](docs/v3-target-architecture.md), and its task transitions by [V3 workflow state machine](docs/v3-workflow-state-machine.md). This roadmap orders V3 delivery; it does not redefine product, architecture, or workflow decisions.

The earlier V1 roadmap, architecture, adapter, state-machine, and phase-contract documents remain preserved historical records of the implementation foundation. They are superseded for future V3 planning where they conflict with the three V3 source documents; no V1 code or historical document is removed by this scope migration.

## Current V1 foundation

The current repository provides a macOS Tauri 2 + React + Rust application with SQLite persistence; reusable core, runtime, process, Git, and agent-API crates; a fake-agent execution/test path; Git worktree management; a tray app; `Command+Shift+Space` global hotkey; a hidden floating prompt; and status/settings window routes. The existing bounded Codex and Claude records/interfaces are not released as fully available V3 sessions. Existing approvals, drift, evidence, and recovery foundations do not yet provide the complete V3 supervisor workflow.

## Planned V3 capabilities

V3 adds a supervisor-owned durable workflow that maps supported Codex and Claude Code sessions to isolated task worktrees, normalizes events, runs configurable deterministic validation, performs read-only review with strict bounded repair rounds, recovers safely after crashes, and presents human-gated final actions. It evolves the current ambient UI into Quick Prompt, Attention Widget, and Task Detail without making the UI authoritative.

## Ordered V3 delivery

| Phase | Outcome | Exit criterion |
| --- | --- | --- |
| 1 | Unified event and durable task model | **Complete.** Versioned events, replay, state transitions, additive SQLite migration, and fail-closed restart recovery pass contract tests. |
| 2 | Native Codex adapter | Supported Codex interface starts, streams, cancels, resumes/recoverably reconciles, and passes adapter fixtures. |
| 3 | Native Claude Code adapter | Supported Claude Code CLI/session interface meets the same bounded contract. |
| 4 | Worktree task transactions | Create/diff/merge/discard/rollback/cleanup are ownership-bound and protect the main tree. |
| 5 | Validation and command profiles | Configurable deterministic checks execute in the worktree with durable artifacts. |
| 6 | Read-only review and bounded repair | Structured review findings and capped repair rounds reach a reliable final-decision boundary. |
| 7 | Attention widget and approval surfaces | Ambient controls expose truthful status and require confirmed approval for consequential actions. |
| 8 | Recovery, security, and license hardening | Restart, permission, artifact redaction, provenance, and license gates pass. |
| 9 | Final UI redesign | The end-to-end V3 workflow is usable without disrupting the current app. |

Detailed phase scope, dependencies, validation, rollback, commit boundaries, and deferrals are in [V3 implementation plan](docs/v3-implementation-plan.md).

## Deferred from V3 core

Cloud synchronization, teams, remote execution, automatic pull requests, IDE/browser plugins, mobile clients, background-daemon extraction, autonomous commit/merge/push, automatic worktree deletion, LLM-based approval decisions, and third-party adapter packs are deferred. ACP, OpenCode, Gemini CLI, Hermes, and quant-development workflows remain future optional integrations, not V3 core dependencies.

## Current implementation position

Phase 1 is implemented in `sentinel-core` only. It adds a provider-neutral V3 task aggregate, append-only normalized event envelopes, session/approval/validation/finding/repair/artifact records, and additive SQLite persistence. Restart reconciliation marks unfinished non-draft tasks `Recovering` and active sessions `RecoveryRequired`; it never assumes that an external agent process survived or infers completion. The React UI and provider adapters remain non-authoritative and unchanged.

Only Phase 2 may begin next, after focused authorization: a supported Codex adapter that supplies normalized events to the Phase 1 supervisor store. No later phase has started.
