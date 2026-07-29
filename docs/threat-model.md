# Threat Model

Agent Sentinel reduces risk through policy, worktrees, structured events, review, and evidence. It does not provide complete sandbox security against a compromised local machine or trusted executable.

| Threat | Impact | Mitigation | Residual risk |
| --- | --- | --- | --- |
| Prompt injection in repository content | Agent is manipulated into unsafe behavior | Treat content as untrusted; contracts, approvals, drift rules, and review | Agents may still follow harmful instructions within allowed scope |
| Malicious project scripts | Arbitrary code executes during checks | Approval profiles, known-command policy, visible commands | Allowed scripts run with user privileges |
| Destructive shell command | Data or system damage | Classification, approval, hard denials, no privilege escalation | Misclassification or user approval remains possible |
| Credential access | Secret disclosure or misuse | Ask for access, redact logs, do not store credentials | User environment and provider CLI may expose credentials |
| Dependency-installation attack | Malicious package or script | Approval for dependency/network changes; manifest drift finding | Approved installs retain ecosystem risk |
| Path traversal | Writes outside worktree | Canonical-path checks and worktree boundary policy | Filesystem races need careful implementation |
| Symlink attack | Boundary bypass | Resolve and validate paths before access; recheck on write | TOCTOU and platform semantics remain residual risk |
| Writes outside worktree | Repository or host modification | Explicit working directory, path policy, protected-path finding | Child tools can behave unexpectedly |
| Git history rewrite | Lost or altered history | Deny force push; no automatic merge/push | User-approved/manual Git actions are outside control |
| Untrusted agent output | False state or unsafe instruction | Normalize structured events; Rust state machine; never treat output as authorization | Provider event accuracy is external |
| Log secret leakage | Credential exposure | Redaction, output limits, retention controls | Unknown secret formats can evade redaction |
| Compromised CLI binary | Full user-level compromise | Explicit discovery/version diagnostics and user-managed installation | The app cannot establish binary provenance alone |
| Process escape/orphan | Continued or untracked work | Process groups, cancellation, checkpoints, restart detection | OS/platform process behavior can limit cleanup |
| Repository Git filter | Repository-controlled clean/process helper executes during inspection | AH2 uses fixed, bounded raw plumbing plus `hash-object --no-filters`, isolated global/system Git configuration, and marker tests proving clean/process helpers are not launched | Same-user filesystem races remain; direct-child kill/reap does not itself prove descendant containment |
| Trusted-runner descendant | A direct trusted child leaves a subprocess after timeout | AH1 prevents repository-selected helpers from launching; timeout/overflow kill and reap the direct child | macOS/aarch64 audit measured descendant survival after direct-child timeout; no process group or portable descendant containment is claimed |
