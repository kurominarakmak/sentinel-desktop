ALTER TABLE v3_tasks ADD COLUMN workflow_mode TEXT NOT NULL DEFAULT 'manual' CHECK (workflow_mode IN ('manual','auto_integrate'));

CREATE TABLE v3_task_integrations (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES v3_tasks(id) ON DELETE RESTRICT,
    review_generation_id TEXT NOT NULL REFERENCES v3_review_generations(id) ON DELETE RESTRICT,
    evidence_digest TEXT NOT NULL CHECK (length(evidence_digest) = 64),
    source_worktree_commit TEXT NOT NULL CHECK (length(source_worktree_commit) = 40),
    target_repository TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    target_head_before TEXT NOT NULL CHECK (length(target_head_before) = 40),
    resulting_target_commit TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('pending','integrated','failed')),
    failure_reason TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
