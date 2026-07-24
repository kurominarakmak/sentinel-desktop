# Agent Sentinel Implementation Roadmap

**Operational source of truth.** `PLAN.md` is the implementation roadmap. Dedicated files under `docs/` are authoritative for their technical domains. [PROJECT_STATUS.md](PROJECT_STATUS.md) contains current phase, documentation navigation, and the immediate next action.

## Product Goal

Build Agent Sentinel: an open-source, local-first desktop application that supervises Codex and Claude Code tasks in isolated Git worktrees and accepts completion only when deterministic evidence passes.

## Locked Decisions

- Tauri 2; React, TypeScript, Vite; Rust with Tokio; SQLite.
- macOS first, then Windows and Linux; architecture remains cross-platform.
- Codex and Claude Code use structured CLI adapters. Codex App Server is experimental and used where appropriate; a small Claude TypeScript SDK bridge is optional only if CLI integration is insufficient.
- Every implementation task uses a Git worktree. TOML stores user/project policy; SQLite stores application state.
- Autonomy profiles are Safe, Balanced, Autonomous, and Custom. MVP includes basic Drift Guardian and Evidence Gate.
- One developer, assisted by Codex and Claude Code, builds the project.
- Version 0.1 is one long-running Tauri process. `sentineld` is deferred until a concrete need exists.
- The app must never automatically merge, push, force-push, or silently install AI CLIs.

## MVP Scope

On macOS, a user can register a local Git repository; detect Git, Codex, and Claude Code; open a global prompt; select an agent; run a task in a worktree; monitor activity in the menu bar; send follow-up; cancel; resolve approval requests; receive protected-path drift warnings; run configured evidence; review the final diff; and restart without losing history.

Excluded from MVP: semantic undo, cross-agent debate or review, cloud sync, teams, remote execution, automatic pull requests, IDE plugins, mobile, LLM-based drift scoring, and automatic rule learning. See [product requirements](docs/product-requirements.md).

## Development Phases And Exit Criteria

| Phase | Outcome | Exit criterion |
| --- | --- | --- |
| 0. Feasibility | Tray, shortcut, prompt, adapters, cancellation, worktrees | Both agents modify fixtures through parsed events; processes cancel; limitations are documented. |
| 1. Foundation | Workspace, Tauri, React, CI, quality tooling | macOS build and CI tests pass; core crates avoid Tauri dependencies. |
| 2. Fake-agent slice | Adapter API, state machine, event bus, SQLite, basic UI | A fake task traverses the UI and state machine. |
| 3. Git isolation | Project registry, worktree manager, diffs | Two fake tasks run separately without touching the main directory. |
| 4. Codex | Detection, `exec --json`, persistence, cancellation, experimental App Server | A Codex task produces an inspectable diff. |
| 5. Claude Code | Streaming parser, follow-up, resume, permissions, cancellation | Claude uses the same normalized interface. |
| 6. Desktop UX | Prompt, tray, task detail, notifications, autostart | Normal workflow requires no terminal beyond install/authentication. |
| 7. Approvals | Profiles, custom policy, audit queue | Sensitive actions cannot proceed without a valid decision. |
| 8. Drift Guardian | Deterministic MVP rules | Every finding is reproducible and factually explained. |
| 9. Evidence Gate | Baselines, commands, final evaluator, reruns | An agent claim alone cannot display Completed. |
| 10. Reliability | Recovery, process cleanup, logs, migrations | Restart preserves history and never silently deletes work. |
| 11. macOS release | Builds, package, signing, notarization, updater | New user can install, configure, and run a task. |
| 12. Windows | Tray, shortcut, cancellation, discovery, installer | Platform behavior is implemented and validated. |
| 13. Linux | Desktop compatibility, discovery, packages, cancellation | Platform behavior is implemented and validated. |

See the explanatory [implementation plan](docs/implementation-plan.md).

## Ordered Backlog

1. Product and security RFCs; scaffold Cargo workspace and Tauri/React app; CI.
2. Fake agent, normalized events, state machine, migrations, project registry, worktrees, and event persistence.
3. Tray, shortcut, prompt, active-task panel, and executable discovery.
4. Codex structured-exec spike, parser, cancellation, and App Server spike.
5. Claude streaming spike, parser, resume, and adapter contract tests.
6. Profiles, approval queue, protected-path/dependency/repeated-failure rules, evidence runner, and final evaluator.
7. Restart recovery, macOS packaging, and first alpha.

## Definition Of Done

The MVP scope above is complete only when all required evidence passes, task history survives restart, and the final diff is available for review. Detailed completion conditions are in [evidence model](docs/evidence-model.md); task states are in [task state machine](docs/task-state-machine.md).

## Deferred Features

Independent cross-agent review is the first post-MVP candidate: one agent implements, the other reviews, accepted findings are fixed, then evidence runs again. All other excluded features remain deferred until the core workflow is reliable.

## Immediate Next Steps

Phase 1 Foundation is complete: the workspace, Tauri shell, React frontend, local validation scripts, and non-interactive CI are verified. Phase 2 is in progress: Phase 2A establishes the typed SQLite run foundation and Phase 2B establishes fake-agent orchestration, process supervision, and the internal event bus. Complete the Phase 2C Tauri bridge and basic run UI before advancing beyond Phase 2.
