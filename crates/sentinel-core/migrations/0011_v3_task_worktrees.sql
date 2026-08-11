CREATE TABLE v3_task_worktrees (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    repository_root TEXT NOT NULL,
    worktree_path TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL UNIQUE,
    base_commit TEXT NOT NULL CHECK (length(base_commit) = 40),
    state TEXT NOT NULL CHECK (state = 'ready'),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
