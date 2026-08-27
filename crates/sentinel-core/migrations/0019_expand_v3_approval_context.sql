-- no-transaction
-- Final-action authorization stores a serialized, exact-worktree context.  The
-- original UI-summary bound (512 bytes) is too small for legitimate isolated
-- native profile paths, so preserve existing capabilities while widening only
-- this durable authorization column to the core API's 2 KiB contract.
PRAGMA foreign_keys = OFF;

CREATE TABLE v3_approvals_rebuilt (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    session_id TEXT,
    action_kind TEXT NOT NULL CHECK (length(action_kind) BETWEEN 1 AND 128),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 2048),
    lifecycle TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    decided_at_ms INTEGER
);

INSERT INTO v3_approvals_rebuilt (
    id, task_id, session_id, action_kind, summary, lifecycle, version,
    created_at_ms, decided_at_ms
)
SELECT
    id, task_id, session_id, action_kind, summary, lifecycle, version,
    created_at_ms, decided_at_ms
FROM v3_approvals;

DROP TABLE v3_approvals;
ALTER TABLE v3_approvals_rebuilt RENAME TO v3_approvals;
CREATE INDEX v3_approvals_pending ON v3_approvals(task_id, lifecycle, created_at_ms);

PRAGMA foreign_keys = ON;
