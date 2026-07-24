# Data Model

SQLite, accessed through SQLx migrations, stores application state while TOML stores user and project policy. Do not expand this model beyond current task supervision needs.

## Phase 2A: Run Persistence Slice

Phase 2A implements only the persistence foundation for the fake-agent slice. `sentinel-core` is Tauri-independent and defines a UUID-backed `RunId`, `TaskRequest`, `Run`, `RunStatus`, `SafeRunError`, and `NormalizedAgentEvent`. A newly created run uses the `fake` agent kind, schema version 1, and `queued` status. Future project, repository, worktree, approval, drift, and evidence entities remain out of this migration.

The initial SQLite migration contains:

| Table | Purpose and constraints |
| --- | --- |
| `runs` | UUID text primary key; bounded application input; checked agent/status values; schema version and millisecond timestamps. `runs_recent_order` supports `created_at_ms DESC, id DESC` listing. |
| `run_events` | Event payload JSON tied to a run by a foreign key; `(run_id, sequence_number)` primary key prevents duplicate sequences. `run_events_order` supports ordered replay. |

The repository enables foreign keys, configures a five-second busy timeout, requests WAL for filesystem databases, and applies migrations at open. SQLite may validly report `memory` journal mode for in-memory databases; configuration tests account for that difference.

Run transitions are validated before they are stored:

| From | Allowed next states |
| --- | --- |
| `queued` | `preparing`, `cancelled` |
| `preparing` | `running`, `failed`, `cancelled` |
| `running` | `cancelling`, `completed`, `failed` |
| `cancelling` | `cancelled`, `failed` |
| terminal (`cancelled`, `completed`, `failed`) | none |

Entering `running` sets `started_at_ms` once. Entering any terminal state sets `finished_at_ms`. `transition_with_event` validates the transition, writes the status/timestamps and event in one SQL transaction, and commits only if both writes succeed. Duplicate events or any write failure roll back the status update too.

Task text is limited to 8,000 UTF-8 bytes and serialized event payloads to 16,000 bytes; over-limit inputs are rejected before a database write. Storage failures use the opaque `CoreError::Storage` rather than returning raw SQLite diagnostics. `SafeRunError` redacts likely bearer tokens, `sk-` keys, API keys, passwords, and secrets while preserving ordinary diagnostic text where possible.

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
