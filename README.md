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

macOS is implemented and validated first; Windows and Linux are planned. Codex
structured execution is bounded to backend-owned managed worktrees and uses
opaque public run references. Codex App Server is explicitly unavailable and
experimental; it is never launched by the desktop bridge. Claude stream-json
records use the same bounded, ownership-scoped persisted interface; follow-up
and resume inputs are fixed-form capabilities and Claude is truthfully reported
unavailable until a private configured runner exists. The Phase 6 desktop
prompt now exposes safe project/worktree and agent
selection, history/detail, tray/shortcut access, and truthful unavailable
notification/autostart status. Phase 7 supplies an approval control plane:
bounded policy decisions and audit records are persisted and reviewable, but
are not delivered to a live Codex or Claude process. Phase 8 adds a read-only
deterministic Drift Guardian; findings do not change repositories or runtimes.

## Installation And Development

Prerequisites are a current Rust toolchain (including `rustfmt` and Clippy), Node.js 22 or later with npm, and Git. macOS desktop development additionally requires the system tooling required by Tauri 2. Agent CLIs are optional; normal development and CI never make a real Codex or Claude request.

```sh
# Run the complete non-interactive Foundation validation suite.
./scripts/validate-foundation.sh

# Launch the desktop application during development (macOS-first).
cd apps/desktop
npm ci
npm run tauri dev
```

The desktop bundle build is a macOS manual validation step and intentionally is not performed in Linux CI. See [contributing](docs/contributing.md) for the complete command list and limitations.

## Documentation

- [Operational implementation roadmap](PLAN.md)
- [Current project status](PROJECT_STATUS.md)
- [Contributor implementation plan](docs/implementation-plan.md)
- [Product requirements](docs/product-requirements.md)
- [Security model](docs/security-model.md) and [threat model](docs/threat-model.md)
- [Agent adapter contract](docs/agent-adapter-contract.md)

## License

Apache-2.0 is the recommended license; the repository license file will define the final terms.
