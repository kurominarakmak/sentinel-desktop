# Phase 9: Evidence Gate authoritative execution contract

## Objective

PLAN.md defines Phase 9 as **Evidence Gate — Baselines, commands, final
evaluator, reruns**. Its exit criterion is: **an agent claim alone cannot
display Completed**.

Phase 9 supplies a deterministic, read-only final evaluator. It does not run
agent commands. In this MVP, command evidence is deliberately unsupported:
only managed baseline evidence and the fixed Phase 8 Drift Guardian result are
authoritative. This is fail-closed rather than treating an agent's text or
lifecycle claim as evidence.

## Closed catalog

The catalog is `1` and contains exactly `EG001` version `1`, **Managed
baseline evidence**. Its subject is one owned managed project/worktree pair.
Its required baseline is that the worktree's recorded base commit remains
associated with a strong managed repository identity. Its required observed
evidence is the Phase 8 canonical Drift Guardian evaluation.

`EG001` passes only when the managed worktree is `ready` and Drift Guardian
has no findings. It fails when retained dirty state or a Drift Guardian finding
contradicts that condition. It is unavailable for other non-ready managed
states. Ownership, identity, inventory, storage, resource, and cancellation
failures are redacted fatal evaluation errors; they are not findings and are
not converted into a pass or unavailable result.

The deterministic reason states the required baseline, observed source
category, comparison, and limitation. It never asserts agent execution, user
intent, root cause, remediation, or runtime approval delivery.

## Evidence model and boundaries

The backend accepts only typed ProjectId and WorktreeId inputs and resolves
ownership, repository identity, baseline commit, and inventory itself. Before
evaluation, the desktop bridge reuses the Phase 3 authoritative inventory
path. Phase 8 is reused as-is; its fixed DG001/DG002 explanations and
fingerprints are not regenerated or mutated.

An ephemeral evidence bundle contains schema/catalog versions, managed
repository and worktree fingerprints, base commit, Drift snapshot fingerprint,
and deterministically ordered Drift finding fingerprints. It excludes paths,
raw Git output, commands, database IDs, timestamps, PIDs, process state,
prompts, agent events, and private persistence rows. It is canonical JSON and
bounded to 4096 bytes. SHA-256 with `eg_` public representation fingerprints
the bundle and result; the public bridge never exposes canonical bytes.

The only commands are `evidence_gate_capability` and
`evaluate_evidence_gate`. Results are ephemeral. No raw evidence injection,
rule selection, history, repository mutation, remediation, approval delivery,
or next-phase capability is exposed.

## Non-goals

Phase 9 does not execute or rerun agent commands, infer arbitrary command
evidence, connect to processes, alter Codex/Claude protocols, use LLM or
heuristic judgement, remediate, persist history, remove worktrees, or begin
Phase 10 reliability work. Automatic worktree removal remains disabled and
containment remains direct-child only.

## Public posture

Public DTOs are explicit allowlists: opaque fingerprints, gate metadata,
decision, factual reason, and limitation only. A passed gate establishes only
the stated baseline comparison; it does not establish that an agent's claimed
action succeeded.
