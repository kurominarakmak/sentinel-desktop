CREATE TABLE claude_run_contexts (
    run_id TEXT PRIMARY KEY NOT NULL REFERENCES runs(id),
    public_reference TEXT NOT NULL UNIQUE CHECK (length(public_reference) BETWEEN 32 AND 128),
    project_id TEXT NOT NULL REFERENCES projects(id),
    worktree_id TEXT NOT NULL REFERENCES managed_worktrees(id),
    task_key TEXT NOT NULL CHECK (length(task_key) BETWEEN 1 AND 128),
    session_token TEXT CHECK (session_token IS NULL OR length(session_token) BETWEEN 1 AND 128),
    adapter TEXT NOT NULL CHECK (adapter = 'claude_stream_json'),
    protocol_version INTEGER NOT NULL CHECK (protocol_version = 1),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('created','starting','running','cancelling','cancelled','succeeded','failed')),
    cancellation_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancellation_requested IN (0, 1)),
    followup_sequence INTEGER NOT NULL DEFAULT 0 CHECK (followup_sequence >= 0),
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    progress_summary TEXT,
    terminal_summary TEXT,
    failure_category TEXT CHECK (failure_category IS NULL OR length(failure_category) <= 128),
    created_at_ms INTEGER NOT NULL,
    transitioned_at_ms INTEGER NOT NULL,
    terminal_at_ms INTEGER,
    CHECK ((terminal_at_ms IS NULL) = (lifecycle IN ('created','starting','running','cancelling')))
);
CREATE INDEX claude_run_contexts_owner_lookup ON claude_run_contexts(public_reference, project_id, worktree_id, task_key);
CREATE UNIQUE INDEX claude_run_contexts_one_active_owner ON claude_run_contexts(project_id, worktree_id, task_key) WHERE lifecycle IN ('created', 'starting', 'running', 'cancelling');
