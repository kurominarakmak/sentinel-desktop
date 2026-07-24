# Autonomy And Approvals

## Profiles

**Safe** automatically allows reads inside the worktree, repository search, and `git status`/`git diff`; it asks for writes, shell commands, network access, dependency changes, and access outside the worktree.

**Balanced** automatically allows reads/writes in the worktree, known test/lint/build commands, and non-destructive Git inspection. It asks for dependency changes, network access outside configured domains, destructive commands, database mutation, access outside the worktree, and Git push.

**Autonomous** automatically allows normal repository operations in the worktree, but always asks for credential access, writes outside registered worktrees, force push, destructive system operations, privilege escalation, and Agent Sentinel policy-file changes.

**Custom** uses explicit TOML rules. Hard safety boundaries remain active in all profiles.

## Decisions And Audit

Always denied actions include automatic merge, push, force-push, silent AI CLI installation, unrestricted permissions, and actions prohibited by policy. An approval request shows the proposed action and policy reason. The user can deny, allow once, or allow for the current task; each decision is persisted with task, rule, action, time, and decision. Allow-for-task never becomes a global allow.

Configuration precedence is task-specific explicit policy, project TOML policy, user TOML policy, then built-in profile defaults, with hard-deny rules taking precedence at every level.

```toml
[policy]
profile = "custom"

[[policy.command]]
pattern = ["cargo", "test", "*"]
decision = "allow"

[[policy.command]]
pattern = ["cargo", "add", "*"]
decision = "ask"

[[policy.command]]
pattern = ["git", "push", "--force", "*"]
decision = "deny"
```

Provider sandboxes, approval settings, permission rules, and hooks are a first enforcement boundary; Agent Sentinel does not rely solely on after-the-fact monitoring. See [security model](security-model.md).
