# Product Requirements

## Problem And User

Agent Sentinel helps a developer supervise autonomous coding tasks without giving up control of a real local repository. Its target user is a developer who already uses Codex or Claude Code and needs task isolation, visible progress, approval records, and deterministic verification.

## Core Experience

From a global shortcut, the user selects a registered Git project and agent, describes a task, and starts it in a task-specific Git worktree. The prompt can close while the long-running desktop application remains in the tray or macOS menu bar. The user monitors actual agent activity, gives follow-up instructions, decides approval requests, reviews the diff, and accepts a task only after evidence passes.

## Main User Flows

1. Register a local Git repository with a base branch, default agent, autonomy profile, protected paths, and evidence commands.
2. Open the prompt, submit a task or plan-only request, and create a branch and isolated worktree.
3. Monitor agent-reported plan steps separately from actual commands, file changes, and evidence.
4. Send a follow-up to an active task, queue it after the current turn, or start another task.
5. Review and record an approval decision, handle a drift finding, cancellation, or restart recovery.
6. Review a diff and final outcome: Completed, Completed with warnings, Evidence incomplete, Failed, or Cancelled.

## Functional Requirements

- Support Codex and Claude Code through normalized structured CLI adapters.
- Create every implementation task in an isolated Git worktree and task branch.
- Persist projects, tasks, activity, approvals, drift findings, and evidence in SQLite.
- Provide Safe, Balanced, Autonomous, and Custom policies with hard safety boundaries in every profile.
- Include basic deterministic Drift Guardian and Evidence Gate behavior in the MVP.
- Detect Git and agent installations; never silently install or alter AI CLIs.

## Non-Functional Requirements

The app is open source, local first, and requires no Agent Sentinel cloud account. It uses authentication owned by existing agent CLIs. The frontend is not authoritative for task state; task transitions are enforced in Rust. The first implementation is one long-running Tauri process and must recover persisted task history after restart.

## MVP Boundaries

The MVP is the macOS workflow for registered repositories, both agents, worktree isolation, live status, follow-up, cancellation, approvals, protected-path drift warnings, evidence checks, final diff review, and restart persistence. It excludes semantic undo, cloud sync, teams, remote execution, automatic pull requests, IDE plugins, mobile, LLM-based drift scoring, and automatic rule learning.

## Differentiation And Success

The product is not merely a prompt launcher: it captures an idea from anywhere, runs an existing agent safely, and requires evidence before completion. The first public alpha succeeds when a macOS user can complete the full MVP workflow without using a terminal except for initial agent installation or authentication. See the [roadmap](../PLAN.md).
