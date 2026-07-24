# Agent Adapter Contract

## Required Contract

Every `AgentAdapter` identifies its kind, detects an installation, starts a session in a supplied worktree, sends a follow-up instruction, interrupts and cancels the session, and responds to an approval decision. Adapters translate all provider-specific activity into a normalized event stream; the task engine must not parse Codex or Claude output directly.

Required normalized events are session started, plan updated, message, command requested/started/completed, file changed, approval required, waiting for input, turn completed, and session failed. Events retain provider session IDs, command IDs, exit codes, summarized output, changed paths, and error context where available.

## Session And Process Behavior

Detection verifies executable availability, version, and usable authentication without storing credentials. Discovery order is explicit configured path, process `PATH`, standard platform paths, login-shell resolution, then a user-selected executable; resolved paths are stored per agent. Start receives task intent, policy, worktree, and event sender. Follow-up instructions are sent to a live session where supported or queued/resumed with the stored session ID. Interrupt requests a graceful stop; cancellation terminates the process tree and results in a terminal task outcome after cleanup.

An adapter reports unsupported capabilities rather than simulating them. Approval handling maps provider permission prompts to Agent Sentinel approval requests and accepts only a recorded policy decision. Parse errors, unavailable executables, authentication failures, process exits, invalid provider events, and reconnection failures become explicit session errors with preserved diagnostic metadata.

## Codex Strategy

The required initial integration is `codex exec --json` JSON Lines for deterministic event parsing, working-directory enforcement, command/file reporting, cancellation, final output, and errors. Codex App Server may support persistent interactive workflows, authentication, history, approvals, and streaming events, but is experimental. Keep `exec_adapter`, `app_server_adapter`, `event_normalizer`, protocol versioning, compatibility tests, and `codex-exec`/`codex-app-server` feature flags isolated. Prefer App Server only when supported and healthy; use structured exec for one-turn tasks and state any steering limitation clearly.

## Claude Code Strategy

The required initial integration is structured `stream-json` input/output, capturing session IDs, tool/Bash/file events, permissions, turn completion, streaming follow-up, and restart resume. When continuous streaming is unavailable, use `claude --resume <session-id> -p "<follow-up>"`. A small TypeScript SDK bridge is optional only when CLI support cannot reliably provide fine-grained permission callbacks, mid-turn steering, interrupts, reconnection, or structured hooks. It contains no business logic; Rust remains authoritative for policy, Git, evidence, drift, storage, and task state. Do not use `--dangerously-skip-permissions`.

## Capabilities And Tests

Required: detection, start, normalized lifecycle events, command/file events when exposed, failure reporting, cancellation, and persisted session metadata. Optional: mid-turn steering, interactive approvals, reconnect, rich plan steps, and App Server/SDK facilities. Contract tests cover detection, start, session/command/file events, follow-up, cancellation, failure, and recovery metadata. Capability differences must be visible to the task engine and UI.
