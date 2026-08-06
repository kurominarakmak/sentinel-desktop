# Sentinel V3 Workflow State Machine

**Workflow source of truth for V3.** This document defines V3 task states, transition guards, and final-action boundaries for the product in [V3 product scope](v3-product-scope.md) and the architecture in [V3 target architecture](v3-target-architecture.md). The V1 state-machine and phase documents remain historical records.

## Scope boundary

This is the V3 target state machine. It does not revise the existing V1 `Run` lifecycle or claim that V1's bounded Codex/Claude paths are active production sessions. V3 will migrate through compatible durable projections, preserving V1 task history and keeping the UI non-authoritative. The current tray, global hotkey, floating prompt, and status/settings windows remain presentation surfaces; they request actions and render state but never transition it.

## Task states

```mermaid
stateDiagram-v2
  [*] --> Draft
  Draft --> Preparing: task confirmed
  Preparing --> Implementing: worktree transaction committed
  Preparing --> Blocked: setup/approval failure
  Implementing --> AwaitingApproval: provider requests approval
  AwaitingApproval --> Implementing: approved
  AwaitingApproval --> Blocked: denied/expired
  Implementing --> Validating: implementer turn complete
  Validating --> Reviewing: required checks pass
  Validating --> Repairing: confirmed repairable deterministic failure
  Reviewing --> Repairing: confirmed blocking finding and rounds remain
  Repairing --> Validating: repair turn complete
  Reviewing --> ReadyForHuman: no confirmed blockers
  Validating --> ReadyForHuman: no review configured
  ReadyForHuman --> Finalized: approved final action or retain branch
  ReadyForHuman --> DiscardPending: discard/rollback requested
  DiscardPending --> Finalized: confirmed
  Implementing --> Cancelled
  Preparing --> Failed
  Implementing --> Failed
  Validating --> Blocked
  Reviewing --> Blocked
  Repairing --> Blocked: repair limit reached
  [*] <-- Finalized
  [*] <-- Cancelled
  [*] <-- Failed
  [*] <-- Blocked
```

`Recovering` is a persisted substate entered at application restart for an active task. It may return to its prior active state only after the adapter proves supported session recovery and ownership; otherwise it becomes `Blocked` (safe action required) or `Cancelled`/`Failed` with retained artifacts. Terminal state is not inferred from process disappearance.

## State invariants

- `Preparing` creates the branch/worktree transaction before an implementation adapter is allowed to start.
- `Implementing` and `Repairing` use the same assigned worktree; a repair never creates a second hidden task/worktree.
- `Validating` executes only declared command profiles in the worktree. Required checks pass before review can clear completion.
- `Reviewing` is read-only. Findings have stable ID, severity, evidence location, confidence, disposition, and round.
- A repair starts only for a Sentinel-confirmed blocker, only after any necessary approval, and only while `repair_round < workflow.max_repair_rounds`.
- `ReadyForHuman` is the only state that can request commit, merge, push, discard, rollback, or cleanup. Approval is bound to task, worktree, base/diff identity, requested action, and record version.
- `Finalized` means the approved outcome and artifact retention are durably recorded; it does not imply merge or push unless those separately approved actions completed.

## Repair and finding disposition

Deterministic failures are blocking by profile configuration. Another model may provide review only through a read-only reviewer session. Its findings are initially `reported`; Sentinel validates shape/evidence, then a user or configured deterministic rule marks each `confirmed_blocking`, `non_blocking`, `duplicate`, or `rejected`. Only `confirmed_blocking` findings are sent to the implementer. If the cap is reached, Sentinel stops and exposes the diff, attempts, check artifacts, and findings for human decision; it never retries indefinitely.

## Failure, cancellation, and rollback

Invalid transitions, missing ownership, stale approval versions, uncertain destructive effects, missing required evidence, malformed provider events, and unrecoverable persistence errors fail closed. Cancellation stops only recorded direct children, retains the worktree, and records cleanup status. Rollback/discard is a separate user-approved Git transaction and failure leaves the worktree intact for inspection. A rollback never rewrites the user's main working tree.
