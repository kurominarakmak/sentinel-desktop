# Evidence Model

## Verified Completion

An agent finishing its response is not verified completion. Evidence Gate independently evaluates the worktree and task record. Required evidence is configured commands, successful required exit codes, no unresolved approval or critical drift, an expected implementation diff, protected-path compliance, no orphan child process, and contract-specific checks. Optional evidence can yield warnings without turning a successful task into a failure.

## Commands And Baselines

Projects define deterministic evidence commands, such as `cargo test` and `cargo clippy -- -D warnings`. Baseline checks run before task work where configured, so later failures can be classified as regressions. Exit code zero is successful; nonzero is failed unless a configured command explicitly defines another interpretation. Command stdout/stderr is retained subject to redaction and output limits.

## Outcomes And Reruns

- **Completed:** every required check passes with no blocking finding.
- **Completed with warnings:** required checks pass; optional checks or non-blocking expectations warn.
- **Evidence incomplete:** required checks did not run or cannot establish the required result.
- **Failed:** a task, process, or required evaluation fails irrecoverably.

Evidence can be rerun after a fix or a transient failure. Each run is separately persisted and the final evaluator uses the current applicable run, preserving prior results for audit.

```toml
[evidence]
commands = ["cargo test", "cargo clippy -- -D warnings"]
baseline = ["cargo test"]
require_expected_diff = true
require_no_orphan_processes = true
```

See [task state machine](task-state-machine.md) and [drift rules](drift-rules.md).
