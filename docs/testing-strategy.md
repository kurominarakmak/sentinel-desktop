# Testing Strategy

## Test Layers

Rust unit tests cover state transitions, event normalization, command classification, path rules, drift scoring, evidence decisions, configuration precedence, and secret redaction. Frontend tests cover task presentation, approvals, drift/evidence states, keyboard flows, and bindings; use the repository's chosen frontend test runner (Vitest is planned).

Integration tests use temporary repositories for worktree creation, branch collisions, file changes, cancellation, approval blocking, test failures, restart recovery, and cleanup failures. Every adapter passes contract tests for detection, task start, session/command/file events, follow-up, cancellation, failure, and recovery metadata.

Phase 2A additionally uses temporary filesystem SQLite databases outside the repository. Its persistence integration suite opens an empty database and checks migrations, schema indexes, foreign-key enforcement, busy timeout, and WAL where supported. It round-trips runs/events and safe JSON, checks sequence ordering and duplicate rejection, verifies state-transition timestamps and invalid-transition rollback, and proves that transition-plus-event commits or rolls back as one transaction. Boundary and redaction cases confirm oversized data never reaches storage and likely credentials are not retained.

## Fixtures And End To End

Maintain Rust, Node/TypeScript, and Python fixture repositories with passing baselines, safe and failing tasks, protected files, dependency manifests, and deterministic evidence commands. The fake agent simulates normal completion, changes, test failure, approval, drift, crash, and cancellation.

Desktop end-to-end tests cover project registration, prompt launch, fake task, tray state, follow-up, approval, drift, evidence, final diff, restart, and retained history. Cross-platform tests validate platform-specific shortcuts, tray behavior, process cancellation, executable discovery, paths, and packaging as each platform phase begins.

## CI And Release

CI runs Rust and frontend tests, formatting, linting, adapter contracts, and relevant integration tests. Before release, required fixture/e2e flows, redaction, recovery, and platform packaging/signing checks must pass. Failure scenarios are first-class test cases, not only manual testing.
