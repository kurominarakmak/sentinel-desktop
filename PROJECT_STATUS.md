# Agent Sentinel Project Status

**Current program: Sentinel V3 planning complete; implementation not started.**

The repository contains working V1 foundations: a macOS Tauri 2 / React desktop shell with tray and global shortcut support; Rust crates for core state/storage, runtime/process supervision, agent APIs, Git worktrees, and fake-agent testing; SQLite migrations; and a substantial automated test suite. Existing uncommitted desktop changes were present before this documentation migration and are intentionally untouched.

## Scope status

V1 phase contracts and documentation are preserved as historical records. Their previous MVP/phase sequencing is superseded by the V3 scope documents below; V1 code remains the migration base and is not deprecated for removal by this change.

## V3 documentation navigation

- [V3 operational roadmap](PLAN.md)
- [V3 product scope](docs/v3-product-scope.md)
- [V3 target architecture](docs/v3-target-architecture.md)
- [V3 implementation plan](docs/v3-implementation-plan.md)
- [V3 workflow state machine](docs/v3-workflow-state-machine.md)
- [V3 OSS integration plan](docs/v3-oss-integration-plan.md)
- [V3 license policy](docs/v3-license-policy.md)

The prior [architecture](docs/architecture.md), [adapter contract](docs/agent-adapter-contract.md), and phase execution contracts describe the implemented V1 foundation. Where they conflict with V3 future scope, the V3 documents govern new work.

## Next action

Begin only V3 Phase 1: unify the durable task model and normalized events without changing active V1 user behavior. No real Codex/Claude integration, third-party source import, or UI redesign is authorized by the planning migration.
