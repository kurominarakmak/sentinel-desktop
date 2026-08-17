CREATE TABLE v3_supervisor_requests (
    request_id TEXT PRIMARY KEY NOT NULL,
    request_kind TEXT NOT NULL,
    intent_fingerprint TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('pending', 'completed')),
    response_json TEXT,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER
);
