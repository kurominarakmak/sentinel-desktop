# V3 Phase 8C provenance and recovery audit

Audit date: 2026-08-13. This is an evidence index, not a claim that external source code was copied. Sentinel's V3 implementation is locally authored.

## OSS behavior provenance

| Source | Revision | Source module | Borrowed behavior | Reuse type | Sentinel location | Sentinel proof |
|---|---|---|---|---|---|---|
| `codex-plugin-cc` | `db52e28f4d9d` | `plugins/codex/scripts/lib/app-server-protocol.d.ts` | Codex App Server thread lifecycle and opaque thread identifiers | behavioral reference | `crates/sentinel-codex/src/lib.rs`, `apps/desktop/src-tauri/src/codex_sessions.rs` | Codex session restart/idempotency tests |
| `coollabsio/jean` | `7c00b9016358` | `jean-core/src/chat/codex_server.rs` | Restart requires a persisted owned thread and a new owned server; absence is not completion | behavioral reference | `crates/sentinel-core/src/v3.rs`, `apps/desktop/src-tauri/src/codex_sessions.rs` | core restart and missing-session tests |
| `AnyiWang/OpenCovibe` | `a8f8fddcd87b` | `src-tauri/src/agent/{session_actor,codex_appserver,claude_stream}.rs` | Ordered normalized events and owned-process cancellation/recovery boundaries | behavioral reference | `crates/sentinel-{core,codex,claude}/src` | event replay and Claude adapter recovery tests |
| `alamops/agetor` | `7d28f2a071bc` | `src/bun/{worktree.ts,task-events.ts,approval.ts}` | Pinned worktree/base evidence, durable event deduplication, explicit approval records | behavioral reference | `crates/sentinel-{core,worktree,review}/src` | worktree mismatch and action authorization tests |
| `cctop` | `0f7baee99a3a` | `menubar/CctopMenubar/Services/PanelCoordinator.swift` | Explicit recovery/attention display states; no state inference in surface logic | behavioral reference | `apps/desktop/src/{V3AttentionWidget,V3Workflow}.tsx` | focused widget/workflow tests |

No direct reuse or ported external source is claimed for these rows. Existing license and scope records remain [v3-oss-audit.md](v3-oss-audit.md) and [v3-selected-donors.md](v3-selected-donors.md).

## Adversarial recovery evidence

| Boundary | Deterministic coverage | Fail-closed result |
|---|---|---|
| Desktop restart during active provider work | `sentinel-core/tests/v3_workflow.rs::restart_restores_active_tasks_without_assuming_a_live_provider` | Task/session become recovery-required; completion is not inferred. |
| Codex process death and repeated reconciliation | `codex_sessions::{missing_session_and_dead_server_leave_or_recreate_explicitly,repeated_reconciliation_reuses_the_owned_process_and_thread}` | Only an owned App Server is recreated; a missing thread remains unrecovered. |
| Claude restart/auth-limited recovery | `sentinel-claude/tests/adapter.rs::restart_reconciliation_returns_only_sentinel_owned_recovery_sessions` | Only owned metadata is returned; unavailable provider proof does not resume. |
| Validation restart | `sentinel-validation::restart_recovery_marks_running_incomplete` | Running validation is explicitly incomplete. |
| Review/repair restart | `sentinel-review::repair_round_and_finding_assignment_survive_restart` | Findings and round lifecycle persist without inferred resolution. |
| Missing/inconsistent worktree | `sentinel-worktree::{missing_worktree_becomes_recovery_required,mismatched_branch_becomes_recovery_required,mismatched_base_becomes_recovery_required}` | Worktree is retained and recovery-required. |
| Corrupt/incomplete state | `sentinel-core/tests/v3_workflow.rs::existing_v1_database_migrates_additively_and_corruption_fails_closed` | State decoding fails closed. |
| Duplicate/replayed events | `sentinel-core/tests/v3_workflow.rs::events_are_ordered_unknown_events_are_preserved_and_replay_is_idempotent` | Replay does not duplicate authoritative events. |
| Consumed/stale approval and racing consumers | `sentinel-review/tests/action_authorization.rs` | Versioned single-use capability has one concurrent winner and remains consumed after reopen. |

## Security regression evidence

| Guarantee | Proof |
|---|---|
| No persisted secrets | `sentinel-validation::credential_redaction_tests::secret_bearing_diagnostics_are_redacted_before_persistence`. |
| No primary/unrelated worktree execution | `sentinel-validation::invalid_profile_and_wrong_worktree_are_refused`. |
| Model text has no approval authority | `sentinel-review` approval requires an explicit durable `ApprovalId` transition; reviewer/implementer tests cannot grant it. |
| Exact single-use action approval | `sentinel-review/tests/action_authorization.rs`. |
| Reviewer remains read-only | `sentinel-review::review_has_no_write_authority_and_never_finalizes_the_task`. |
| Provider completion does not complete Sentinel task | `sentinel-claude/tests/adapter.rs::exit_is_recorded_without_completing_the_sentinel_task`. |
| Interrupted work is never successful by inference | core restart and validation restart tests above. |

Known scope: the authenticated local Claude smoke is intentionally ignored unless explicitly authorized and logged in. No test masks that environmental limit.

Broader `cargo test --workspace` was run during this audit. It completed all
V3-focused suites but has one pre-existing, unrelated desktop B1 fixture
failure: `bridge_tests::production_b1_classification_uses_fresh_inventory_and_preserves_both_worktrees`
returned `git_command_failed` from its inventory fixture (`main.rs:4360`). It
is recorded here rather than changed or masked by Phase 8C.
