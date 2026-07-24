# Contributing

## Prerequisites And Layout

Development requires Rust, Node.js, the selected package manager, Git, and macOS tooling for the first platform. Codex and Claude Code are optional for real-adapter work; never expect contributors to install them silently. Planned layout is `apps/desktop` for the React/Tauri app, `crates` for Rust core crates, `fixtures` for test repositories, `migrations`, `scripts`, and `.github/workflows`.

## Running And Testing

Exact workspace commands are not available until scaffolding exists. Use the documented package scripts to run the desktop app, Rust test command to run core tests, and frontend test command to run UI tests; contributors should update this document with exact reviewed commands when Phase 1 establishes them.

## Extension Work

Add an agent adapter by implementing the common contract, declaring capabilities, normalizing provider events, preserving session metadata, and passing adapter contract tests. Add a drift rule as a deterministic rule with factual findings, severity, score contribution, false-positive handling, and tests. Add evidence checks through project policy/configuration with baseline behavior, exit-code interpretation, persistence, and final-evaluator tests.

## Pull Requests And Security

Keep changes scoped, include tests appropriate to behavior, document policy or platform impact, and avoid changing locked architecture decisions without an explicit documented contradiction resolution. Do not place secrets in issues, tests, fixtures, or logs. Report vulnerabilities through [SECURITY.md](../SECURITY.md).
