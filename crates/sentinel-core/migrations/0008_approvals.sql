CREATE TABLE approval_requests (
    id TEXT PRIMARY KEY NOT NULL,
    public_reference TEXT NOT NULL UNIQUE CHECK (length(public_reference) BETWEEN 32 AND 128),
    project_id TEXT NOT NULL REFERENCES projects(id),
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(id),
    task_key TEXT NOT NULL CHECK (length(task_key) BETWEEN 1 AND 128),
    profile TEXT NOT NULL CHECK (profile IN ('safe','balanced','autonomous','custom')),
    action_category TEXT NOT NULL CHECK (length(action_category) BETWEEN 1 AND 64),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 512),
    state TEXT NOT NULL CHECK (state IN ('pending','allowed_once','allowed_for_task','denied','hard_denied')),
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    decided_at_ms INTEGER
);
CREATE INDEX approval_requests_owner_lookup ON approval_requests(public_reference, project_id, worktree_id, task_key);
CREATE INDEX approval_requests_pending ON approval_requests(project_id, state, created_at_ms);
