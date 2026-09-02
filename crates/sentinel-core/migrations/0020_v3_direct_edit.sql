-- Expand the durable workflow-mode constraint without changing existing task
-- records. SQLite cannot alter a CHECK constraint in place, so rebuild only
-- this parent table while foreign-key enforcement is temporarily disabled.
-- no-transaction
-- SQLite only honors foreign-key pragma changes outside a transaction. This
-- rebuild updates the workflow-mode CHECK while preserving every V3 child
-- table that references v3_tasks.
PRAGMA foreign_keys=OFF;
PRAGMA legacy_alter_table=ON;

CREATE TABLE v3_tasks_direct_edit (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT,
    workflow_id TEXT NOT NULL CHECK (length(workflow_id) BETWEEN 1 AND 128),
    workflow_mode TEXT NOT NULL DEFAULT 'manual' CHECK (workflow_mode IN ('manual','auto_integrate','direct_edit')),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 8000),
    lifecycle TEXT NOT NULL,
    recovery_condition TEXT NOT NULL,
    recovery_previous_lifecycle TEXT,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    terminal_at_ms INTEGER
);

INSERT INTO v3_tasks_direct_edit (id,project_id,workflow_id,workflow_mode,summary,lifecycle,recovery_condition,recovery_previous_lifecycle,version,created_at_ms,updated_at_ms,terminal_at_ms)
SELECT id,project_id,workflow_id,workflow_mode,summary,lifecycle,recovery_condition,recovery_previous_lifecycle,version,created_at_ms,updated_at_ms,terminal_at_ms FROM v3_tasks;

DROP TABLE v3_tasks;
ALTER TABLE v3_tasks_direct_edit RENAME TO v3_tasks;
CREATE INDEX v3_tasks_active_order ON v3_tasks(lifecycle, updated_at_ms DESC, id DESC);

PRAGMA foreign_keys=ON;
PRAGMA legacy_alter_table=OFF;
