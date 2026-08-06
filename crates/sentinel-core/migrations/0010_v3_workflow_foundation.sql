-- V3 records are deliberately separate from V1 runs and adapter contexts.
-- This additive migration preserves V1 history and lets the V3 supervisor
-- adopt its own durable aggregate without changing legacy behaviour.
CREATE TABLE v3_tasks (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT,
    workflow_id TEXT NOT NULL CHECK (length(workflow_id) BETWEEN 1 AND 128),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 8000),
    lifecycle TEXT NOT NULL,
    recovery_condition TEXT NOT NULL,
    recovery_previous_lifecycle TEXT,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    terminal_at_ms INTEGER
);
CREATE INDEX v3_tasks_active_order ON v3_tasks(lifecycle, updated_at_ms DESC, id DESC);

CREATE TABLE v3_task_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    session_id TEXT,
    provider TEXT NOT NULL CHECK (length(provider) BETWEEN 1 AND 128),
    event_kind TEXT NOT NULL CHECK (length(event_kind) BETWEEN 1 AND 128),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    occurred_at_ms INTEGER NOT NULL,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    causation_id TEXT,
    correlation_id TEXT,
    payload_json TEXT NOT NULL,
    raw_diagnostic_json TEXT,
    UNIQUE(task_id, sequence_number)
);
CREATE INDEX v3_task_events_order ON v3_task_events(task_id, sequence_number);

CREATE TABLE v3_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    provider TEXT NOT NULL CHECK (length(provider) BETWEEN 1 AND 128),
    provider_session_ref TEXT NOT NULL CHECK (length(provider_session_ref) BETWEEN 1 AND 512),
    lifecycle TEXT NOT NULL,
    recovery_condition TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    terminal_at_ms INTEGER,
    UNIQUE(task_id, provider, provider_session_ref)
);
CREATE INDEX v3_sessions_task_order ON v3_sessions(task_id, created_at_ms, id);

CREATE TABLE v3_approvals (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    session_id TEXT,
    action_kind TEXT NOT NULL CHECK (length(action_kind) BETWEEN 1 AND 128),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 512),
    lifecycle TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    decided_at_ms INTEGER
);
CREATE INDEX v3_approvals_pending ON v3_approvals(task_id, lifecycle, created_at_ms);

CREATE TABLE v3_validation_results (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    profile_id TEXT NOT NULL CHECK (length(profile_id) BETWEEN 1 AND 128),
    check_name TEXT NOT NULL CHECK (length(check_name) BETWEEN 1 AND 128),
    required INTEGER NOT NULL CHECK (required IN (0, 1)),
    lifecycle TEXT NOT NULL,
    summary TEXT,
    artifact_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX v3_validation_results_task_order ON v3_validation_results(task_id, created_at_ms, id);

CREATE TABLE v3_repair_rounds (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    round_number INTEGER NOT NULL CHECK (round_number >= 1),
    lifecycle TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(task_id, round_number)
);

CREATE TABLE v3_review_findings (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    repair_round_id TEXT REFERENCES v3_repair_rounds(id) ON DELETE RESTRICT,
    severity TEXT NOT NULL,
    disposition TEXT NOT NULL,
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 2048),
    evidence_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX v3_review_findings_task_order ON v3_review_findings(task_id, created_at_ms, id);

CREATE TABLE v3_artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 128),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 512),
    content_hash TEXT,
    metadata_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX v3_artifacts_task_order ON v3_artifacts(task_id, created_at_ms, id);
