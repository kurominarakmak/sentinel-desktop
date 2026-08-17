# Agent Sentinel V3 Roadmap

## Authority and historical record

V3 scope is defined by [V3 product scope](docs/v3-product-scope.md). Its implementation boundaries are defined by [V3 target architecture](docs/v3-target-architecture.md), and its task transitions by [V3 workflow state machine](docs/v3-workflow-state-machine.md). This roadmap orders V3 delivery; it does not redefine product, architecture, or workflow decisions.

The earlier V1 roadmap, architecture, adapter, state-machine, and phase-contract documents remain preserved historical records of the implementation foundation. They are superseded for future V3 planning where they conflict with the three V3 source documents; no V1 code or historical document is removed by this scope migration.

## Current V1 foundation

The current repository provides a macOS Tauri 2 + React + Rust application with SQLite persistence; reusable core, runtime, process, Git, and agent-API crates; a fake-agent execution/test path; Git worktree management; a tray app; `Command+Shift+Space` global hotkey; a hidden floating prompt; and status/settings window routes. The existing bounded Codex and Claude records/interfaces are not released as fully available V3 sessions. Existing approvals, drift, evidence, and recovery foundations do not yet provide the complete V3 supervisor workflow.

## Planned V3 capabilities

V3 adds a supervisor-owned durable workflow that maps supported Codex and Claude Code sessions to isolated task worktrees, normalizes events, runs configurable deterministic validation, performs read-only review with strict bounded repair rounds, recovers safely after crashes, and presents human-gated final actions. It evolves the current ambient UI into Quick Prompt, Attention Widget, and Task Detail without making the UI authoritative.

## Ordered V3 delivery

| Phase | Outcome | Status |
| --- | --- | --- |
| 1 | Unified event and durable task model | Complete |
| 2 | Native Codex adapter | Complete with documented provider limitations |
| 3 | Native Claude Code adapter | Complete with authentication-limited live validation |
| 4 | Worktree task transactions | Complete |
| 5 | Validation and command profiles | Complete |
| 6 | Read-only review and bounded repair | Complete |
| 7 | Attention widget and approval surfaces | Complete in Tauri; native presentation implemented in parallel |
| 8 | Recovery, security, and provenance hardening | Complete |
| 9 | Final Tauri UI redesign | Complete |

Detailed phase scope, dependencies, validation, rollback, commit boundaries, and deferrals are in [V3 implementation plan](docs/v3-implementation-plan.md).

## Deferred from V3 core

Cloud synchronization, teams, remote execution, automatic pull requests, IDE/browser plugins, mobile clients, background-daemon extraction, autonomous commit/merge/push, automatic worktree deletion, LLM-based approval decisions, and third-party adapter packs are deferred. ACP, OpenCode, Gemini CLI, Hermes, and quant-development workflows remain future optional integrations, not V3 core dependencies.

## Current implementation position

The V3 Rust workflow is implemented through the human-approval boundary. The
native SwiftUI/AppKit frontend now runs in parallel with Tauri and remains
presentation-only through `sentinel-native-bridge`. Native replacement is a
parity and release decision, not a backend rewrite; Tauri remains available
until native interaction QA and distribution requirements are accepted.
