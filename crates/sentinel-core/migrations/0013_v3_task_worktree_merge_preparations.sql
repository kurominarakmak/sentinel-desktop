CREATE TABLE v3_task_worktree_merge_preparations (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_task_worktrees(task_id) ON DELETE RESTRICT,
    target_branch TEXT NOT NULL,
    target_commit TEXT NOT NULL CHECK (length(target_commit) = 40),
    worktree_commit TEXT NOT NULL CHECK (length(worktree_commit) = 40),
    target_advanced INTEGER NOT NULL CHECK (target_advanced IN (0, 1)),
    merge_ready INTEGER NOT NULL CHECK (merge_ready IN (0, 1)),
    conflicts_json TEXT NOT NULL,
    diff_json TEXT NOT NULL,
    prepared_at_ms INTEGER NOT NULL
);
