# Testing Strategy

## Test Layers

Rust unit tests cover state transitions, event normalization, command classification, path rules, drift scoring, evidence decisions, configuration precedence, and secret redaction. Frontend tests cover task presentation, approvals, drift/evidence states, keyboard flows, and bindings; use the repository's chosen frontend test runner (Vitest is planned).

Integration tests use temporary repositories for worktree creation, branch collisions, file changes, cancellation, approval blocking, test failures, restart recovery, and cleanup failures. Every adapter passes contract tests for detection, task start, session/command/file events, follow-up, cancellation, failure, and recovery metadata.

Phase 2A additionally uses temporary filesystem SQLite databases outside the repository. Its persistence integration suite opens an empty database and checks migrations, schema indexes, foreign-key enforcement, busy timeout, and WAL where supported. It round-trips runs/events and safe JSON, checks sequence ordering and duplicate rejection, verifies state-transition timestamps and invalid-transition rollback, and proves that transition-plus-event commits or rolls back as one transaction. Boundary and redaction cases confirm oversized data never reaches storage and likely credentials are not retained.

Phase 2B runtime integration tests run only the local deterministic fake-agent binary. They use temporary SQLite files and Tokio to verify process-supervised success, failure, partial and malformed output, cancellation races and process groups, parent-exit descendant cleanup with retained stdout/stderr pipes, live-bus lag/disconnect behavior, ordered event persistence, persistence failure, and bounded redacted stderr. No test invokes Codex, Claude Code, or the manual `sentinel-probe` binary.

## Fixtures And End To End

Maintain Rust, Node/TypeScript, and Python fixture repositories with passing baselines, safe and failing tasks, protected files, dependency manifests, and deterministic evidence commands. The fake agent simulates normal completion, changes, test failure, approval, drift, crash, and cancellation.

Desktop end-to-end tests cover project registration, prompt launch, fake task, tray state, follow-up, approval, drift, evidence, final diff, restart, and retained history. Cross-platform tests validate platform-specific shortcuts, tray behavior, process cancellation, executable discovery, paths, and packaging as each platform phase begins.

Phase 2C2 frontend integration coverage uses deterministic deferred promises to exercise persisted/live event ordering, selection and lifecycle races, listener registration/unmount cleanup, submission pending and failure isolation, every cancellation-result DTO, RunId-scoped cancellation operations, and runtime-environment failure/retry behavior. The completed Phase 2C2 automated gate recorded 92 passing frontend tests, 76 passing Rust workspace tests, passing Vite production and Tauri no-bundle builds, passing `cargo fmt --all --check`, and passing workspace Clippy with warnings denied; final review found no remaining P1/P2 issues. Phase 2C3 then completed real macOS desktop acceptance for success, failure, cancellation, replay/burst, malformed protocol, restart persistence, detached-run capability, and focus/window behavior.

## CI And Release

CI runs Rust and frontend tests, formatting, linting, adapter contracts, and relevant integration tests. Before release, required fixture/e2e flows, redaction, recovery, and platform packaging/signing checks must pass. Failure scenarios are first-class test cases, not only manual testing.
