# Sentinel V3 OSS Integration Sequence

These are implementation-order constraints, not authorization to implement. Every phase retains Sentinel as workflow owner and imports, at most, the narrowly selected material named below.

## 1. Normalized event model and durable state

- **Selected upstream source:** `agent-client-protocol` as a schema candidate; `ralph-review` finding-artifact concept.
- **Sentinel modules affected:** `sentinel-core`, `sentinel-agent-api`, `sentinel-runtime`, migrations and fixture tests.
- **Acceptance tests:** Atomic task-event transition; monotonic replay; unknown-provider-event preservation; provider-session mapping; review-finding schema; restart projection; redaction and stale-version failure.
- **Rollback condition:** Any migration cannot replay V1 history or provenance/license review fails; feature-gate V3 records and retain V1 projection.
- **Expected focused commit:** `feat(core): add v3 durable workflow events`.
- **Deferred:** ACP transport, real provider processes, worktree mutation, UI redesign.

## 2. Codex adapter

- **Selected upstream source:** `codex-plugin-cc` `app-server-protocol.d.ts`; Jean only as recovery comparison.
- **Sentinel modules affected:** new `sentinel-codex`, `sentinel-agent-api`, `sentinel-runtime`, adapter fixtures.
- **Acceptance tests:** Capability probe; JSON-RPC start/notification/approval-request mapping; `thread/resume` mapping; cancellation; disconnect/restart reconciliation; malformed/unknown event handling.
- **Rollback condition:** Installed App Server differs from the supported contract or ownership cannot be proven; mark Codex unavailable and retain the task/worktree.
- **Expected focused commit:** `feat(codex): add app-server session adapter`.
- **Deferred:** Detached app-server adoption, terminal scraping, plugin import, automatic approval response.

## 3. Claude Code adapter

- **Selected upstream source:** Official supported Claude Code CLI/session interface; OpenCovibe reference-only comparison.
- **Sentinel modules affected:** new `sentinel-claude`, `sentinel-agent-api`, `sentinel-runtime`, adapter fixtures.
- **Acceptance tests:** Fixed start/resume invocation; structured stream normalization; session-ID persistence; cancellation; provider permission event mapping; restart reconciliation; no dangerous-bypass mode.
- **Rollback condition:** Required CLI/session capability is absent or a stream cannot be validated; mark Claude unavailable without fallback to terminal parsing.
- **Expected focused commit:** `feat(claude): add supported session adapter`.
- **Deferred:** Claude SDK bridge, provider-settings mutation, copied Claude code.

## 4. Worktree transaction

- **Selected upstream source:** cctop refresh-before-destruction concept; parallel-code reference-only edge cases.
- **Sentinel modules affected:** `sentinel-git`, `sentinel-core`, workflow service and transaction tests.
- **Acceptance tests:** One task/branch/worktree; main-tree protection; base/diff identity; merge preparation; approved discard/rollback/cleanup refresh/refusal; retained worktree on Git failure.
- **Rollback condition:** Any uncertain identity/effect or failed preflight; do not remove worktree or branch.
- **Expected focused commit:** `feat(git): add v3 worktree transactions`.
- **Deferred:** Automatic cleanup, force cleanup by default, auto-merge/push/PRs.

## 5. Approvals and permissions

- **Selected upstream source:** No code donor; codex-review is reference-only for finding/decision coverage.
- **Sentinel modules affected:** `sentinel-core`, policy/approval service, adapter boundary, desktop DTOs.
- **Acceptance tests:** Ownership/version-bound approval; deny/expiry/stale handling; provider request normalization; final Git-action confirmation; audit redaction.
- **Rollback condition:** An approval can execute arbitrary caller input or crosses task/worktree ownership; fail closed and disable delivery.
- **Expected focused commit:** `feat(policy): bind v3 approvals to workflow actions`.
- **Deferred:** Global auto-allow, provider bypass modes, agent-controlled approval.

## 6. Validation commands

- **Selected upstream source:** No code donor; architect-loop and ralph-review are reference-only for deterministic gates.
- **Sentinel modules affected:** `sentinel-validation`, `sentinel-core`, configuration, artifacts/tests.
- **Acceptance tests:** Declared build/lint/test profile pass/fail/timeout/cancel; worktree cwd enforcement; baseline classification; bounded redacted output; deterministic precedence over reviewer result.
- **Rollback condition:** Command/profile cannot be safely resolved inside the managed worktree; record incomplete validation and block readiness.
- **Expected focused commit:** `feat(validation): run v3 command profiles`.
- **Deferred:** Agent-selected shell commands, model-only evaluation, automatic remediation.

## 7. Bounded review and repair loop

- **Selected upstream source:** ralph-review finding-artifact concept; architect-loop role-separation reference.
- **Sentinel modules affected:** `sentinel-review`, `sentinel-workflow`, core state machine, adapter roles, tests.
- **Acceptance tests:** Read-only reviewer contract; validated finding artifact; confirmed-blocker-only handoff; bounded rounds; deterministic failure precedence; cap exhaustion remains non-ready; preserved audit artifact.
- **Rollback condition:** Reviewer can write/execute unmanaged mutation, malformed findings are accepted, or a terminal state is reached with unresolved blockers.
- **Expected focused commit:** `feat(workflow): add bounded review repair loop`.
- **Deferred:** Parallel builder fleets, cross-agent debate, unbounded retries.

## 8. Ambient status integration

- **Selected upstream source:** threadline and cctop reference-only native UX patterns.
- **Sentinel modules affected:** `apps/desktop/src-tauri`, React prompt/status/settings/task detail surfaces, focused native tests.
- **Acceptance tests:** Global quick prompt from another app; attention-required widget; frontmost-app capture and safe focus restoration; accessory/Dock posture; unavailable-native-capability fallback.
- **Rollback condition:** Native focus restoration is unreliable or user focus changed; return to ordinary task-detail behavior without forcing activation.
- **Expected focused commit:** `feat(desktop): add v3 ambient attention status`.
- **Deferred:** Native Swift helper import, transcript scanning, IDE/browser extension.

## 9. Diff and final approval

- **Selected upstream source:** parallel-code reference-only review/diff freshness patterns.
- **Sentinel modules affected:** `sentinel-git`, workflow/approval services, React Task Detail, tests.
- **Acceptance tests:** Diff bound to task/base/worktree identity; check/finding packet; stale approval refusal; explicit commit/merge/push/discard outcomes; no UI-only action.
- **Rollback condition:** Diff identity changes after presentation or final action preflight changes; invalidate approval and return to `ReadyForHuman`/blocked state.
- **Expected focused commit:** `feat(desktop): add final diff approval packet`.
- **Deferred:** Auto-commit/merge/push, PR provider integration.

## 10. Crash recovery and hardening

- **Selected upstream source:** Jean/OpenCovibe reference-only recovery comparisons; cctop cleanup refusal pattern.
- **Sentinel modules affected:** `sentinel-core`, `sentinel-runtime`, `sentinel-process`, `sentinel-git`, migrations, CI checks.
- **Acceptance tests:** Crash at each durable transition; owned-child/session reconciliation; supported resume; unknown-process fail-closed; retained worktree; secret redaction; dependency/provenance scan.
- **Rollback condition:** Identity/session ownership cannot be re-established, source provenance is incomplete, or destructive recovery is uncertain; mark blocked/interrupted and preserve artifacts.
- **Expected focused commit:** `feat(reliability): harden v3 recovery boundaries`.
- **Deferred:** Background daemon extraction, remote recovery, automatic cleanup.
