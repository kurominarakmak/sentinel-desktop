CREATE TABLE v3_task_worktrees_reconciled (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    repository_root TEXT NOT NULL,
    worktree_path TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL UNIQUE,
    base_commit TEXT NOT NULL CHECK (length(base_commit) = 40),
    state TEXT NOT NULL CHECK (state IN ('ready', 'recovery_required')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

INSERT INTO v3_task_worktrees_reconciled
    (task_id, repository_root, worktree_path, branch, base_commit, state, created_at_ms, updated_at_ms)
SELECT task_id, repository_root, worktree_path, branch, base_commit, state, created_at_ms, updated_at_ms
FROM v3_task_worktrees;

DROP TABLE v3_task_worktrees;
ALTER TABLE v3_task_worktrees_reconciled RENAME TO v3_task_worktrees;
