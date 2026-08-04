# Phase 3C-B2-B0 — Unified-Diff Byte Grammar and Fixture Gate

## Status, scope, and approval gate

This is the implemented B2-B0 grammar and fixture gate. B2-A remains private;
B2-B1 consumes this envelope through a fixed bounded Git command and complete
parser. The corpus remains static evidence rather than a runtime authority.

The contract defines one future staged stream and one future unstaged stream.
Each stream is independently bounded, is requested for exactly one already
resolved authoritative internal path, and yields exactly one surface state:
`Absent`, `NonExtractable`, or `Extractable`. It never combines the streams.

## Proposed future command-output envelope — approval required

B2-B1 must not implement an argv until B2-B0 approves the following proposed
envelope (including the two path decisions below):

```text
git -c core.quotePath=false -c color.ui=false -c core.fsmonitor=false \
  diff [--cached for staged only] --no-ext-diff --no-textconv --no-color \
  --no-renames --full-index --src-prefix=a/ --dst-prefix=b/ -- <literal path>
```

The command must use B1's trusted executable, isolated configuration and
environment, literal pathspec policy, pinned managed `GIT_WORK_TREE`, null
stdin, bounded stdout/stderr, and the one outer request deadline. Pager,
external diff, textconv, diff drivers, filters, aliases, user prefixes,
mnemonic prefixes, fsmonitor, and mutable host configuration are not authority.
Semantic attributes must have already passed the B2 policy gate. Non-zero exit,
overflow, timeout, cancellation, path mismatch, or malformed output fails
closed and returns no partial evidence.

`--full-index`, `--src-prefix`, `--dst-prefix`, and the exact quoted-path
decoder are B2-B0 approval decisions, not existing production behavior. If a
Git version cannot provide this stable envelope, B2-B0 is blocked.

## Raw stream rules

The future primitive receives one complete bounded stdout byte vector. Streaming
implementations are allowed only if they accept exactly the same language and
do not expose a result before EOF. Empty stdout is the sole representation of
an absent surface. Every non-empty record ends in byte `0A`; EOF mid-record,
trailing bytes, a second section, or any unconsumed byte is malformed.

Headers are ASCII-only. Hunk content is byte-oriented until its prefix and
record boundary have been recognized. A content payload is strict UTF-8, has at
most `MAX_DIFF_LINE_BYTES` (16 KiB) after prefix and before ending metadata,
and contains no NUL, ESC, forbidden C0 control, or bidi control. No lossy
decoding is permitted. Header/section input is capped by the B2-A per-line,
per-surface, per-extraction, and request-total limits: 512 hunks/surface,
20,000 lines/surface, 1 MiB text/extraction, 3 MiB parsed/request, and the
existing child stdout/stderr caps.

The only accepted CR is a single byte immediately before a record LF in a
hunk body payload; it maps to `LineEnding::CrLf` and is removed from text. Any
other CR is literal control content and rejects. A non-CR body LF maps to
`LineEnding::Lf`. A line followed by the marker below maps to
`LineEnding::None` for the marker's applicable side. This is conservative:
literal terminal CR plus LF is indistinguishable from CRLF bytes and is treated
as CRLF, while literal interior CR rejects.

### Empirical byte observations (Git 2.48.1, macOS, disposable repositories)

The following observations were captured only from newly created repositories
under `/private/tmp`, using the proposed fixed-diff options and `xxd -p`; no
Agent Sentinel, production, or user repository was a Git target. A source
record `old\r\n` was emitted in a deletion body as `2d 6f 6c 64 0d 0a`, while
`old\n` was `2d 6f 6c 64 0a`. A literal interior CR (`old\rX\n`) was emitted
as `2b 6f 6c 64 0d 58 0a`. This proves the raw rule above for Context,
Deletion, and Addition records alike.

For a new side without a final terminator, Git emitted `+new\n` followed by
the marker bytes and a final record LF. For both sides without final
terminators it emitted `-old\n`, marker, `+new\n`, marker. A staged addition
without a final terminator emitted `@@ -0,0 +1 @@`, `+added\n`, then the same
marker. These transcripts establish old-only, new-only, and both-side marker
positions for deletion/addition lines. Context evidence is fixture `P14`: Git
2.48.1 on macOS, in a newly allocated `/private/tmp/b2-b0-context-*`
repository, transformed `6f6c640a66696e616c` into `6e65770a66696e616c` using
the envelope in this document. Its captured fixture SHA-256 is
`4f86959f56f9d0d0bd39c6c15c5308a82cfa651b2982f9ea654c53fe9f8a988c`.
The body bytes are `20 66 69 6e 61 6c 0a` followed by exactly one marker record
and EOF: it is Context, both sides lack final LF, and no later applicable line
exists. It maps exactly to Context / old=None / new=None / old-marker=true /
new-marker=true. One-sided Context markers, duplicate, malformed, or non-final
Context markers reject; one-sided newline divergence is Deletion plus Addition.

## One-file grammar

Terminals are ASCII literals. `LF` is `0A`; `SP` is `20`; `HEX40` is exactly
40 lower-case hex digits; `UINT` is `[0-9]+` with no leading zero unless the
value is exactly `0`; all conversions are checked into `u64`.

```text
stream          = empty / file-section EOF
file-section    = diff-header LF index-header LF mode-headers path-headers hunks
diff-header     = exact-diff-header(authoritative-path)
index-header    = "index " HEX40 ".." HEX40 [SP mode]
mode-headers    = *(old-mode / new-mode / new-file-mode / deleted-file-mode)
path-headers    = old-path LF new-path LF
old-path        = exact-old-header(authoritative-path) / "--- /dev/null"
new-path        = exact-new-header(authoritative-path) / "+++ /dev/null"
hunks           = *hunk
hunk            = hunk-header LF *body-record
hunk-header     = "@@ -" range SP "+" range SP "@@" [SP section-text]
range           = UINT ["," UINT]
body-record     = context / deletion / addition / no-newline-marker
context         = SP payload LF
deletion        = "-" payload LF
addition        = "+" payload LF
no-newline-marker = "\\ No newline at end of file" LF
```

For an ordinary modification, `index-header` carries its mode suffix and no
new-file/deleted-file mode header is permitted. A staged addition or deletion
uses an index header without that suffix plus exactly the corresponding
`new file mode` or `deleted file mode` header. No other combination is
accepted.

`old mode`, `new mode`, `new file mode`, and `deleted file mode` are permitted
only in their corresponding ordinary, addition, or deletion forms. A mode is
exactly six ASCII octal digits and must be a supported regular-file mode.
Additions require `--- /dev/null` and `new file mode`; deletions require
`+++ /dev/null` and `deleted file mode`. Ordinary modifications require both
`a/` and `b/` headers. Mode-only output is not textual evidence: it must have
been classified as `NonExtractable(ModeOnly)` before this grammar is invoked.

`similarity index`, `dissimilarity index`, `rename from`, `rename to`, `copy
from`, `copy to`, `Binary files differ`, `GIT binary patch`, `diff --cc`,
`diff --combined`, combined hunk headers, gitlink/submodule output, unknown
extended headers, and more than one `diff --git` header are forbidden. They
are either B2 policy non-extractable states or `MalformedOutput`; they cannot
be silently skipped.

## Path headers — exact authoritative-byte comparison

The path authority is the resolved internal identity, never the UI selection.
Under the proposed `core.quotePath=false`, `--src-prefix=a/`, and
`--dst-prefix=b/` envelope, the parser does **not** split or decode a generic
`diff --git` token pair. It constructs and byte-compares the complete expected
header: `diff --git a/` + authoritative UTF-8 path + ` b/` + the same path.
This remains unambiguous even when the path contains spaces because the entire
known byte sequence is compared at once.

Empirical Git 2.48.1 output uses no separator for an ordinary simple path but
emits one empty timestamp separator (`TAB`) after an unquoted path containing
spaces. It emits C-quoted paths for a literal backslash or double quote even
with `core.quotePath=false`. Therefore the fixed grammar accepts only these
two canonical forms: an unquoted exact authoritative UTF-8 path followed by
at most one empty `TAB` separator, or a Git C-quoted exact path with no TAB.
The decoder accepts only `\\`, `\"`, `\t`, `\n`, and exactly three octal digits;
it decodes bytes before strict UTF-8 validation and rejects every other,
incomplete, or non-canonical escape. `diff --git` likewise accepts either two
known unquoted constructed paths or two exact C-quoted constructed paths; it
never splits an arbitrary path at the first space. The decoded bytes must
equal the authoritative internal path exactly. No alternate representation is
accepted: an unquoted non-space path has no TAB; an unquoted path containing
one or more spaces has exactly one final TAB in `---`/`+++` only; quoted forms
and `/dev/null` have no TAB; multiple TABs or any byte after TAB rejects.

| Header | Canonical operands | TAB | Evidence |
| --- | --- | --- | --- |
| `diff --git` | two unquoted `a/`/`b/` operands, or two C-quoted operands | forbidden | P11; path capture table |
| `---` | `a/` path or `/dev/null` | exactly one only for unquoted space path | P11 |
| `+++` | `b/` path or `/dev/null` | exactly one only for unquoted space path | P11 |

ASCII, dash, spaces, single quote, and valid Unicode are unquoted; backslash
and double quote are C-quoted; TAB, controls, bidi, invalid UTF-8, and paths
over the B2-A path bound are rejected before output. Composed/decomposed names
are compared as emitted authoritative bytes, without Unicode normalization.

The authoritative path is strict UTF-8 and compared without Unicode
normalization, slash rewriting, or lossy conversion. Leading dashes, spaces,
Unicode, and backslashes are representable. Tabs, LF, NUL, ESC, forbidden C0,
and bidi controls in the resolved path are rejected for B2 extraction before
this grammar, making the literal `TAB` separator unambiguous. `/dev/null` is
valid only on the absent side of a new-file/deleted-file section. Any header
that differs from the constructed expected bytes is `PathMismatch`; a second
section is `MalformedOutput`.

### Empirical unusual-path capture

Git 2.48.1 was run in a newly allocated `/private/tmp/b2-b0-paths-*`
repository with the proposed options, `--`, fixed `a/`/`b/` prefixes, disabled
rename/color/external-diff/textconv, and `core.quotePath=false`. Raw header
SHA-256/hex transcripts were recorded for ASCII, single/multiple/leading and
trailing spaces, leading dash, BMP and supplementary Unicode, composed and
decomposed Unicode, backslash, double quote, single quote, and `a/`, `b/`,
`---`, `+++`, and `@@`-prefixed names. Spaces produced literal bytes in the
`diff --git` header and one trailing TAB in each old/new header. Backslash and
double quote produced C-quoted headers with `\\` and `\"` escapes. Leading
dash and ordinary Unicode were literal; no display normalization is used for
comparison. On the current macOS filesystem the decomposed spelling was
observed as composed output, so B2 accepts only the authoritative path bytes
supplied by later B2-C validation and rejects any output that differs; it does
not normalize to match a UI spelling. Invalid UTF-8/control path artifacts are
negative raw fixtures rather than repository-path experiments.

| Authoritative filename bytes (hex) | Raw-output SHA-256 | Observed header form |
| --- | --- | --- |
| `706c61696e2e747874` (`plain.txt`) | `bb9d752179d5491ea6ce0eb95119ed9446746f40ed09f9b1221255f5ab5c4ad1` | unquoted, no TAB |
| `6f6e652073706163652e747874` | `073acb9f57db424c0ceee465b7740bf441543b95b38704ec1220d19e5c8506e8` | unquoted; old/new each end in TAB |
| `2d646173682e747874` (`-dash.txt`) | `5418527bf6665efdd573a1d9e7fac8614a96ca86392dd2b46ad7ab3667bd8fc7` | unquoted literal leading dash |
| `737570702df09f98802e747874` | `6d32785e61342fb6a63841a05eefc39987fbee5393542cfc8fdc9c95330a5b7d` | unquoted supplementary-plane UTF-8 |
| `6261636b5c736c6173682e747874` | `6e30b629a056230287411ec87bf5a6949755866deeff59d10a7b2826db21ff3f` | C-quoted with `\\` escape |
| `646f75626c652271756f74652e747874` | `5d96967fad1db97666be78b438d1ef8984ad77b6cac97a4bbf4eb93998d04e64` | C-quoted with `\"` escape |

## Hunk and line mapping

An omitted range count means one. `0` is accepted only for a zero-length range;
the start is accepted exactly as emitted and stored in `u64` without arithmetic
normalization. Section text is optional, is ASCII/strict UTF-8 without controls
or bidi, has a 4 KiB draft bound, and is discarded rather than retained. Any
overflow, malformed punctuation, count mismatch, or out-of-order hunk rejects.

Context maps to `TextualDiffLineKind::Context` and contributes one old and one
new count; deletion maps to `Deletion` and contributes old only; addition maps
to `Addition` and contributes new only. Empty payloads are valid lines. Prefix
bytes protect literal payload beginning with `---`, `+++`, `@@`, `+`, `-`, SP,
or `\\`; a record without one of the four body prefixes is invalid.

The no-final-newline marker has exact bytes `5c 20 4e 6f 20 6e 65 77 6c 69 6e
65 20 61 74 20 65 6e 64 20 6f 66 20 66 69 6c 65 0a`. It attaches only to the
immediately preceding body line. After a deletion it marks old only; after an
addition it marks new only. Git 2.48.1 empirically emits exactly one marker
after a final Context record when both old and new sides lack a final
terminator. That marker maps to `old_line_ending = None`,
`new_line_ending = None`, and both side-marker flags. A context marker cannot
express one-side-only absence: newline-state divergence is emitted as
Deletion plus Addition with their own markers. Marker-at-start,
duplicates, marker after an inapplicable line, marker before later applicable
same-side lines (including later hunks), and marker without a preceding line
reject. The future parser delays committing the preceding line until this
optional marker is resolved, retaining one bounded pending line only.

## Conceptual state and errors

The B2-B1 design must use equivalent logical states: `Start`, `FileHeaders`,
`ExpectOldPath`, `ExpectNewPath`, `BetweenHunks`, `InHunk`,
`AfterLineAwaitingOptionalMarker`, `Complete`, and `Rejected`. Only the
transitions implied by the grammar are legal. Counters increment when a body
line is committed; UTF-8/control validation happens before retention; EOF is
accepted only from `Complete`; any error discards all partial evidence.

Malformed headers, unsupported headers, invalid UTF-8/control, numeric
overflow, count mismatch, unexpected EOF, trailing bytes, multiple sections,
and invalid marker placement map to private `MalformedOutput` or
`InvalidEvidence` without payload diagnostics. Authoritative path disagreement
maps to `PathMismatch`; child overflow to `OutputOverflow`; budget failure to
`ResourceLimit`; timeout/cancellation/non-zero Git exit to their existing
private errors. Fresh classification/staleness/lifecycle outcomes remain
B2-C concerns and are not inferred by this grammar.

## Raw hostile-byte fixture policy

`raw-manifest.tsv` is the byte-length and SHA-256 manifest for binary fixtures
in `raw/`. They are generated only by
`scripts/generate-phase-3c-b2-b0-raw-fixtures.py` from reviewed byte literals;
the generator is fixture tooling, not a parser. Invalid UTF-8 (continuation,
truncation, overlong, surrogate, above-range, section, and path), NUL, ESC/CSI/
OSC, representative forbidden C0 bytes, all B2 policy bidi controls, and
invalid CR placements reject as `MalformedOutput` before any evidence exists.
Each raw-manifest row records byte length and first dangerous-byte offset.

Only payload `CR LF` is a valid CRLF source-line representation. CR elsewhere,
including header/path/marker CR, interior CR, a trailing bare CR, and multiple
terminal CR bytes rejects. `P13-crlf-valid.bin` is the positive byte artifact.

## Fixture corpus, provenance, and non-goals

The static corpus and SHA-256 manifest live in
`docs/fixtures/phase-3c-b2-b0/`. Fixtures marked `empirical-git-2.48.1` were
captured only from disposable repositories under `/private/tmp` on macOS using
the proposed no-external-diff/no-textconv/no-color/no-renames shape; no user or
Agent Sentinel repository was diffed. `scripts/verify-phase-3c-b2-b0-fixtures.sh`
only verifies static fixture hashes and manifest shape; it is not a parser or
production tooling.

B2-B0 does not implement parsing or extraction, execute Git in production,
prove repository coherence, perform A/B/C or E1/E2/E3, access persistence,
create a bridge, alter automatic removal, or solve production descendant
containment.

Approval requires all manifest hashes, positive and negative fixtures, exact
authoritative-header byte comparison, empirical CRLF/no-newline proof,
all bounds, and complete consumption to be reviewed cleanly. B2-B0 remains in
review; its fixture evidence is not a production protection. B2-B1 must not
start until the formal B2-B0 approval review is clean.
