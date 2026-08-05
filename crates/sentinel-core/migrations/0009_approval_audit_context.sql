-- Phase 7 extends the foundation request row without changing the original
-- migration. Runtime references are opaque public references, never PIDs,
-- paths, or adapter payloads.
ALTER TABLE approval_requests ADD COLUMN adapter_identity TEXT NOT NULL DEFAULT 'none'
    CHECK (adapter_identity IN ('none', 'codex', 'claude_code'));
ALTER TABLE approval_requests ADD COLUMN runtime_reference TEXT;
ALTER TABLE approval_requests ADD COLUMN resolution_reason TEXT;

CREATE INDEX approval_requests_exact_owner_pending
    ON approval_requests(project_id, worktree_id, task_key, adapter_identity,
        runtime_reference, state, created_at_ms, public_reference);

CREATE TABLE approval_decision_audit (
    id TEXT PRIMARY KEY NOT NULL,
    approval_reference TEXT NOT NULL REFERENCES approval_requests(public_reference),
    project_id TEXT NOT NULL REFERENCES projects(id),
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(id),
    task_key TEXT NOT NULL CHECK (length(task_key) BETWEEN 1 AND 128),
    decision TEXT NOT NULL CHECK (decision IN ('allowed_once','allowed_for_task','denied','hard_denied')),
    decision_source TEXT NOT NULL CHECK (decision_source IN ('user','policy','restart_reconciliation','timeout','runtime_terminal','cancellation')),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 256),
    profile TEXT NOT NULL CHECK (profile IN ('safe','balanced','autonomous','custom')),
    previous_state TEXT NOT NULL CHECK (previous_state = 'pending'),
    resulting_state TEXT NOT NULL CHECK (resulting_state IN ('allowed_once','allowed_for_task','denied','hard_denied')),
    resolved_at_ms INTEGER NOT NULL,
    UNIQUE(approval_reference)
);
CREATE INDEX approval_decision_audit_owner
    ON approval_decision_audit(project_id, worktree_id, task_key, approval_reference);
