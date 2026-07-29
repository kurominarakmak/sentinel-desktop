# Phase 3C-A — Trusted Read-Only Change Inventory

Phase 3C-A adds on-demand, read-only inventory for a verified Sentinel-managed
worktree. It uses a fixed `git --no-optional-locks -c core.fsmonitor=false -c
core.untrackedCache=false status --porcelain=v2 -z --untracked-files=all
--ignore-submodules=none --no-renames` operation through the trusted Git
runner. The runner also sets `GIT_OPTIONAL_LOCKS=0`, clears its environment,
uses null stdin, disables prompts and pagers, bounds streams, times out, kills,
and reaps children.

The inventory is complete-or-error: 256 KiB stdout, 16 KiB stderr, 1,000
records, and 4 KiB valid UTF-8 repository-relative paths are maximums. No
partial inventory is returned. Porcelain-v2 records `1`, `u`, and `?` are
strictly parsed; rename/copy (`2`), ignored (`!`), headers, malformed data,
invalid UTF-8, and path aliases fail closed. Rename detection is disabled;
staged and unstaged changes remain distinct, untracked contents are not read,
binary contents are not classified or returned, and submodules are represented
only by bounded status metadata without recursive inspection.

Repository-relative paths reject every leading-backslash spelling on every
platform, including Windows rooted, UNC, device, and verbatim forms. Internal,
non-leading backslashes remain ordinary filename bytes under the documented
Unix-safe policy. An ordinary `1` record with `XY == ".."` is malformed: the
only valid clean inventory is completely empty status output.

Inspection accepts no frontend path or Git configuration. It reuses Phase 3B
backend-owned exact leaf validation, project identity/fingerprint verification,
linked/non-primary protection, exact worktree metadata checks, and the
per-project operation lock. Only `ready` and `retained_dirty` rows are eligible;
the leaf and row are revalidated after status before returning an inventory.
Absolute paths, raw Git output, executable paths, fingerprints, and repository
metadata never enter the future bridge shape. Inventories are not persisted and
there is no Tauri command or React control in this batch.

Disposable integration fixtures exercise the actual trusted operation through
the desktop service and prove managed and primary index bytes, HEAD/branch,
status, tracked content, and untracked content remain unchanged, with no
`index.lock` left behind. They configure a disposable `core.fsmonitor` helper
and verify it is not executed; the fixed optional-lock and untracked-cache
safeguards are covered by the same exact-index evidence. These checks prove
the documented command configuration rather than claiming that every possible
filesystem metadata side effect is impossible.

Phase 3C-B textual diff extraction, Phase 3C-C public bridge work, and Phase
3C-D final integration work remain deferred. Windows-native path/reparse tests
remain target-gated; Linux strong-fingerprint registration remains fail-closed.
The documented same-user filesystem TOCTOU boundary remains; descriptor-relative
atomic protection is not claimed.

Phase 3C-A is complete. Final aarch64-apple-darwin evidence is 124 passing
Rust tests, 92 passing frontend tests, `cargo fmt --all --check`, workspace
Clippy with warnings denied, the frontend production build, and the Tauri
no-bundle build. Focused reviews found no remaining P1/P2 issues. Windows-native
path and reparse validation remains target-gated and was not run locally.
