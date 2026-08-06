# Sentinel V3 Product Scope

## Product definition

Agent Sentinel is an ambient macOS supervisor for coding agents. It is available over the user's active Safari, VS Code, or other application without taking over their workspace. A user supplies a prompt, selects a repository and workflow, and Sentinel creates an isolated worktree, runs a supported coding agent, verifies and reviews the result, manages a bounded repair loop, and asks a human to approve any consequential final action.

## Primary workflow

```text
prompt → repository + workflow → isolated worktree → implementer session
      → normalized events → deterministic checks → read-only review
      → confirmed blockers → bounded repair loop → final diff/checks/findings
      → human approval for commit, merge, push, discard, or cleanup
```

## Required V3 capabilities

1. **Ambient macOS experience.** Tray-first operation, global hotkey, floating quick prompt, status/attention widget, restoration of the previously focused app, and no unnecessary Dock or Cmd+Tab presence.
2. **Direct agent integrations.** Native, supported interfaces for Codex and Claude Code that can start, resume where supported, stream, cancel, and recover/reconcile sessions. Hermes is not required by or embedded in the core.
3. **Durable supervision.** Sentinel persists task state, normalized events, artifacts, approvals, review rounds, and recovery facts. It owns transition and completion decisions.
4. **Workspace transactions.** A task has one branch and one worktree. Sentinel supports inspectable diff, merge preparation, discard, rollback, and cleanup while protecting the user's primary tree.
5. **Validation and review.** Workflow command profiles run build/lint/test checks. A reviewer operates read-only and returns structured severity/evidence findings. Deterministic failures always block ahead of model opinion.
6. **Bounded repair.** Only confirmed blocking findings are returned to the implementer, with a configured maximum repair-round count and a full audit trail.
7. **Extensibility.** Stable adapter and workflow-preset interfaces, generic artifacts and metrics, and later optional packs/adapters without diluting the core ownership boundary.

## Ownership and non-goals

| Owner | Responsibilities | Must not decide |
| --- | --- | --- |
| Sentinel supervisor (Rust) | state, transitions, worktrees, policies, approvals, validation, review-loop bounds, completion | provider behavior or a human's final approval |
| Agent adapters | provider discovery, session transport, provider-event normalization, capability reporting | task completion, Git lifecycle, policy, validation verdict |
| Implementer/reviewer agents | proposed code, messages, read-only findings | permission grants, worktree operations outside assigned commands, terminal outcome |
| React/Tauri UI | prompt, presentation, focus/window behavior, explicit user actions | authority, hidden state transitions, auto-approval |
| User | repository/workflow choice and final consequential approval | low-level orchestration ordering |

V3 does not promise a sandbox against a compromised local account, repository, agent binary, or OS. It does promise transparent ownership boundaries, worktree isolation, redaction, and fail-closed decisions when required evidence or authority is absent.

## Deferred capabilities

No cloud service, teams, remote runners, IDE/browser extensions, mobile client, background daemon, automatic PR creation, autonomous commit/merge/push, automatic deletion of unreviewed worktrees, semantic undo, agent-generated permissions, or third-party adapter is included. ACP, OpenCode, Hermes, and quant-development workflows are future optional packs, not V3 dependencies.
