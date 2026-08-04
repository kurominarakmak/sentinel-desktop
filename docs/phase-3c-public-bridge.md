# Phase 3C-C Public Textual-Diff Bridge

`inspect_textual_diff` is the sole Phase 3C-C textual-diff command. Its input
contains typed `ProjectId` and `WorktreeId` strings plus a non-authoritative
repository-relative path candidate. The persisted worktree row must belong to
the supplied project; otherwise the request rejects before Git access.

The bridge invokes the locked B1 inventory/classification coherence sequence,
then uses only the selected exact inventory path for the bounded staged and
unstaged B2 patch envelopes. The parser completely consumes both raw streams
before returning a result. Unicode names whose NFC and NFD spellings differ
are rejected both at selection and inventory parsing, preventing platform
normalization aliasing.

The public result contains only `project_id`, `worktree_id`, a textual
availability flag, staged/unstaged addition and deletion totals, and total
hunk count. It exposes no path, line text, raw patch bytes, Git stderr,
command, environment, process identifier, database value, or private
eligibility/evidence state. Errors are fixed redacted categories.

This bridge is read-only. It does not start a provider agent, write a
repository, persist extraction results, remove a worktree, or add Phase 4
behavior. Managed-worktree automatic removal remains disabled.
