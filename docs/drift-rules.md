# Drift Rules

## Purpose And Design

Drift Guardian detects deterministic scope or behavior violations during a task. Findings are factual, reproducible, and explain their contributing facts. Its risk score is not an AI confidence score.

## MVP Rules

| Rule | Trigger | Default severity |
| --- | --- | --- |
| Protected path | `.env`, credential files, configured protected paths, or paths outside the worktree change | Critical |
| Scope violation | A changed file is outside the intent contract's allowed scope | High |
| File explosion | `changed_files > contract.max_files_changed` | Medium or High |
| Dependency change | Unauthorized manifest or lockfile change (`package.json`, `Cargo.toml`, `requirements.txt`, `pyproject.toml`, etc.) | High |
| Repeated failure | Three equivalent command failures by default | Medium |
| Test regression | A baseline-passing test fails after task work | High |
| Completion without evidence | Agent claims completion before required checks pass | High |

## Risk And Handling

Scores are the sum of fixed, documented rule contributions, clamped to 0-100; the implementation must record every contribution rather than infer a score from agent output. The approved baseline defines bands, but not individual weights: define and test those fixed weights when the rule engine is implemented. The bands are 0-19 Low, 20-49 Medium, 50-79 High, and 80-100 Critical. The UI presents each finding, rule, affected path or command, severity, and score contribution. Users can approve an allowed exception, adjust an incorrect contract/policy, rerun checks, cancel, or inspect the diff. Critical unresolved findings block completion.

False positives are handled by correcting project policy or the task contract and recording any task-specific approval; rules themselves remain deterministic. Examples: changing `Cargo.toml` for an approved dependency produces a visible dependency finding and approval; changing `.env` is critical; four repeated `cargo test` failures produce a repeated-failure finding.
