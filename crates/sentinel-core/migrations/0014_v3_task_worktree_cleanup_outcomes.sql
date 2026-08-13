CREATE TABLE v3_task_worktree_cleanup_outcomes (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_task_worktrees(task_id) ON DELETE RESTRICT,
    state TEXT NOT NULL CHECK (state IN ('discarded', 'retained')),
    reason TEXT,
    approval_id TEXT REFERENCES v3_approvals(id) ON DELETE RESTRICT,
    completed_at_ms INTEGER NOT NULL
);
