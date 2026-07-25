# Phase 3A — Trusted Project Registry and Git Validation

Phase 3 in [PLAN.md](../PLAN.md) is Git isolation: a project registry, a worktree manager, and diffs. Phase 3A completes only the registry and read-only validation foundation. It does not create or remove a worktree, mutate a repository, generate a diff, run an agent, or expose a product control for Codex or Claude.

## Project model

Each local registration has an Agent Sentinel-generated `ProjectId`, a bounded display name, and validation metadata. The local SQLite record stores canonical backend paths only where later backend isolation needs them:

- canonical repository root;
- canonical primary working-tree root;
- canonical Git common directory;
- a local identity derived from the canonical common directory;
- branch or detached/unborn HEAD metadata; and
- registration and validation timestamps.

The frontend DTO deliberately omits all of those paths and the identity. A project ID is stable across ordinary metadata refreshes and is never derived solely from a mutable user path.

## Canonicalisation and validation

Registration accepts only a directory selection and optional display name. The backend resolves the selected path before persistence, so nested directories and symlink aliases identify the same top-level worktree. Missing paths, files, non-Git directories, and bare repositories are rejected. The Agent Sentinel application repository is protected from registration.

The validator resolves Git through a backend-owned trusted-candidate policy, then launches its canonical executable path directly; it never executes the first inherited `PATH` entry. macOS and Unix use a fixed validated candidate policy. Windows uses Windows Known Folder APIs for machine Program Files locations and never trusts `ProgramFiles`, `ProgramW6432`, `PATH`, or other launcher-supplied environment roots. Unsupported or unavailable discovery fails closed. Each child has a cleared minimal environment, null stdin, disabled prompts/pagers, deterministic locale, and no inherited Git repository overrides or credential/helper settings. It uses only fixed read-only commands: `rev-parse --is-bare-repository`, `--is-inside-work-tree`, `--show-toplevel`, `--git-common-dir`, `--git-dir`, `worktree list --porcelain -z`, `symbolic-ref -q --short HEAD`, and `rev-parse --verify --quiet HEAD`.

Stdout and stderr are read concurrently into separate 8 KiB capped buffers. Overflow or the three-second timeout kills and reaps the process. Raw Git stderr, command arguments, executable paths, canonical paths, and inherited environment are never forwarded to bridge errors. Missing Git, discovery failures, timeouts, overflow, malformed metadata, bare repositories, detached HEAD, unborn HEAD, and unexpected Git failures remain distinct internal outcomes; only documented expected exit statuses classify detached or unborn state.

The registry represents a normal primary worktree, a detached HEAD, an unborn repository, or an existing linked worktree. Linked worktrees are classified rather than created or changed. The canonical Git common directory is the duplicate location identity, so different nested paths in one worktree cannot create duplicate projects; repositories with matching names remain distinct.

Each new registration also stores an opaque, versioned strong fingerprint for the canonical common-directory object. On macOS, `strong_v1:macos_object_v1` hashes the device, inode, and filesystem birth time. On Windows, `strong_v1:windows_object_v1` hashes the volume serial, file index, and creation time. The Windows common directory is opened with `CreateFileW`, metadata-only access, read/write/delete sharing, `OPEN_EXISTING`, and `FILE_FLAG_BACKUP_SEMANTICS`; its RAII-owned directory handle is passed to `GetFileInformationByHandle`. A target-gated native Windows test exercises that production path. Linux intentionally fails closed in Phase 3A until a mount/object/generation provider can establish a replacement-resistant identity; device/inode alone is never accepted as strong. The raw tuple and digest never cross the bridge. Revalidation requires both the common-directory identity and the strong fingerprint, so delete/recreate reports an identity change rather than retargeting the original `ProjectId`.

## Primary working-tree protection

Phase 3A derives the protected primary root from Git's machine-readable worktree listing rather than inferring it from the common-directory parent. This also correctly handles repositories created with `--separate-git-dir`. The application repository is inspected through the same trusted boundary, and candidates are rejected when their common-directory identity and instance fingerprint match it; linked worktrees therefore cannot bypass the protection. Later worktree creation and execution must distinguish Sentinel-created worktrees from this protected primary location and must never run an agent directly in it. This slice only records the metadata; it does not alter an index, worktree, branch, ref, remote, configuration, or hook.

## Persistence and bridge

Forward-only migrations `0002_projects.sql`, `0003_project_fingerprint.sql`, and `0004_project_fingerprint_scheme.sql` create typed `projects` columns with a stable primary key, unique repository location identity, versioned fingerprint state, canonical metadata, validation state, and timestamps. Existing empty fingerprints are marked `legacy_unverified`; prior device/inode values are marked `weak_v0`. Those projects remain listable and bridge-visible as `requires_trusted_revalidation`, but cannot be treated as ready for future isolation work.

An explicit user-requested revalidation performs a one-time trusted upgrade for a legacy or weak record: it inspects the backend-stored location, confirms the canonical common-directory identity, applies protected-repository checks, and atomically writes `strong_v1`. This is a trust reset that adopts the currently inspected repository; it does not prove historical continuity. Once strong, ordinary revalidation never overwrites the fingerprint and requires an exact scheme/digest match.

The typed Tauri boundary provides `register_project`, `list_projects`, `get_project`, `revalidate_project`, and `unregister_project`. Requests cannot include a Git executable, command, arguments, shell, environment, Git directory, common directory, identity, or worktree policy. There is deliberately no project picker UI in this slice: PLAN places normal desktop workflow in Phase 6.

## Tests

Focused fixtures cover normal, nested, no-remote, detached, unborn, corrupt metadata, Unicode/space, symlink, bare, non-repository, missing, linked-worktree, separate-Git-directory, strong delete/recreate, and normal-change fingerprint-stability cases. Controlled executables cover hostile inherited PATH, cleared Git overrides, null stdin, stream overflow, and timeout cleanup. Registry persistence covers registration, deterministic listing, legacy/weak one-time upgrades, strict identity-mismatch revalidation, and registry-only removal. Desktop bridge tests assert a path-free DTO, safe errors, and linked-worktree protection of the application repository. Fixture worktrees are used only to inspect Git's linked-worktree metadata; production Phase 3A code does not create worktrees.

## Completion evidence

Phase 3A is complete. Final validation on aarch64-apple-darwin passed 98 Rust tests, 92 frontend tests, `cargo fmt --all --check`, workspace Clippy with warnings denied, the frontend production build, and the Tauri no-bundle build. The final focused reviews found no remaining P1/P2 issues. Windows-native directory-fingerprint tests are target-gated and present, but were neither executed nor cross-compiled locally because no Windows Rust target is installed. Linux strong-fingerprint registration intentionally fails closed. No production repository or worktree was mutated, and no sentinel-probe or real Codex/Claude product-agent task was executed.

## Deferred work

Phase 3B will add backend-managed worktree lifecycle and protected execution isolation. Phase 3C will add diff integration. Phase 4 Codex detection and execution remains not started. No Sentinel probe or real product-agent task was executed while implementing or testing this slice.
