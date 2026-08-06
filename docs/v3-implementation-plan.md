# Sentinel V3 Implementation Plan

This is the ordered plan for V3 implementation. Each phase preserves working V1 behavior until its replacement is tested. Commits below are expected focused commits, not authorization to commit until their documented gates pass.

## 1. Unified event and durable task model

- **Exact scope:** Version the normalized event envelope; add the V3 task aggregate, workflow/round/finding/approval/artifact records, atomic transition persistence, projection/replay, and recovery decision records.
- **Affected modules:** `sentinel-core`, `sentinel-agent-api`, `sentinel-runtime`, migrations/tests; thin desktop DTO additions only.
- **Dependencies:** Existing SQLite/event/run foundation and fake-agent fixtures.
- **Tests/validation:** Rust unit/integration transition, migration, ordering, redaction, stale-version, replay, and crash-recovery tests; `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **Failure/rollback:** Migration failure, incompatible event replay, or uncertain state blocks rollout; retain V1 tables/projections and ship a reversible schema migration only after backup/restore testing.
- **Expected commit:** `feat(core): add v3 durable task and event model`.
- **Deferred:** Real provider transport, worktree mutations, review, and UI redesign.

## 2. Native Codex adapter

- **Exact scope:** Capability probe and supported Codex interface implementation for start/stream/cancel/resume-or-reconcile; strict normalization, fixed worktree binding, and protocol fixtures. App Server is selected only when the probe proves the supported capability contract; otherwise an official supported fallback is explicit.
- **Affected modules:** new `sentinel-codex` (or isolated equivalent), `sentinel-agent-api`, `sentinel-runtime`, config, fake protocol fixtures.
- **Dependencies:** Phase 1 model; current official interface compatibility spike.
- **Tests/validation:** Fixture contract tests for discovery, lifecycle/events, malformed input, cancellation, resume/recovery, redaction, and ownership; all Phase 1 gates; manual opt-in local CLI smoke test that performs no user-repo mutation.
- **Failure/rollback:** Unsupported/version-drifted protocol marks Codex unavailable; feature flag disables the adapter and preserves records/worktrees.
- **Expected commit:** `feat(codex): add supported v3 session adapter`.
- **Deferred:** Hermes, autonomous approval delivery, external source copying.

## 3. Native Claude Code adapter

- **Exact scope:** Implement supported Claude Code CLI/session transport, structured stream normalization, start/resume/cancel/recovery capabilities, and explicit permission-event mapping. Never use dangerous permission bypass modes.
- **Affected modules:** new `sentinel-claude`, shared API/runtime/config, fixture tests.
- **Dependencies:** Phase 1 and a current Claude CLI compatibility spike; interface behavior documented by Anthropic.
- **Tests/validation:** Equivalent adapter contract suite, including JSON stream framing, session resume, permission mapping, cancellation races, redaction, and unsupported capability behavior; full Rust validation.
- **Failure/rollback:** Capability mismatch leaves the adapter unavailable and tasks recoverable/inspectable; no fallback to terminal scraping or copied Claude code.
- **Expected commit:** `feat(claude): add supported v3 session adapter`.
- **Deferred:** SDK bridge unless a later separately approved interface gap requires it; Hermes.

## 4. Worktree task transactions

- **Exact scope:** Make creation, diff, merge preparation, discard, rollback, cleanup, and retention an ownership-bound transaction with main-tree protection and human-gated destructive actions.
- **Affected modules:** `sentinel-git`, `sentinel-core`, supervisor workflow service, migrations, Tauri commands.
- **Dependencies:** Phase 1 identity/approval model and existing Git foundation.
- **Tests/validation:** Temporary-repository tests for conflicts, concurrent tasks, main-tree immutability, failed creation, diff identity, rejected stale approvals, rollback/cleanup failure retention; existing Git tests plus Rust validation.
- **Failure/rollback:** Any uncertain Git effect retains the worktree and records `cleanup_failed`; never reset/rewrite the main tree. Feature-gate destructive UI until recovery paths pass.
- **Expected commit:** `feat(git): add v3 worktree task transactions`.
- **Deferred:** Automatic cleanup, auto-merge, auto-push, PR creation.

## 5. Validation and command profiles

- **Exact scope:** Add project/workflow command profiles for build, lint, test, baseline, required/optional semantics, execution limits, redacted artifacts, and deterministic verdicts.
- **Affected modules:** new `sentinel-validation`, `sentinel-core`, configuration, desktop presentation/tests.
- **Dependencies:** Phases 1 and 4.
- **Tests/validation:** Fixture projects covering pass/fail/timeout/cancellation/baseline regression/redaction/profile precedence; full Rust/frontend checks and command-profile integration suite.
- **Failure/rollback:** Missing, unsafe, or non-deterministic configuration is not run; failed commands preserve worktree/artifacts and block final readiness according to profile.
- **Expected commit:** `feat(validation): add workflow command profiles`.
- **Deferred:** Model judging of test output and automatic remediation.

## 6. Review and bounded repair workflow

- **Exact scope:** Add a read-only reviewer role, structured findings/dispositions, deterministic priority rules, confirmed-blocker handoff, capped repair rounds, and final decision packet.
- **Affected modules:** new `sentinel-review` and `sentinel-workflow`, adapters, core records, UI projections.
- **Dependencies:** Phases 1–5 and at least one usable implementer and reviewer adapter capability.
- **Tests/validation:** State-machine/property tests for every round boundary, read-only capability tests, schema/evidence validation, cap enforcement, deterministic-failure precedence, reviewer disagreement, and recovery between rounds.
- **Failure/rollback:** Reviewer unavailability or invalid evidence produces a visible non-clearable state, not a fabricated pass; cap exhaustion stops repair and retains all artifacts.
- **Expected commit:** `feat(workflow): add read-only review and bounded repair`.
- **Deferred:** Multi-agent debate, autonomous reviewer approval, unbounded retries.

## 7. Attention widget and approval surfaces

- **Exact scope:** Implement tray/attention widget, global quick prompt, floating status, focus restoration, accessible task window, and explicit approval/diff/final-action surfaces over supervisor DTOs.
- **Affected modules:** `apps/desktop/src-tauri` native window/tray code, React components/styles/tests, bridge DTOs.
- **Dependencies:** Phases 1, 4–6; existing V1 tray/hotkey shell.
- **Tests/validation:** React interaction tests; macOS manual acceptance for Safari/VS Code focus restoration, hotkey, Dock/Cmd+Tab posture, accessibility, approval-version staleness, and no background final action; production build.
- **Failure/rollback:** Native capability failure falls back to the normal task window and declares unavailable status; UI never directly performs destructive Git actions.
- **Expected commit:** `feat(desktop): add v3 ambient attention and approvals`.
- **Deferred:** Browser/IDE extension, notifications/autostart beyond explicitly tested native capability.

## 8. Recovery, security, and license hardening

- **Exact scope:** Harden restart/reconciliation, direct-child control, artifact retention/redaction, permission auditing, security posture, OSS provenance/notice enforcement, and migration resilience.
- **Affected modules:** core/runtime/process/storage, security docs, CI scripts, `THIRD_PARTY_NOTICES.md` if material is adopted.
- **Dependencies:** Phases 1–7 and completed source-specific license reviews.
- **Tests/validation:** Fault injection for persistence/process/app crashes, secret-redaction corpus, permission/approval audit tests, worktree recovery tests, license inventory/provenance CI, full desktop/Rust suite.
- **Failure/rollback:** Unknown runtime ownership becomes safe blocked/interrupted; uncertain license/provenance prevents merge/release; retain worktrees/artifacts for inspection.
- **Expected commit:** `feat(reliability): harden v3 recovery security and provenance`.
- **Deferred:** Daemon extraction, cloud telemetry/sync, remote execution.

## 9. Final UI redesign

- **Exact scope:** Replace V1 presentation incrementally with the cohesive V3 task timeline, final decision packet, workflow selection, artifacts, findings, and settings while maintaining backend ownership.
- **Affected modules:** React app, styles, Tauri DTO adapters, visual/manual test assets.
- **Dependencies:** All prior phases, especially stable supervisor DTOs and macOS ambient behavior.
- **Tests/validation:** UI unit/integration/accessibility tests, visual regression where available, manual macOS complete workflow from external app through final approval, production/Tauri build, full regression suite.
- **Failure/rollback:** Keep the existing V1 shell route behind a migration/feature flag until parity passes; no data migration may drop task history.
- **Expected commit:** `feat(desktop): complete v3 workflow experience`.
- **Deferred:** Cross-platform redesign, third-party packs/adapters, cloud/teams/mobile.

## Cross-phase rules

Do not clone or copy external repositories during implementation without the OSS adoption gate. Do not perform unrelated refactors. Do not allow an agent, UI, or adapter to own task finalization, permissions, worktree lifecycle, or validation policy. Review the affected V3 documents at each phase gate for contradictions, capability claims, and unsupported assumptions before committing.
