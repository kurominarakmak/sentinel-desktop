# Sentinel V3 OSS Integration Plan

## Policy and approach

V3 may wrap, port, or adapt narrowly scoped permissively licensed modules after a source-specific review. It will not clone, fork wholesale, or copy another desktop product. Existing Tauri, Rust, React, and Git foundations remain in place. Claude Code is integrated only through Anthropic-supported interfaces; its implementation is not copied.

## Initial research register

| Candidate/source | License observed | Intended use | Decision for this phase | Required attribution if adopted |
| --- | --- | --- | --- | --- |
| [Tauri](https://github.com/tauri-apps/tauri) | MIT or Apache-2.0 | Existing desktop/tray/window foundation | Retain dependency; no source copy | Dependency notice/version and upstream license notices |
| [Tauri official plugin workspace](https://github.com/tauri-apps/plugins-workspace) | Apache-2.0 reported by upstream organization | Evaluate isolated window-state/notification helpers only | Candidate; do not import yet | Exact crate/version, license text, modifications |
| [Claude Code CLI reference](https://docs.anthropic.com/en/docs/claude-code/cli-usage) | Documentation/service terms; not an OSS code grant | Supported CLI/session protocol integration | Approved interface, not reusable source | Link/version/capability evidence; no code attribution claim |
| Codex supported interface documentation | Provider interface, not an OSS code grant | Native protocol adapter | Interface-only candidate pending current compatibility spike | Provider docs/version/capability evidence |
| Any full desktop app or unclear/copyleft repository | unclear, reciprocal, or incompatible | broad feature reuse | Rejected | None; do not copy |

The Tauri upstream publishes MIT/Apache licensing, and Anthropic documents structured CLI output plus explicit session-resume options. These facts justify evaluation of their interfaces, not copying either application. [Tauri source](https://github.com/tauri-apps/tauri), [Claude CLI reference](https://docs.anthropic.com/en/docs/claude-code/cli-usage).

## Adoption gate

Before adding any external code, the implementer must create `THIRD_PARTY_NOTICES.md` (or update it) and a provenance entry with: exact URL and immutable revision/version; license file location and SPDX identifier; dependency versus copied/ported classification; files/functions used; modifications; copyright/notice text; build/runtime impact; security review; and reviewer approval. A license scanner and human review must both pass. If a source has mixed, missing, custom, non-commercial, copyleft, or uncertain terms, reject it unless a later explicit legal decision approves it.

## Planned integration sequence

1. Use official Tauri APIs/dependencies already compatible with the foundation; prefer dependency declaration to copied snippets.
2. Implement Codex and Claude adapters against supported protocols with locally authored normalization and fixture tests.
3. Evaluate isolated permissive modules only for a demonstrated gap; vendor the smallest feasible unit, retain upstream notices, and test it behind a Sentinel-owned interface.
4. Keep ACP, OpenCode, Hermes, and workflow packs outside core until their licenses, protocol stability, maintenance, and security posture pass this same gate.

No external repository will be cloned, copied, vendored, or incorporated during V3 planning.
