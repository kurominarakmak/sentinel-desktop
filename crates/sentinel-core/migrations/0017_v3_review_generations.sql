CREATE TABLE v3_review_generations (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    digest TEXT NOT NULL CHECK (length(digest) = 71),
    base_commit TEXT NOT NULL CHECK (length(base_commit) = 40),
    worktree_revision TEXT NOT NULL CHECK (length(worktree_revision) = 40),
    diff_digest TEXT NOT NULL CHECK (length(diff_digest) = 71),
    validation_evidence_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(task_id, digest)
);
CREATE INDEX v3_review_generations_task_order ON v3_review_generations(task_id, created_at_ms, id);

ALTER TABLE v3_review_findings ADD COLUMN review_generation_id TEXT REFERENCES v3_review_generations(id) ON DELETE RESTRICT;
CREATE INDEX v3_review_findings_generation_order ON v3_review_findings(review_generation_id, created_at_ms, id);
