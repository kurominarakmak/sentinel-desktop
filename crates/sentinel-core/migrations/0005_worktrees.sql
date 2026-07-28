CREATE TABLE managed_worktrees (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
    worktree_path TEXT NOT NULL UNIQUE,
    base_commit TEXT NOT NULL CHECK (length(base_commit) = 40),
    repository_identity TEXT NOT NULL,
    repository_fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('creating', 'ready', 'removing', 'removed', 'failed', 'missing', 'identity_changed', 'retained_dirty')),
    error_category TEXT,
    created_at_ms INTEGER NOT NULL,
    ready_at_ms INTEGER,
    removed_at_ms INTEGER,
    last_validated_at_ms INTEGER NOT NULL
);
CREATE INDEX managed_worktrees_project_order ON managed_worktrees(project_id, created_at_ms DESC, id DESC);
