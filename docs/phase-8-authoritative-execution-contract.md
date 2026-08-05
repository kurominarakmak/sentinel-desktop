# Phase 8 Authoritative Execution Contract

## Objective

Phase 8 implements the PLAN.md Drift Guardian outcome: deterministic MVP rules
whose findings are reproducible and factually explained. It is read-only.

## Fixed catalog

Catalog version 1 has two non-overlapping rules over authoritative persisted
managed-worktree state. `DG001` is a warning when a worktree is
`retained_dirty`; its invariant is `ready`. `DG002` is an error when the state
is any other unavailable state; its invariant is `ready or retained_dirty`.
Both use the managed project/worktree association, strong project fingerprint,
base commit, repository fingerprint, state, and bounded error category. No
rule inspects user paths, raw Git output, prompts, agent output, or intent.

## Determinism and evidence

The engine builds schema-version-1 canonical JSON with catalog version 1,
excluding paths, database IDs, timestamps, temporary roots, and row order. A
SHA-256 snapshot fingerprint identifies this canonical byte sequence. Findings
are sorted by `(rule_id, fingerprint)` and each SHA-256 fingerprint includes
rule/version, expected fact, observed fact, and snapshot fingerprint. The
template states expected fact, observed fact, and exact rule comparison; it
does not infer cause or remediation.

## Authority and boundary

The request accepts only typed ProjectId and WorktreeId. Backend reads the
managed rows and rejects ownership mismatch or non-strong project authority.
Results are all-or-nothing and bounded to eight findings and 4096 snapshot
bytes. The public bridge uses explicit allowlisted DTOs only.

Phase 8 has no persistence history, arbitrary rules, LLM analysis, network
access, remediation, filesystem scanning, automatic worktree removal, runtime
approval delivery, process waiter, or Phase 9 Evidence Gate behavior.

## Completion

Temporary isolated fixtures prove repeatable canonical bytes, fingerprints,
ordering, positive/negative rule behavior, and ownership failure. Existing
Phase 3–7 constraints remain in force. Phase 9 is not started.
