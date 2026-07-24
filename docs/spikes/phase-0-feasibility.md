# Phase 0 Feasibility Report

**Status:** Complete for the currently available agent environment: Codex feasibility is verified. Claude Code is deferred because no active authenticated Claude session is available. Cross-agent validation is not complete.

## Environment

Validation environment: macOS Darwin 24.0.0 on Apple Silicon, Git 2.48.1, Node 26.0.0, npm 11.12.1, pnpm 11.6.0, Rust 1.97.1, Codex CLI 0.144.6, and Claude Code 2.1.201.

## What Was Implemented

- A Tauri 2 shell with a small always-on-top prompt window, hide-on-close behavior, tray menu actions, and explicit Quit.
- A configurable shortcut registration point using the macOS default `Command+Shift+Space`.
- A React prompt with project path, Fake Agent/Codex/Claude Code selection, multiline task input, Escape-to-hide, Enter-to-submit, automatic input focus, live events, and diagnostics.
- Reusable Rust crates for the event model, fake agent, Git worktrees, and child-process supervision.
- Manual-only agent-probe actions. Normal tests and normal UI use do not make model requests.

## Directly Observed Agent Capabilities

### Codex

`codex --version` reported `codex-cli 0.144.6`. `codex exec --help` directly confirmed `--json` JSON Lines output, working-directory selection, resume, and sandbox selection. The runner uses workspace-write access only for the disposable fixture and invokes resume using the verified `codex exec resume --json <session-id> <prompt>` syntax.

**Codex normal/resume: PASS.** The verified run `run-1784888797000884000` exited `0`, captured 11 structured events across 5 event types, and recorded session ID `019f93a9-a7a5-7850-96e2-c5ba6377d328`. Initial execution and session resume both passed. Exact `result.txt` validation and unexpected-file validation both passed, with no validation failures. Artifacts: `target/agent-sentinel-probes/run-1784888797000884000-codex-normal`.

**Codex cancellation: PASS.** The verified run `run-1784888901055693000` exited `0` with no validation failures. Cancellation latency was 5755 ms. Its stdout confirms that Codex started `sh wait.sh` and that the command was in progress when cancellation occurred. Artifacts: `target/agent-sentinel-probes/run-1784888901055693000-codex-cancellation`.

### Claude Code

**Claude Code: DEFERRED, not tested.** Local help previously confirmed `--output-format=stream-json`, `--input-format=stream-json`, `--resume`, and permission-mode options. The developer currently has no active Claude subscription or authenticated Claude session, so no Claude fixture task, session resume, event parsing, or cancellation probe was invoked. This is not a failure.

## Manual Real-Agent Probe Runner

The only real-agent probe path is the explicit `sentinel-probe` command-line binary. It is never called by normal tests, CI, application startup, or the desktop prompt. These commands can consume Codex or Claude Code usage and require an explicit confirmation flag:

```sh
cargo run -p sentinel-probe -- environment
cargo run -p sentinel-probe -- codex --confirm-real-agent
cargo run -p sentinel-probe -- claude --confirm-real-agent
cargo run -p sentinel-probe -- codex --test cancellation --confirm-real-agent
cargo run -p sentinel-probe -- claude --test cancellation --confirm-real-agent
cargo run -p sentinel-probe -- all --confirm-real-agent
```

`environment` reports executable paths and versions for Git, Codex, Claude Code, and `jq`, plus a non-secret authentication-status indicator. It does not print tokens or credential values. A real-agent command exits with `3` without `--confirm-real-agent`, `2` when the selected agent is unavailable or does not appear authenticated, `4` when safe fixture setup fails, `1` when validation fails, and `0` only after every selected validation passes.

Each real-agent run prints its temporary Git fixture path, artifact directory, stdout log path, stderr log path, and summary path before the request. It creates only `README.md`, plus `wait.sh` for a cancellation probe, and runs in that disposable repository. Normal probes instruct the agent to change only `result.txt`; cancellation probes permit only the fixture wait script before the requested completion file. The runner uses Codex workspace-write sandboxing and does not use dangerous or unrestricted permission flags. It does not push, merge, install dependencies, or intentionally access credentials or files outside the fixture.

Artifacts are written to `target/agent-sentinel-probes/<run-id>-<agent>-<test>/`: `probe-summary.json`, `normalized-events.jsonl`, `raw-agent-events.jsonl`, `stdout.log`, `stderr.log`, `fixture-path.txt`, `detected-versions.txt`, `cancellation-result.txt` when applicable, and `validation-failures.txt`. `stdout.log` and `stderr.log` are always created, including when empty. All paths and the summary share the run ID, so inspection uses the explicitly printed run directory rather than searching artifact directories by filename. Output is redacted for obvious token-shaped values. The fixture is removed only after complete success; a failed fixture is retained for inspection. Use `--cleanup` with a real-agent command to request cleanup even after failure.

The normal probe requires the agent to create the exact first line in `result.txt`, captures structured JSON Lines, records a session identifier, then resumes that session to append the exact second line. It fails for an unexpected changed file. The cancellation probe waits for the fixture `wait.sh` activity, requests graceful cancellation through `sentinel-process`, escalates after its bounded timeout when required, checks the tracked process group on Unix, verifies `completion.txt` was not created, and records cancellation latency. It retains structured events captured before cancellation in `probe-summary.json` and records whether cancellation was requested, the agent process terminated, the process group terminated, and `completion.txt` was absent.

## Worktree Behavior

`sentinel-git` creates a unique `agent-sentinel/spike-<timestamp>` branch, adds a worktree below a caller-supplied temporary directory, reports changed files through porcelain status, and refuses removal when the worktree is dirty. Its two passing tests create a temporary Git fixture and never modify the developer's working directory.

## Child-Process Behavior

`sentinel-process` captures stdout/stderr line events, waits for exit status, requests cancellation, and, on Unix, starts a new process group then sends `SIGTERM` followed by `SIGKILL` after a bounded timeout. The public interface does not expose macOS-specific types. The passing process-group test intentionally starts a child process and verifies the group no longer exists after cancellation.

## Tray And Global Shortcut Behavior

The startup prompt is configured with `visible: false`, so no normal desktop window is created visibly at launch. On macOS the setup path uses `tauri::ActivationPolicy::Accessory` and `set_dock_visibility(false)`; it does not use `set_skip_taskbar`. The tray contains Open Prompt, Show Spike Status, and Quit. Open Prompt and the shortcut both call the same opener, which shows, restores, centers, focuses, and emits a frontend input-focus event. Close requests explicitly call `prevent_close` before hiding the prompt; Escape and successful submission also hide it. Fake events emit to the frontend and update the tray title to Idle/Running/Waiting/Completed/Failed states.

`cargo build -p agent-sentinel-desktop`, the frontend production build, and the Rust workspace tests pass. A direct startup-log check confirmed `tray setup started`, `tray icon loaded`, and `tray successfully built` after the dedicated monochrome template PNG was introduced and retained in managed state. A controlled macOS executable launch started PID 16184 and was terminated by the validation command. macOS denied the automation process assistive access and keystroke injection (`System Events` errors `-25211` and `1002`), so the menu-bar icon, tooltip, menu interaction, Dock state, shortcut, focus, Escape, close-to-hide, reopening, and Quit cannot be marked as directly observed.

## Prompt Rendering Behavior

The blank prompt was caused by `main.tsx` exporting the React component without calling `createRoot(...).render(...)`. The document already contained the required `#root` element. The entrypoint now mounts the prompt through `createRoot`, has a React error boundary, and installs a DOM fallback screen for initialization failures. A local request to the existing Vite server returned HTTP 200 and its transformed `main.tsx` module contained `createRoot`, `PromptErrorBoundary`, and the Project path UI. This verifies served entrypoint wiring, not interactive browser or Tauri rendering.

The prompt now registers one capture-phase `keydown` listener on `window` when React mounts. Its directly tested behavior is to prevent and stop Escape from a focused textarea, invoke Tauri window hiding, ignore other keys, and remove the listener during cleanup. The prompt capability now explicitly includes `core:window:allow-hide`. macOS tray/window interaction remains pending direct visual validation.

## Architecture Assumptions

Validated by direct probe: Codex structured JSON event parsing, session capture and resume, exact output validation, unexpected-file validation, and cancellation all pass. Claude's CLI capability was checked locally, but its real-agent path is deferred. The crate boundaries keep reusable event, worktree, and process logic outside Tauri.

Not yet validated: cross-agent behavior with Claude, plus the documented interactive macOS checklist for tray visibility, global shortcut reliability from another active application, hide-to-tray behavior, focus, and close-to-hide.

## Known Limitations

- Claude Code remains deferred, so Phase 0 has no Claude fixture task, parsed provider event schema, session capture, follow-up/resume, or provider cancellation result.
- The macOS tray, global shortcut, focus, and close-to-hide behavior need an interactive desktop verification.
- The attempted automated macOS checklist is blocked until the validation tool receives Accessibility permission; do not treat startup logs or successful compilation as visual confirmation.
- Browser and Tauri prompt interaction, input focus, Escape, reopen, and fake-agent submission remain pending direct visual validation.
- The spike icon is a temporary local asset, not final branding.
- The probe currently records event type metadata and root-level session IDs; provider-specific normalization remains a Phase 4/5 concern.
- This spike intentionally contains no persistence, policy engine, Drift Guardian, Evidence Gate, approval system, daemon, packaging, or production UI.

## Recommended Changes Before Phase 1

Maintain the existing macOS interactive checklist for tray visibility, shortcut activation from another application, focus, and close-to-hide behavior. Do not infer Claude support from the Codex result; run the deferred Claude probes only after an authenticated Claude session is available.
