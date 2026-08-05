# Contributor Implementation Plan

[`PLAN.md`](../PLAN.md) is the operational roadmap; this document explains how contributors should execute it without changing its priorities. Dedicated files under `docs/` are authoritative for their technical domains.

## Guiding Sequence

Validate uncertain integrations before polishing the interface. Build the core as reusable Rust crates, then prove the task lifecycle with a fake agent before connecting real CLIs. Keep the desktop process long-running and defer `sentineld` until a documented extraction criterion is met.

## Phase 0: Feasibility

Spike the macOS shortcut, tray, floating prompt, hide-to-tray behavior, Git worktree creation, executable-path discovery, process cancellation, Codex `exec --json`, Codex App Server, Claude `stream-json`, and Claude resume by session ID. The phase exits only when both agents can modify fixture repositories through parsed events, both can be cancelled, worktrees can be safely removed, and persistent-steering limitations are documented.

## Foundation Through Real Adapters

Foundation establishes the monorepo, Cargo workspace, Tauri/React application, CI, formatting, linting, license, contribution docs, and architecture records. Next, the fake agent drives normal completion, file changes, test failure, approvals, drift, crash, and cancellation through the state machine, SQLite, tray status, and prompt.

Git isolation then registers projects, validates repositories, records base commits, creates branches/worktrees, generates diffs, supports cleanup, and opens editor or terminal. Only after that do the Codex and Claude adapters enter the product. Codex begins with structured `exec` parsing; App Server remains experimental and version-tested. Claude begins with structured streaming, session capture, follow-up input, resume fallback, permission events, cancellation, and errors.

## Phase 4 execution boundary

Phase 4 persists a redacted Codex run context before execution, binds it to the
authoritative project/worktree/task tuple, and exposes only opaque-reference
start/query/cancel DTOs. The adapter has fixed argv/environment, bounded JSON
Lines parsing, direct-child-only cancellation, optimistic lifecycle updates,
and conservative restart interruption reconciliation. No automatic worktree
removal occurs. App Server remains unavailable/experimental and Phase 5 is not
started.

## Product Controls And Release Work

Desktop UX adds the prompt, search, agent selection, tray state, task details, notifications, keyboard navigation, and autostart. Approval profiles, audit history, deterministic drift findings, and evidence checks follow. Reliability adds persistent logs, restart recovery, interrupted-task detection, process-group cleanup, output limits, rotation, worktree recovery, and migrations.

macOS packaging is the first release target: Apple Silicon and Intel/universal builds, menu-bar polish, DMG, signing, notarization, updater, and accessibility checks. Windows and Linux follow only with their platform-specific validation.

## Contribution Constraints

Do not add application features from the deferred list before the end-to-end core workflow is reliable. Do not replace structured events with terminal-output scraping when a structured interface exists. Do not make the UI authoritative, modify the primary working tree by default, auto-merge, auto-push, force-push, or install AI CLIs silently.

Refer to [adapter contract](agent-adapter-contract.md), [testing strategy](testing-strategy.md), and [platform support](platform-support.md) while implementing each phase.
