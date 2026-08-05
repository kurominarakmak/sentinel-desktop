# Phase 4 Authoritative Execution Contract

## Authority and objective

[PLAN.md](../PLAN.md) is authoritative for Phase 4: implement Codex detection,
structured `codex exec --json`, persistence, cancellation, and an experimental
App Server boundary so a Codex task can produce an inspectable Phase 3 diff.
This contract decomposes that outcome without changing PLAN.md. Authority is
PLAN.md, then this document, then component contracts; `PROJECT_STATUS.md` is
descriptive only. Phase 5 (Claude Code) is not started.

## Milestones and dependencies

1. **Codex discovery and capability model.** Resolve only an explicitly
configured or PATH-discovered executable, run bounded `--version`, retain no
credentials, and expose a redacted availability/capability DTO.
2. **Private `exec --json` adapter.** Start only a verified managed worktree
with fixed arguments and no shell; parse bounded UTF-8 JSON Lines into the
existing normalized event model. Unknown, malformed, oversized, or secret-like
records fail closed without partial provider data.
3. **Runtime and persistence integration.** Persist normalized lifecycle and
provider-session metadata atomically, bind each live adapter to its RunId and
managed ProjectId/WorktreeId authority, and retain cancellation ownership until
the direct child is reaped. A task never executes in a primary repository.
4. **Experimental App Server capability boundary and desktop bridge.** Record
only an opt-in, versioned unavailable/available capability; do not start an
App Server or make it an authority. The public Tauri bridge returns redacted
run and capability information only. The existing Phase 3 textual-diff bridge
remains the only diff inspection route.
5. **Isolated fake-Codex integration.** A deterministic test executable
emulates documented JSON Lines and is run only in newly-created temporary
repositories/worktrees. It proves structured parsing, persistence,
cancellation, malformed-output failure, and Phase 3 isolation preservation.

## Trust, data, and resource boundaries

The frontend submits intent only; Rust resolves ProjectId/WorktreeId and the
authoritative path/inventory state. Provider stdout is untrusted transport, not
authorization. Public APIs never reveal paths, raw stdout/stderr, command
arguments, environment, PIDs, credentials, database internals, or private
evidence. Every child has null stdin, a fixed working directory, bounded
stdout/stderr/line sizes, a deadline, cancellation, kill/reap ownership, and
redacted fixed errors. No shell, lossy decoding, path normalization, automatic
worktree removal, merge, push, or provider installation is allowed.

Persistence writes only bounded normalized events and redacted session tokens;
an error, cancellation, parser failure, identity drift, deadline, or storage
failure produces no successful partial result. Cancellation is idempotent and
cannot affect another RunId or worktree.

The persisted context is deliberately separate from the legacy run event row:
it stores a fixed redacted task marker, never the submitted prompt. Its opaque
reference is backend-generated random syntax (`cr_` plus a UUID token), is
unique under SQLite constraints, and only authorizes lookup with exact
ProjectId, WorktreeId, and task key. Lifecycle updates use the persisted
version; the transition table is `created→starting→running→{succeeded,failed}`
or `created→cancelled`, `starting/running→cancelling→cancelled` (with failure
allowed during startup/cancellation). Terminal records carry their terminal
summary atomically and cannot transition again. Cancellation wins over a late
successful process event. At restart no OS process inspection or reconnection
is attempted: every nonterminal record becomes the redacted `interrupted`
terminal failure.

## Non-goals and Phase 5 boundary

This phase does not implement Claude Code, follow-up steering, approvals,
policy/drift/evidence evaluation, automatic cleanup, remote execution, or
production descendant containment changes. App Server is a capability record
only; interactive App Server workflow is deferred. No real Codex/Claude CLI,
production repository, or user worktree is used during implementation/tests.

## Completion and final validation

Completion requires every milestone above, direct Phase 3 regression coverage,
all configured Rust/frontend checks, adapter/persistence/cancellation/security
tests, fixture verification, `git diff --check`, a clean holistic P1/P2
review, one Phase 4 commit, a clean worktree, unchanged PLAN.md, unchanged
`phase-3-complete` tag at `630a54f`, and Phase 5 still not started.
