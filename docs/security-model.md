# Security Model

## Trust Boundaries And Principles

The user, registered repository, agent CLI, operating system, Git CLI, and network are distinct trust boundaries. Repository content and agent output are untrusted inputs, not authorization. Apply least privilege: run as the user, never administrator/root; restrict task execution to its worktree; ask before sensitive actions; retain human review of the resulting branch and diff.

## Isolation And Execution

Every implementation task uses a task branch and registered Git worktree. The primary working tree is never modified directly by default. Processes launch through controlled boundaries with explicit working directories, policy evaluation, child-process cleanup, and persisted metadata. Do not claim complete sandbox security: provider controls supplement but do not replace operating-system boundaries and user judgment.

## Secrets, Logs, Files, And Network

Agent Sentinel does not store Codex or Claude passwords and uses their CLI-owned authentication. It must not log API keys, bearer tokens, passwords, cookies, SSH keys, or secret environment variables, nor read `.env` contents for telemetry. Redact logs, cap output, and restrict file access and writes outside registered worktrees through policy. Phase 2B captures at most 4 KiB of redacted process stderr in memory; the reusable SQLite repository independently re-redacts and caps each persisted safe-error category/message at 512 UTF-8 bytes. Network access is policy-controlled; dependency installation and external network use require the applicable approval.

## Prohibited Automation And Assumptions

The app never silently installs or modifies AI CLIs, enables unrestricted permissions, auto-merges, pushes, force-pushes, deletes unreviewed worktrees, treats agent output as authorization, or trusts an agent completion claim. It assumes the user controls the local machine, registered project, and installed binaries; a compromised user account, repository, CLI binary, or OS can exceed product-level controls. See [threat model](threat-model.md) and [autonomy policy](autonomy-and-approvals.md).
