CREATE TABLE v3_validation_executions (
    validation_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_validation_results(id) ON DELETE RESTRICT,
    command_json TEXT NOT NULL,
    exit_code INTEGER,
    duration_ms INTEGER NOT NULL,
    stdout TEXT NOT NULL,
    stderr TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('passed','failed','timed_out','cancelled','interrupted','invalid')),
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER NOT NULL
);
