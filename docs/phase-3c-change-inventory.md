# Phase 3C-A — Trusted Read-Only Change Inventory

Phase 3C-A is **COMPLETE**. The former stock `git status --porcelain=v2` inventory
and Phase 3B stock-status cleanliness check are not production-reachable: a
repository-configured clean or process filter can execute while Git compares a
working-tree file. `--no-optional-locks`, fsmonitor suppression, a clean child
environment, and bounded execution do not prevent that code execution.

AH1 fails inventory and cleanliness decisions closed with fixed unavailable
errors after the existing ownership/lifecycle validation. It never represents
that condition as an empty or clean inventory, and B1 is blocked because it
requires a complete inventory. The retained stock-status invocation is a
test-only disposable regression control. AH1 also introduces non-authoritative
base-to-index `diff-index --cached --raw -z --no-renames` plumbing; it is not a
complete inventory and makes no unstaged or clean claim. Marker tests are
required before any new command becomes production-reachable.

A corrected disposable forensic fixture stages the base-to-index change before
installing a unique clean or process driver, then invokes only the exact
`diff-index` command. On the tested Unix/macOS target, both the direct fixed
command and the sentinel-git wrapper leave those markers absent. The earlier
staged-foundation marker failure was fixture contamination: its read-only
snapshot invoked the retained test-only stock-status control after filter
configuration. The AH1 desktop inventory path does not invoke the staged
foundation; it stops at `inventory_unavailable`. This evidence keeps the
staged foundation non-authoritative and does not restore complete inventory.

The generic trusted-runner audit uses a disposable direct helper and a
fixture-local, test-only `FixtureProcessSupervisor`. Before any descendant can
launch, the helper becomes leader of a dedicated process group, reports a
strict, bounded FIFO GroupReady frame, and blocks at a launch gate. The
supervisor binds that event to the actual trusted test-runner `Child::id()`
before arming its PGID. The disposable descendant is itself an owned test
`Child`; its `Child::id()` is the sole source for `DescendantLaunched`, so a
different live PID in the same group is rejected. The supervisor verifies that
PID is live, distinct, and in the armed group with `getpgid`. It observes zero
launches before release and exactly one launch after release. The deterministic
SIGTERM-ignoring fixture emits `TermIgnoreReady` only after that owned child
successfully installs `SIG_IGN`; fallback evidence requires ordered SIGTERM,
a bounded Alive group probe, SIGKILL, and final ESRCH absence. Group SIGTERM
and SIGKILL remain its only emergency cleanup authority. Only ESRCH proves group absence; EPERM
and unexpected syscall errors retain cleanup failure and armed ownership. PID
files, manifests, and ready markers are fallible protocol-under-test data;
their absence, malformation, or duplication cannot determine cleanup. The
post-gate broken publication regression proves the group is still cleaned and
the same runner handle terminally joined. Successful group cleanup disarms the
PGID before the Drop-only fallback. On tested macOS/aarch64, the production runner still kills
and waits for its direct child while a descendant remains alive immediately
afterward; the supervisor's process group is test containment only and is not a
production runner change. Diagnostic join deadlines retain the same handle
across multiple barrier releases; cleanup runs before the controlled terminal
cancellation join.
Timeout and stdout/stderr overflow converge on the same direct-child
`start_kill` plus `wait` code path. No process group is created and no
descendant containment is claimed. This is a separate generic-runner hardening
issue, not an AH1 blocker: the AH1 production boundary prevents
repository-controlled clean/process helpers from launching in the first place.

During AH1, production does not run porcelain status and does not return a
`WorktreeChangeInventory`, an empty inventory, or `clean=true`. It first
performs the established backend-owned leaf, project identity/fingerprint,
linked/non-primary ownership, exact worktree metadata, lifecycle, and
per-project-lock validation, then returns fixed `InventoryUnavailable`. No
changed path is silently omitted as clean, and B1 cannot select a path or run
classification while this prerequisite is unavailable.

The former porcelain-v2 parser and its complete-or-error record contract remain
only as retired parser/test-control reference. They must not be read as a
current production inventory claim. AH2 must replace them with a complete
filter-free pipeline before ordinary staged, unstaged, untracked, conflict, or
clean outcomes can be returned again. The AH1 staged `diff-index` foundation
is bounded and strict, but it is base-to-index metadata only: it cannot imply
complete worktree cleanliness, absence of unstaged changes, or absence of
untracked files.

Repository-relative path validation rejects Windows-rooted, UNC, device,
verbatim, drive-relative, and every backslash-containing lookup spelling on
Windows. Internal non-leading backslashes remain ordinary filename bytes only
under the Unix-safe policy. Absolute paths, raw Git output, executable paths,
fingerprints, and repository metadata never enter the future bridge shape.
Inventories are not persisted and there is no Tauri command or React control in
this batch.

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

AH2 supplies the authoritative filter-free inventory and is completed by the
Phase 3C-A authoritative-inventory commit. The final holistic review found no
remaining production P1/P2 issues. It collects two matching bounded
snapshots: staged changes come from `diff-index --cached --raw -z
--no-renames <base>`, untracked paths from `ls-files --others
--exclude-standard -z`, and unmerged stages from `ls-files --unmerged -z`.
Tracked worktree state comes from `ls-files --stage -z` plus per-path `hash-object
--no-filters`, not `diff-files` or status, because raw `diff-files` can still
trigger a configured clean filter. Every command has fixed argv, a cleared
environment, a trusted empty HOME/XDG/global-config context, literal path
semantics, disabled fsmonitor/untracked cache, NUL framing, output bounds, and
strict raw-byte parsers. Lookup paths are lexically validated immediately
before filesystem and `hash-object` access: Windows rejects backslashes,
drive, UNC, device, rooted, and traversal spellings; Unix retains literal
backslashes only where they remain ordinary filename bytes. `ls-files -t -z`
detects skip-worktree entries, which currently fail closed as
`InventoryUnavailable` rather than being misreported as deletions. Index
snapshots, command results, repository identity/fingerprint, repository root,
and base must agree twice before merge; otherwise inventory is unavailable.
Gitlinks are currently fail-closed rather than flattened into an incomplete
nested-submodule claim.
Successful empty output is clean only after every component and both snapshots
agree. One 15-second monotonic inventory deadline spans both snapshots and all
sequential `hash-object` calls; each child timeout is capped by the remaining
shared budget, and deadline exhaustion returns neither partial nor clean
output. Managed-worktree removal remains disabled even with an authoritative
inventory. B1 is complete under B1H: its classification boundary consumes only
this inventory, requires C1=C2=C3 repeated evidence, constructs success from
C3, and reloads persisted eligibility before final acceptance. B2 remains **NOT
STARTED**.

Every authoritative invocation also pins `GIT_WORK_TREE` to the already
validated managed leaf, overriding repository-local `core.worktree`; linked
worktree discovery still selects its own `.git` indirection and index. The
outer locked desktop inspection establishes the same 15-second deadline before
its initial validation and retains it through final validation; nested inventory
helpers reuse that absolute deadline rather than allocating a fresh budget.

## AH1 freeze and test-harness scope

AH1 freezes only the production fail-closed boundary. Its Unix fixture
supervisor, FIFO observations, process-group cleanup, descendant identity
checks, and SIGTERM/SIGKILL fallback evidence are test-only proof machinery;
they neither add production containment nor change the trusted runner's
direct-child-only termination contract. Further test-proof strengthening is
non-blocking unless it changes the final Phase 3C-A production conclusion, and
is deferred to the holistic Phase 3C-A review. AH2 does not depend on
repository-controlled helper execution: AH1 prevents that execution while the
complete inventory is unavailable.
