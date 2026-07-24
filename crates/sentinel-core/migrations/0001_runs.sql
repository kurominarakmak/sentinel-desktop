CREATE TABLE runs (
    id TEXT PRIMARY KEY NOT NULL,
    task_text TEXT NOT NULL CHECK (length(task_text) > 0),
    agent_kind TEXT NOT NULL CHECK (agent_kind IN ('fake', 'codex', 'claude_code')),
    status TEXT NOT NULL CHECK (status IN ('queued', 'preparing', 'running', 'cancelling', 'cancelled', 'completed', 'failed')),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    created_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    finished_at_ms INTEGER,
    exit_code INTEGER,
    error_category TEXT,
    error_message TEXT
);

CREATE TABLE run_events (
    run_id TEXT NOT NULL REFERENCES runs(id),
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    event_type TEXT NOT NULL CHECK (length(event_type) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    occurred_at_ms INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (run_id, sequence_number)
);

CREATE INDEX runs_recent_order ON runs(created_at_ms DESC, id DESC);
CREATE INDEX run_events_order ON run_events(run_id, sequence_number);
