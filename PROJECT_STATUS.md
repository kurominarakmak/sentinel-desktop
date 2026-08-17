# Agent Sentinel Project Status

## Authority

V3 product, architecture, and workflow decisions are respectively defined by [V3 product scope](docs/v3-product-scope.md), [V3 target architecture](docs/v3-target-architecture.md), and [V3 workflow state machine](docs/v3-workflow-state-machine.md). [PLAN.md](PLAN.md) is the V3 delivery order. V1 documents and phase contracts are preserved historical records of the existing implementation; where they conflict with V3 future planning, the V3 source documents govern.

## Current status

### PRESENT

- V3 Phase 1 durable foundation in `sentinel-core`: provider-neutral task state, explicit transition guards, append-only normalized events, and additive SQLite migration `0010_v3_workflow_foundation.sql`.
- Durable opaque provider-session metadata, approvals, validation results, review findings, repair rounds, and artifact metadata. The Rust repository is the only write authority exposed by this phase.
- V3 restart posture: unfinished non-draft tasks restore as `Recovering` with `NoLiveProcessAssumed`; active sessions become `RecoveryRequired`. Completion is never inferred from a missing external process, and V1 rows remain intact.
- macOS Tauri 2 + React + Rust application foundation with SQLite persistence.
- Reusable `sentinel-core`, `sentinel-runtime`, `sentinel-process`, `sentinel-git`, and `sentinel-agent-api` crates, plus fake-agent fixtures and tests.
- Tray application, `Command+Shift+Space` global hotkey, hidden floating prompt, and status/settings window routes.
- Existing project/worktree, process-supervision, normalized-event, approval, drift, evidence, and restart-recovery foundations with automated regression coverage.

### COMPLETE WITH KNOWN LIMITATIONS

- V3 Phase 2 native Codex adapter: the Sentinel-owned desktop App Server manager lazily owns one process, starts and resumes task-scoped threads, streams normalized events into durable V3 state, persists thread/turn/session metadata, and supports cancellation without treating `turn/completed` as Sentinel task completion. Capability negotiation and deterministic protocol/lifecycle fixtures pass.
- Real authenticated local validation in a disposable Git repository passed for App Server initialization, thread/turn start, normalized durable streaming, and clean-shutdown thread resume. Deterministic in-flight interruption and crash/recreation coverage pass.
- A real turn could not be held in flight reliably for interruption, and a real `thread/resume` after abrupt owned-App-Server death currently returns `RpcError`; both are documented provider limitations, deferred to Phase 8 recovery hardening rather than Phase 2 blockers.

### COMPLETE WITH AUTH-LIMITED VALIDATION

- V3 Phase 3 native Claude Code adapter: Sentinel owns a supported non-interactive `--print --output-format stream-json` child, persists the opaque Claude session ID, streams normalized V3 events, cancels only its owned process, resumes only Sentinel-owned session records, and reconciles persisted records after restart. Executable/version detection, supported stream-json/resume interfaces, and real CLI JSON framing were verified. Deterministic fake-Claude start/stream/cancel/resume/malformed/exit/restart fixtures pass. The local `claude 2.1.201` binary was not logged in, so authenticated provider lifecycle validation is deferred to Phase 8 or when credentials become available; no login or account change was attempted.

### COMPLETE
- V3 Phase 4 is **COMPLETE**. Task worktrees are ownership-bound from creation
  through reconciliation, pinned-base diff and merge preparation, and explicit
  approval-gated discard/retention. Destructive preflight refreshes identity,
  branch, HEAD, and cleanliness and never touches the primary or unrelated trees.

### COMPLETE

- V3 Phase 5 is **COMPLETE**. Repository-scoped build/lint/test/custom profiles
  run ordered fixed-argv checks only in reconciled task worktrees, with allowed
  environment, durable redacted results/artifacts, supervisor invocation, and
  explicit restart interruption recovery.

### COMPLETE

- V3 Phase 6 is **COMPLETE**. Read-only review findings, bounded confirmed
  repair/re-review rounds, fresh deterministic validation, and durable final
  approval packets are composed without allowing reviewer/implementer text to
  resolve findings, finalize tasks, or execute a Git final action.

### COMPLETE

- V3 Phase 7 is **COMPLETE**. Quick Prompt, the event-driven Attention Widget,
  and durable Task Detail expose V3 task state, review/validation evidence,
  repair history, and human approval actions without inferring workflow state.

### COMPLETE

- V3 Phase 8 is **COMPLETE**. Provider/session recovery, execution and
  credential boundaries, exact single-use authorization, and provenance plus
  adversarial recovery audit evidence are covered by deterministic V3 suites.

### COMPLETE

- V3 Phase 9 is **COMPLETE**. The compact desktop shell, Quick Prompt,
  Attention Widget, Task Detail, Status, and Settings share the final V3
  visual system while durable supervisor state remains the only workflow
  authority.

### NATIVE MIGRATION — CODE COMPLETE, PARITY ACCEPTANCE PENDING

- The SwiftUI/AppKit menu-bar shell implements Quick Prompt, Attention Widget,
  Task Detail, Status, and Settings through a bundled newline-delimited JSON
  sidecar. Swift is presentation-only.
- The shared Rust `sentinel-supervisor` creates and reconciles the owned
  worktree before provider startup, advances deterministic validation and
  read-only cross-model review, bounds repair rounds, and creates the durable
  human-approval packet. Provider completion never finalizes a task.
- Bridge mutation IDs/responses are durable and intent-bound. Reconnect reloads
  fresh Rust snapshots and never automatically replays an unproven mutation.
- The Tauri frontend remains present as the previous frontend/reference. Native
  promotion still requires accepted hands-on macOS interaction QA and release
  signing/notarization policy; automated controller tests do not prove those.

### PLANNED

- Complete hands-on native parity acceptance across focus restoration, Spaces,
  fullscreen applications, multiple displays, sleep/wake, provider death, and
  bridge restart.
- Decide whether to promote Native Swift and retire Tauri only after that
  evidence exists. Configure Developer ID signing/notarization before external
  distribution.

### DEFERRED

- Cloud synchronization, teams, remote execution, automatic pull requests, IDE/browser plugins, mobile clients, daemon extraction, autonomous commit/merge/push, and automatic worktree deletion.
- Optional adapters/packs: ACP, OpenCode, Gemini CLI, Hermes, and quant-development workflows.

## Next action

Run and record the focused native parity smoke matrix, then make the explicit
Tauri retirement decision. Do not treat unit tests as proof of unperformed
manual macOS behavior.
