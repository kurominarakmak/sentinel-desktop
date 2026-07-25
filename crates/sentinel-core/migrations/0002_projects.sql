CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL CHECK (length(display_name) > 0),
    repository_identity TEXT NOT NULL UNIQUE,
    repository_root TEXT NOT NULL,
    primary_root TEXT NOT NULL,
    git_common_dir TEXT NOT NULL,
    branch TEXT,
    head TEXT,
    validation_state TEXT NOT NULL CHECK (validation_state IN ('valid', 'detached', 'unborn', 'linked_worktree')),
    is_primary_worktree INTEGER NOT NULL CHECK (is_primary_worktree IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_validated_at_ms INTEGER NOT NULL
);
CREATE INDEX projects_recent_order ON projects(created_at_ms DESC, id DESC);
