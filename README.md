# Agent Sentinel

Agent Sentinel is a local-first desktop supervisor for Codex and Claude Code tasks running in isolated Git worktrees.

> **Status: active development.** The project is not yet a usable release; unfinished functionality must not be assumed available.

## Planned Core Features

- Global task prompt and tray or menu-bar supervision
- Codex and Claude Code structured CLI adapters
- Git-worktree isolation and task history in SQLite
- Safe, Balanced, Autonomous, and Custom approval profiles
- Deterministic Drift Guardian and Evidence Gate

## Architecture

Tauri 2 hosts a React, TypeScript, and Vite frontend plus a Rust/Tokio core. SQLite stores application state; Git CLI worktrees isolate implementation tasks. See the [architecture](docs/architecture.md).

## Platforms And Agents

macOS is implemented and validated first; Windows and Linux are planned. Supported agents are Codex and Claude Code. Codex App Server is an experimental future integration where appropriate.

## Installation And Development

No installation package is available yet. Development setup and placeholder commands are documented in [contributing](docs/contributing.md).

## Documentation

- [Operational implementation roadmap](PLAN.md)
- [Current project status](PROJECT_STATUS.md)
- [Contributor implementation plan](docs/implementation-plan.md)
- [Product requirements](docs/product-requirements.md)
- [Security model](docs/security-model.md) and [threat model](docs/threat-model.md)
- [Agent adapter contract](docs/agent-adapter-contract.md)

## License

Apache-2.0 is the recommended license; the repository license file will define the final terms.
