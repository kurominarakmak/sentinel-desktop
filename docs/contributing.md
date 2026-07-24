# Contributing

## Prerequisites And Layout

Development requires Rust (including `rustfmt` and Clippy), Node.js 22 or later with npm, Git, and macOS tooling required by Tauri 2 for desktop development. Codex and Claude Code are optional for real-adapter work; normal development, tests, and CI never expect them to be installed or authenticated.

The workspace root contains reusable Rust crates in `crates/`, the Tauri/React desktop application in `apps/desktop`, Foundation scripts in `scripts/`, and GitHub Actions checks in `.github/workflows`. Reusable crates must not depend on Tauri; `scripts/verify-core-boundaries.sh` enforces that manifest boundary.

## Running And Testing

Install frontend dependencies from the committed lockfile, then run the complete Foundation suite from the repository root:

```sh
./scripts/validate-foundation.sh
```

The equivalent individual commands are:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
./scripts/verify-core-boundaries.sh

cd apps/desktop
npm ci
npm test
npm run build
```

To launch the desktop application locally on macOS:

```sh
cd apps/desktop
npm run tauri dev
```

To validate the macOS Tauri build without signing, notarization, or distribution packaging:

```sh
cd apps/desktop
npm run tauri build -- --no-bundle
```

GitHub Actions runs only the non-interactive Rust and frontend checks on Linux. The macOS Tauri build and tray/shortcut/focus behavior remain manual checks because they require a macOS desktop environment; they are not claimed as CI coverage.

## Extension Work

Add an agent adapter by implementing the common contract, declaring capabilities, normalizing provider events, preserving session metadata, and passing adapter contract tests. Add a drift rule as a deterministic rule with factual findings, severity, score contribution, false-positive handling, and tests. Add evidence checks through project policy/configuration with baseline behavior, exit-code interpretation, persistence, and final-evaluator tests.

## Pull Requests And Security

Keep changes scoped, include tests appropriate to behavior, document policy or platform impact, and avoid changing locked architecture decisions without an explicit documented contradiction resolution. Do not place secrets in issues, tests, fixtures, or logs. Report vulnerabilities through [SECURITY.md](../SECURITY.md).
