# Data Model

SQLite, accessed through SQLx migrations, stores application state while TOML stores user and project policy. Do not expand this model beyond current task supervision needs.

| Entity | Main fields |
| --- | --- |
| Project | id, name, repository path, base branch, default agent, autonomy profile, protected paths, evidence config |
| Task | id, project id, agent, original prompt, intent contract, status, phase, branch, worktree path, risk score, timestamps |
| Agent session | id, task id, provider session ID, adapter kind/capabilities, process metadata, timestamps |
| Task event | id, task/session id, normalized type, payload, sequence, timestamp |
| Command run | id, task id, command, working directory, lifecycle, exit code, redacted output summary |
| File change | id, task id, path, change kind, observed time |
| Approval request | id, task id, action, policy reason, decision, scope, timestamps |
| Drift finding | id, task id, rule, severity, score contribution, facts, resolution |
| Evidence result | id, task id, command/check, baseline relation, status, exit code, output summary, run time |
| Checkpoint | id, task id, recovery state, recorded session/process/worktree metadata, time |
| Application setting | key, scope, serialized value, updated time |

Projects have many tasks; tasks have many sessions, events, command runs, file changes, approval requests, findings, evidence results, and checkpoints. Index task project/status/created time; session provider ID; event task/sequence; command task/time; file change task/path; approval task/status; finding task/severity; evidence task/run time. Retain task history for recovery and audit; redact secrets, cap stored output, rotate logs, and retain worktrees with unreviewed changes.
