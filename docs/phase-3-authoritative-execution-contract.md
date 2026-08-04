# Phase 3 Authoritative Execution Contract

## Authority and boundary

This document is the user-authorized detailed execution contract for Phase 3.
It does not replace or modify [PLAN.md](../PLAN.md): PLAN.md remains
authoritative for the high-level Phase 3 objective and the Phase 3/Phase 4
boundary. This contract decomposes that objective only. `PROJECT_STATUS.md`
and status/threat summaries are descriptive, not authority sources. On a
conflict, the higher-ranked source wins and the implementation fails closed.

Phase 4 is excluded. Automatic managed-worktree removal remains disabled and
production descendant containment remains unchanged.

## Phase 3 dependency graph

1. **3C-B2-B0**: approve the byte grammar, fixtures, canonical expected
   evidence, and provenance gate.
2. **3C-B2-B1**: implement the private, bounded Git extraction and parser
   defined by the approved B2-B0 language.
3. **3C-B2-C**: implement private lifecycle/coherence orchestration.
4. **3C-B2-D**: complete private integration and adversarial validation.
5. **3C-C**: add the minimal redacted public bridge.
6. Complete final two-fake-task isolation acceptance and a holistic review.

No milestone may start until its predecessor is complete and clean. Approved
milestone contracts control technical details within their scope.

## Relationship to the PLAN objective

The final Phase 3 product must permit two deterministic fake tasks to run in
separate managed isolated worktrees without either executing in or mutating
the main repository directory. Inventory, classification, path, lifecycle,
and evidence observations remain authoritative and fail closed. Public access
is permitted only after the private B2 trust boundary is complete.

## Milestone scope and exit criteria

### 3C-B2-B0

B2-B0 contains only the exact staged/unstaged output envelope; byte grammar;
path equality and encoding; newline, marker, header, error, and resource
rules; conceptual parser states; empirical fixtures; manual adversarial
fixtures; canonical positive evidence; provenance; and binary-safe verifier
tooling. It contains no production parser, Git execution, persistence,
coherence, or public API.

It exits only when one deterministic language exists; every positive fixture
maps uniquely to B2-A evidence; every negative fixture maps to an exact private
error; empirical claims are reproducible in isolated repositories; manual
fixtures have deterministic construction provenance; hashes, lengths,
references, totals, and invariants verify; rejection self-tests pass; and a
P1/P2 review is clean.

### 3C-B2-B1

B2-B1 is private-only fixed-argv Git extraction and the strict bounded
byte-oriented parser from B2-B0. It includes redacted private errors,
cancellation/deadlines, complete consumption, canonical path comparison, and
fixture conformance. It excludes persistence, coherence, and public APIs. It
exits only with exact positive/negative conformance, enforced bounds, no
partial evidence, green focused/workspace validation, and a clean P1/P2 review.

### 3C-B2-C

B2-C privately accepts only the sequence
`A → C1 → E1 → B → C2 → E2 → C → C3 → E3`, followed by equality of A/B/C,
C1/C2/C3, and E1/E2/E3; refreshed managed rows; final repository/worktree
identity validation; final shared-deadline validation; and acceptance of E3.
It enforces lifecycle drift, transactional budgets, cancellation, and no
partial persistence, but exposes no public bridge. It exits only with
adversarial drift coverage, green validation, and a clean P1/P2 review.

### 3C-B2-D

B2-D privately integrates and adversarially validates staged/unstaged/no-text,
ineligible/mode/binary/conflict, identity/path/lifecycle/classification/
extraction drift, newline forms, canonical paths, malformed output, budgets,
timeout, cancellation, command failure, retention, deadline, redaction, and
no-partial-persistence behavior in isolated repositories. It exits only with
green focused/full validation and a clean P1/P2 review.

### 3C-C

Only after B2-D, 3C-C may add the minimal Tauri/TypeScript bridge. It uses
typed ProjectId/WorktreeId association and non-authoritative selection, but
never exposes raw internal paths, Git output/stderr, commands, environment,
PIDs, database values, or internal evidence. It exits only with Rust,
serialization/redaction, frontend, and full validation green plus a public
trust-boundary review.

### Final isolation acceptance

Use fake agents and newly isolated fixtures to prove two tasks use only their
own managed worktrees; neither changes the main repository; cross-task
ProjectId/WorktreeId/path use fails; and cancellation/failure of one cannot
mutate the other. No real Codex, Claude, production repository, or user
worktree may be used.

## Completion and validation

Each milestone requires focused checks, `cargo fmt --all --check`, workspace
tests, Clippy with warnings denied, desktop build, `git diff --check`, accurate
documentation, and a clean internal P1/P2 review before its focused commit.
Frontend checks additionally apply at 3C-C. Phase 3 completes only after all
above milestones, a clean final holistic review, a clean worktree, an
unmodified PLAN.md, and no Phase 4 behavior.
