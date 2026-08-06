# Agent Sentinel V3 Roadmap

**V3 operational source of truth.** This document and the `docs/v3-*.md` documents define future work. The earlier V1 phase documents remain in the repository as historical implementation records; they are superseded for product scope and must not be silently rewritten or removed.

## Product outcome

Agent Sentinel V3 is an ambient macOS coding-agent supervisor. From Safari, VS Code, or another app, a developer can invoke a prompt, select a repository and workflow, and have Sentinel create an isolated Git worktree, supervise a Codex or Claude Code session, validate and review its work, run bounded repairs, and present the final diff and approval actions. Sentinel—not an agent or the UI—owns task state, permissions, worktrees, validation, and completion decisions.

## V3 commitments

- Preserve the Tauri + React + Rust foundation and all working V1 behavior.
- One implementation task maps to one Sentinel task, Git branch, and worktree; the main working tree is protected.
- Use supported Codex and Claude Code interfaces through native adapters. Hermes is not a core dependency.
- Persist normalized events, task state, artifacts, approvals, and recovery facts locally.
- Deterministic checks outrank reviewer opinions. A human must approve commit, merge, push, cleanup that discards work, and other destructive operations.
- Keep the app ambient: tray-first, global hotkey, floating prompt, attention/status widget, focus restoration, and no unnecessary Dock or Cmd+Tab presence.

## Ordered V3 delivery

| Phase | Outcome | Exit criterion |
| --- | --- | --- |
| 1 | Unified event and durable task model | Versioned events, replay, state transitions, and crash recovery pass contract tests. |
| 2 | Native Codex adapter | Supported Codex interface starts, streams, cancels, resumes/recoverably reconciles, and passes adapter fixtures. |
| 3 | Native Claude Code adapter | Supported Claude Code CLI/session interface meets the same bounded contract. |
| 4 | Worktree task transactions | Create/diff/merge/discard/rollback/cleanup are ownership-bound and protect the main tree. |
| 5 | Validation and command profiles | Configurable deterministic checks execute in the worktree with durable artifacts. |
| 6 | Read-only review and bounded repair | Structured review findings and capped repair rounds reach a reliable final-decision boundary. |
| 7 | Attention widget and approval surfaces | Ambient controls expose truthful status and require confirmed approval for consequential actions. |
| 8 | Recovery, security, and license hardening | Restart, permission, artifact redaction, provenance, and license gates pass. |
| 9 | Final UI redesign | The end-to-end V3 workflow is usable without disrupting the current app. |

Detailed scope, dependencies, validation, rollback, commit boundaries, and deferrals are in [docs/v3-implementation-plan.md](docs/v3-implementation-plan.md).

## Explicitly deferred from V3 core

Cloud synchronization, teams, remote execution, automatic pull requests, IDE/browser plugins, mobile clients, background daemon extraction, autonomous commit/merge/push, automatic worktree deletion, LLM-based approval decisions, arbitrary third-party adapter packs, ACP/OpenCode/Hermes adapters, and quant-development workflow packs are deferred. These may become optional packs only after the V3 core is stable.

## Immediate next action

V3 planning and scope migration is complete. Do not begin implementation until a focused Phase 1 design/implementation task is approved against the V3 documents.
