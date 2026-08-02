# Phase 3C-B2 — Internal Bounded Textual Extraction Contract

## Status and authority

Phase 3C-B2 contract design is **APPROVED / COMPLETE**; its final contract
re-review is **CLEAN**, with no remaining blocking contract P1/P2 issues. This
document is the approved internal baseline for later B2 implementation.
Implementation is **NOT STARTED**: B2-A, B2-B0, B2-B1, B2-C, and B2-D are each
**NOT STARTED**. `PLAN.md` remains the operational roadmap: it requires Phase
3 Git isolation/diffs and eventual final-diff review, but does not specify B2's
format, API, limits, or eligibility. The completed B1 documents reserve B2 for
future bounded textual extraction. The decisions labelled **contract default**
below are necessary conservative choices, not claims that PLAN.md already
specified them.

No B2 production code, parser, extraction command, Tauri command, TypeScript
DTO, or UI exists. B2-B0 grammar/fixture work has not been written or approved.
Phase 3C-C owns any future public bridge, serialization, and rendering contract
and remains **NOT STARTED**.

## Objective, request identity, and trust boundary

B2 will be a private, read-only extraction step for one selected change in a
validated Sentinel-managed linked worktree. It returns bounded, normalized
textual-diff records only. It neither applies a patch nor mutates Git or the
filesystem.

B2 must operate inside its own ProjectId-locked request. The initial private
request is conceptually:

```text
BoundedTextualExtractionRequest {
  project_id: ProjectId,
  worktree_id: WorktreeId,
  selection: NonAuthoritativePathSelection,
}
```

`ProjectId` and `WorktreeId` are typed internal identifiers. The request
targets exactly one managed worktree and exactly one selected path. It must
obtain paths only from fresh authoritative inventory and fresh B1-grade
classification for that exact `WorktreeId`. A previous B1 result is not durable
authority; caller-supplied paths, surfaces, modes, counts, eligibility, or
classifications are non-authoritative and must be ignored except as a selection
candidate. The candidate must exactly match fresh inventory or the request
fails. A future public request shape is deferred to 3C-C.

Initial persisted lookup must prove that the project and worktree rows exist,
`project.id == request.project_id`, `worktree.id == request.worktree_id`,
`worktree.project_id == request.project_id`, and lifecycle,
ownership/authorization, managed path, and repository
association are eligible and valid. The refreshed final lookup repeats those
exact identity and eligibility checks. A ProjectId lock is not proof of this
association: missing, replaced, reassociated, cross-project, or ineligible rows
return no extraction. B2 must never derive an arbitrary/first worktree from a
project, resolve one by filesystem path, or accept a caller-authoritative path.

**Contract default:** the first implementation does not batch paths. A
32-path/64-surface ceiling is reserved for a separately reviewed extension and
is not an active request capability.

## Internal result representation

The authoritative internal representation is structured, bounded data, not raw
unified-patch text:

```text
TextualExtractionResult { files: Vec<TextualFileExtraction> }
TextualFileExtraction { path_key, staged: Option<TextualSurfaceExtraction>,
  unstaged: Option<TextualSurfaceExtraction>, non_extractable_surfaces }
TextualSurfaceExtraction { old_mode, new_mode, hunks,
  total_added_lines, total_deleted_lines }
TextualHunk { old_start, old_count, new_start, new_count, lines }
TextualDiffLine { kind: Context | Addition | Deletion, text: String,
  old_line_ending: Option<LineEnding>, new_line_ending: Option<LineEnding> }
LineEnding { Lf, CrLf, None }
```

`path_key` is an internal validated identity, never a raw filesystem path.
Staged and unstaged surfaces remain separate; B2 never synthesizes a combined
patch. The result contains no raw Git headers/stderr, binary payload, complete
file content, symlink target, conflict-stage, or submodule content.

Line terminator bytes are excluded from `text`. For a deletion,
`old_line_ending` is present and `new_line_ending` is absent; for an addition,
the reverse holds; for context both are present and equal. `Lf` and `CrLf`
preserve source terminators; `None` preserves an unterminated final line on the
applicable old or new side. A raw `\r\n` at a line boundary is `CrLf`; a carriage
return elsewhere is content and is rejected by the initial C0-control policy.

The Git `\ No newline at end of file` record is parser metadata, never line
content. It applies only to the immediately preceding applicable old or new
diff line; duplicate, contextless, or malformed markers fail closed. The B2-B0
grammar must define the exact side assignment for every legal marker placement.
This preserves stable LF/CRLF, LF-to-CRLF and CRLF-to-LF, old/new missing final
newlines, and newline-added/newline-removed transitions.

Structured records make limits and strict validation deterministic, remove
ambiguous header parsing from consumers, preserve surface identity and newline
state, and permit a later 3C-C serialization review without making
terminal/control/path data a consumer boundary.

## Eligibility policy

Only a freshly B1-classified `TextEligible` surface is extractable. It must
have numeric numstat evidence, a bridge-safe validated path, regular-file
semantics where applicable, authoritative surface identity, and no binary,
conflict, gitlink/submodule, sparse/skip-worktree, mode/type, ownership, or
attribute ambiguity.

| B1 surface state | B2 result |
| --- | --- |
| TextEligible and all requirements above | Extractable |
| Binary, ModeOnly, symlink, gitlink/submodule, sparse/skip-worktree, unsupported type | Explicit `NotExtractable` surface |
| Untracked, conflict, configured/deferred or unsupported attribute | Explicit `NotExtractable` surface |
| NotApplicable | Omitted for that surface |
| malformed/unavailable classification, stale/operational failure | Safe error; never empty success |

A file may retain one extractable surface while explicitly representing the
other as non-extractable. If neither surface is extractable, a successful
bounded `no_textual_surface` result is valid only when fresh B1 classification
and fresh attribute evidence establish that semantic state. Timeout, validation,
parser, stale, or limit failure must never become that result.

Staged extraction compares validated persisted base to index; unstaged
extraction compares index to worktree. Staged-plus-unstaged paths yield two
records. A staged addition has an explicit absent pre-image; deletion may have
hunks only when its surface is text-eligible. Unstaged untracked content is
not extractable.

Every C1/C2/C3 and E1/E2/E3 pass obtains normalized
`AttributeEligibilityEvidence` for `filter`, `diff`, `text`, `binary`, `eol`,
and `working-tree-encoding`. Each entry records the attribute name, Git state
(`unspecified`, `set`, `unset`, or `value`), a value only when Git returned one,
and the resulting eligibility/deferred reason. B2-B0 must also determine
whether the supported Git version exposes `crlf`, `ident`, or another
encoding-related attribute on the chosen extraction path; any
semantics-affecting attribute is added to this evidence before B2-B1 begins.

**Contract default:** every listed attribute must be exactly `unspecified`.
`set`, `unset`, and `value` for any listed or newly identified
semantics-affecting attribute make both surfaces `NotExtractable`; B2 performs
no normalization or interpretation of it. This includes built-in
`working-tree-encoding`, `eol`, `text`, and `binary`; `--no-textconv` is not
claimed to disable them. Configured diff drivers, filters, and
textconv-related behavior are likewise non-extractable, and no helper may
execute. Attributes proven non-transforming for the exact reviewed argv may be
ignored only when B2-B0 documents why.

The normalized attribute evidence must compare exactly across C1/C2/C3, and
each E pass must use matching fresh evidence. Attribute-only drift—even with
the same inventory, numstat, and visible hunk text—returns `change_stale`. No
file-read workaround for a deferred path is permitted.

## Command boundary and mandatory parser gate

The intended read-only forms are conceptually `git diff --cached <base> --
<validated-path>` and `git diff -- <validated-path>`. Every eventual invocation
must have fixed argv, literal pathspec handling, `--` before the validated
literal path, `--no-ext-diff`, `--no-textconv`, `--no-color`, and `--no-renames`.
It reuses the trusted executable, clean environment, null stdin,
pager/prompt/config isolation, fsmonitor suppression, managed `GIT_WORK_TREE`,
repository-local `core.worktree` protection, remaining-deadline cap, output
caps, and kill-and-wait behavior.

Final extraction argv and any production parser are **unauthorized** until
B2-B0 below receives a clean review. External diff, textconv, custom diff
drivers, clean/smudge/process filters, aliases, pager, hooks, shell
construction, stock `git status`, `git diff-files`, raw file reads,
mutating/remote Git commands, and filesystem deletion are prohibited.

The parser is strict UTF-8 structured parsing. It rejects invalid UTF-8, NUL,
ESC, bidi controls, C0 controls other than tab, malformed headers, unexpected
or mismatched path/header data, binary markers, duplicate/contradictory hunks,
inconsistent hunk counts, truncation, trailing unparsed bytes, and overlong
lines. Ordinary Unicode scalar values are preserved after validation. 3C-C
must separately define safe presentation.

### B2-B0 — unified-diff byte grammar and fixture corpus

B2-B0 is documentation and review only. Before any extraction parser or
production extraction code exists, it must publish and receive clean review for
the exact staged and unstaged fixed argv; accepted and rejected byte grammars,
framing, and parser state machine; canonical valid and adversarial fixture
corpora; path/header association; all bounds; and every line-ending/no-newline
rule.

The grammar must define exactly one selected file section per extraction
command, allowing zero only for explicitly valid no-change input; extra or
duplicate sections reject. It must define treatment of `diff --git`, `---`,
`+++`, `index`, new/deleted/old mode, similarity/dissimilarity, rename/copy,
binary, and every other extended header. Rename/copy and unexpected extended
headers reject even with `--no-renames`.

It must identify every header path, its byte decoding for quoted paths, tabs,
spaces, quotes, backslashes, and supported bridge-safe newlines; require exact
comparison with the authoritative selected path; and reject mismatched prefixes,
extra paths, and `/dev/null` except on the valid addition/deletion side.

It must fix the exact `@@ -old_start,old_count +new_start,new_count @@` grammar,
omitted-count semantics, numeric range/overflow handling, optional section-text
policy, line-prefix grammar, and old/new/context count reconciliation. It must
also fix the no-newline marker's exact bytes, placement, and old/new side
applicability; malformed, duplicate, or contextless markers reject. Content
resembling `diff --git`, `---`, `+++`, `@@`, or a backslash remains hunk content
when its parser state and line prefix say so; it never escapes a hunk merely by
resembling a header.

No trailing unparsed bytes, truncated final record, second section, or
binary/mode-only fallback is valid. The corpus must contain valid staged and
unstaged modifications, addition, deletion, CRLF, absent final newline,
bridge-safe unusual paths, and embedded header-like content, plus every
rejection case above. B2-B0 stops only after its grammar review is clean with no
unresolved ambiguity. It may have a separate reviewed contract commit after
approval; B2-B1 parser/extraction implementation begins only in a later task.

## Deterministic bounds, accounting, and outcomes

Existing stricter authoritative limits always win. The following B2 defaults
are explicit and apply to the whole request as stated; no bound permits
truncation or partial hunks.

### Per-child capture and duration

| Constant | Value | Outcome |
| --- | ---: | --- |
| `MAX_CHILD_STDOUT_BYTES` | 2 MiB | `output_overflow` |
| `MAX_CHILD_STDERR_BYTES` | 16 KiB | `output_overflow` |
| child duration | `min(existing 3 seconds, remaining request budget)` | `timeout` |

Each cap applies independently to every Git child; reused B1 inventory,
classification, and attribute operations retain their existing stricter caps.
Overflow terminates, kills and waits for that child, discards all request
evidence, and returns `output_overflow`.

### Request-total budgets

| Constant | Value | Accounting |
| --- | ---: | --- |
| `MAX_TOTAL_CHILDREN_PER_REQUEST` | 88 | Every Git child in the table below |
| `MAX_TOTAL_CAPTURED_STDOUT_BYTES` | 8 MiB | Sum from every child, including failed/repeated passes |
| `MAX_TOTAL_CAPTURED_STDERR_BYTES` | 1 MiB | Sum from every child, including failed/repeated passes |
| `MAX_TEXT_BYTES_PER_EXTRACTION` | 1 MiB | Normalized line-text bytes across both surfaces of one E pass |
| `MAX_TOTAL_PARSED_DIFF_BYTES` | 3 MiB | Normalized textual bytes parsed by E1+E2+E3; discarded data still counts |
| `MAX_TOTAL_RETAINED_EXTRACTION_BYTES` | 3 MiB | All retained normalized structures plus structural overhead |
| `MAX_HUNKS_PER_SURFACE` | 512 | Per eligible surface |
| `MAX_TOTAL_HUNKS_PER_REQUEST` | 3,072 | Sum across both surfaces and E1/E2/E3 |
| `MAX_LINES_PER_SURFACE` | 20,000 | Per eligible surface in one E pass |
| `MAX_LINES_PER_EXTRACTION` | 40,000 | Both surfaces in one E pass |
| `MAX_TOTAL_LINES_PER_REQUEST` | 120,000 | Sum across E1/E2/E3 |
| `MAX_DIFF_LINE_BYTES` | 16 KiB | Validated line text, excluding terminator metadata |
| `MAX_SELECTED_PATH_BYTES` | 4 KiB | Exact request candidate after authoritative validation |
| request deadline | one existing B1-style absolute deadline | All stages, cleanup included |

Authoritative inventory retains its existing path-record and output bounds on
each A/B/C pass. `MAX_SELECTED_PATH_BYTES` does not grant a separate inventory
budget. Staged and unstaged surfaces share every extraction and request-total
budget; a non-extractable surface contributes only bounded metadata.

The direct Git-child budget is fixed, with no retries or unlisted commands:

| Operation | Maximum Git children |
| --- | ---: |
| establish trusted context and initial Git validation | 8 |
| Inventory A, B, C | 8 each / 24 total |
| B1-grade C1, C2, C3 | 12 each / 36 total |
| E1, E2, E3 (fresh attributes plus staged/unstaged extraction) | 4 each / 12 total |
| refreshed final Git validation | 8 |
| **Total** | **88** |

If B2-B0 demonstrates a different command need, it must revise and review this
table before B2-B1; it may not silently consume an unbudgeted child.

For direct equality, parse and retain E1; parse E2 and compare it to E1, then
discard E1 after equality succeeds; retain E2; parse E3 and compare it to E2,
then discard E2 after equality succeeds; retain only accepted E3 through final
validation and return. At most two normalized extraction structures are retained
simultaneously. Direct structural equality remains authoritative; no digest is
used initially. Error, timeout, cancellation, or overflow drops all retained
structures. Earlier discarded structures remain counted in captured and parsed
request totals.

One absolute deadline starts with the request. No inventory, classification,
extraction, persistence, parser, final-validation, cleanup, or E pass receives
a fresh timeout. Async/database work and parser loops are capped by remaining
time and cancellation checks at bounded intervals; timeout returns no partial
output.
Stable internal outcomes are `success`, `no_textual_surface`,
`not_text_eligible`, `change_stale`, `worktree_not_ready`,
`project_unavailable`, `inventory_unavailable`, `classification_unavailable`,
`malformed_output`, `path_mismatch`, `output_overflow`, `resource_limit`,
`timeout`, `git_command_failed`, and `operation_cancelled`. `not_text_eligible`
is a valid non-extractable selected-surface outcome; `no_textual_surface` is
valid only when all selected surfaces are freshly and semantically
non-extractable. Neither includes an operational error. Public Tauri error
mapping is deferred to 3C-C. Errors expose no paths, Git output, commands,
configuration, PIDs, or persisted rows.

## Coherence and acceptance

Matching numstat counts do not bind hunk text: different edits can share the
same counts. The required private sequence is therefore:

```text
lock by ProjectId, one deadline, initial exact ProjectId/WorktreeId rows,
trusted Git context
A -> C1 + AttributeState1 -> E1 -> B -> C2 + AttributeState2 -> E2
  -> C -> C3 + AttributeState3 -> E3
require A=B=C, C1=C2=C3, AttributeState1=AttributeState2=AttributeState3,
  E1=E2=E3
refresh persisted project/worktree rows
refreshed final repository/worktree/index/base validation
deadline check -> return only E3
```

E1/E2/E3 are independently collected normalized structures. Exact equality
covers validated identity, surface, old/new modes, hunk coordinates/order,
every line kind and string value, every old/new line-ending value, no-newline
marker applicability, totals, complete file/surface set, and any explicit
non-extractable state. Direct equality of bounded structures is the initial
choice; a later digest must cover that complete normalization and keep direct
material-field tests.

Every pass rechecks relevant attributes. Any content/index/mode/type/path-set,
filter, lifecycle, row, repository/worktree association, fingerprint,
base/HEAD, parser, deadline, cancellation, or resource mismatch returns no
extraction. After E3, persisted rows are reloaded and must retain the exact
request ProjectId/WorktreeId association plus eligible lifecycle, ownership,
path, repository, fingerprint, and base identity; final Git validation uses
those refreshed records. This is repeated observation, not an assertion of
filesystem immutability after E3.

## Required implementation stages and tests

### B2-A — private contract-derived types and bounds

Allowed scope: private request identity, normalized evidence/newline metadata,
attribute evidence, deterministic bounds, and internal outcomes. Prohibited:
Git extraction, parser, Tauri/TypeScript/UI/public DTO work. Required tests:
identity, newline/equality, attribute-state, and budget-model unit tests. Stop
condition: private types compile and their contract tests are clean. Review
boundary: B2-A review. Commit policy: one reviewed B2-A commit only after that
review; no commit is authorized by this documentation task.

### B2-B0 — parser grammar sub-contract and fixtures

Allowed scope: the reviewed byte grammar and fixture corpus above. Prohibited:
production parser or extraction implementation. Required tests: fixture corpus
review only; parser code must not exist. Stop condition: clean grammar review
with every required framing and rejection rule fixed. Review boundary: B2-B0
security review. Commit policy: a separate contract-only commit may follow
approval; B2-B1 remains a later task.

### B2-B1 — safe extraction primitive and strict parser

Allowed scope: B2-B0-approved fixed argv, bounded runner, parser, normalized
output, and parser/security fixtures. Prohibited: orchestration and public
bridge. Required tests: every accepted/rejected grammar, UTF-8/control/newline,
path, helper-suppression, and per-child limit case. Stop condition: parser is
complete-or-error and B2-B0 rules all pass. Review boundary: B2-B1 primitive
review. Commit policy: one reviewed B2-B1 commit after approval only.

### B2-C — coherence orchestration

Allowed scope: ProjectId lock; exact ProjectId/WorktreeId validation; A/B/C,
C1/C2/C3, attributes, E1/E2/E3, aggregate accounting, refreshed rows, final
Git validation, and disposable race tests. Prohibited: public bridge or later
phase behavior. Required tests: multi-worktree identity, all coherence races,
aggregate accounting, and no-partial-output cleanup. Stop condition: only E3
can be accepted after every comparison/refresh. Review boundary: B2-C security
review. Commit policy: one reviewed B2-C commit after approval only.

### B2-D — internal integration, adversarial validation, and completion

Allowed scope: authorized internal integration, adversarial regression suite,
documentation, holistic review, and B2 completion. Prohibited: a public bridge.
Required tests: complete B2 matrix and full validation. Stop condition: holistic
review clean. Review boundary: final B2 review. Commit policy: one completion
commit only after clean review.

### Phase 3C-C — public bridge

Only then may Tauri command, TypeScript DTO, serialization, UI rendering,
public selection, and safe public error mapping be considered. They are outside
B2.

Required disposable-fixture tests include:

- exact WorktreeId selection with two worktrees in one project; cross-project,
  missing, replaced, reassociated, and refreshed identifier mismatch rows;
- stable LF and CRLF, LF/CRLF transitions, old/new missing final newline,
  newline-added/removed, same visible text with different newline state, and
  malformed/duplicate/contextless no-newline markers;
- B2-B0 accepted/rejected grammar corpus: extra/duplicate sections, path
  mismatch, embedded headers, unexpected extended headers, rename/copy, and
  valid staged/unstaged modification, addition, deletion, and unusual paths;
- every `filter`, `diff`, `text`, `binary`, `eol`, and
  `working-tree-encoding` state; attribute-only drift with stable hunks; no
  helper and no raw-file-read fallback;
- per-child stdout/stderr, total capture/parsed/retained/line/hunk/command
  overflow; staged-plus-unstaged and E1/E2/E3 aggregate accounting; discard of
  earlier evidence; timeout/cancellation at every stage; and no partial output;
- stable staged/unstaged edits, separated dual surfaces, valid
  no-textual-surface, all non-extractable states, E1=E2=E3,
  same-numstat/different-content, E1/E2/E3 content/index/mode/filter races,
  lifecycle revocation, row/association/base changes, and no surviving
  task/child.

## Deferred and prohibited behavior

B2 does not add a public API, patch application, raw patch return, automatic
worktree removal, production descendant/process-group handling, Windows job
objects, B3/3C-C/3C-D/Phase 4 behavior, or real-agent workflows. Automatic
removal remains disabled and production descendant containment remains open.
