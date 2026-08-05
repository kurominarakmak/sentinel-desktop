# Phase 6 Authoritative Execution Contract

## Authority and objective

[PLAN.md](../PLAN.md) defines Phase 6 as **Desktop UX: Prompt, tray, task
detail, notifications, autostart**, exiting when normal workflow needs no
terminal beyond agent installation/authentication. This contract decomposes
that outcome while preserving Phase 3–5 authority. PLAN remains unmodified.

## Product boundary

Phase 6 makes existing backend-owned projects, managed worktrees, agent
capabilities, and redacted runs usable through a coherent desktop prompt:
project selection, agent selection, bounded task input, history/task detail,
availability, cancellation, tray opening, and notification preferences. The UI
is intent only; it never becomes authority for paths, worktrees, IDs,
lifecycle, process handles, or agent execution.

Phase 6 does not implement Phase 7 profiles, approvals, protected-path policy,
audit decisions, drift, evidence, remote execution, automatic cleanup, merge,
push, App Server execution, session/process reconnection, or arbitrary
filesystem/process controls. When an agent capability is unavailable, the UI
states this truthfully and does not imply that a task ran.

## Dependency order

1. Define minimal redacted desktop discovery DTOs for projects, worktrees, and
   agent capability. Refreshes use existing authoritative backend commands.
2. Add a frontend desktop-prompt model with explicit selected project,
   worktree, agent, task, availability, and stale-request generations. Agent
   choice is a UI preference only; the backend remains the execution authority.
3. Integrate task history/detail, terminal polling cleanup, duplicate-start
   prevention, cancellation confirmation, window-focus restoration, and tray
   opening with bounded refreshes.
4. Add notification and autostart capability posture: only supported
   backend-confirmed states may be displayed; unavailable platform support is
   explicit and no hidden OS configuration is changed.
5. Test UI ordering, unmount cleanup, unavailable behavior, redaction, and
   Phase 3–5 regressions using fake services and temporary resources only.

## Authority, safety, and resources

Public requests contain only typed opaque project/worktree selection and
bounded task/agent choice. The backend re-resolves all association and path
authority. No public DTO includes raw paths, prompts after submission, events,
argv, environment, PID, database IDs, session tokens, or runtime structures.
Unknown/stale data gives stable redacted errors. Refresh and polling are
bounded, generation-ordered, terminal-aware, and disposed on unmount.

No Phase 6 migration is needed unless a user-visible preference cannot be
represented safely; notification/autostart capability is runtime-derived and
not persisted as an OS claim. Cancellation remains the existing ownership-
scoped direct-child operation; restart retains history and does not reconnect
processes/sessions. Automatic worktree removal remains disabled.

## Tests and completion

Use fake services/executables, temporary databases/repositories/worktrees, and
no network or product-agent process. Cover project/worktree/agent selection,
invalid and unavailable selection, duplicate submission, stale async response,
terminal cleanup, cancellation feedback, unmount disposal, safe presentation,
and capability posture. Run the complete Rust/frontend gate plus Phase 5,
Phase 4, and Phase 3 isolation regressions. Phase 7 remains not started.
