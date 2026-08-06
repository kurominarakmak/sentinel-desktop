# Sentinel V3 Third-Party License Plan

## Audit result

All 15 cloned repositories contain an explicit permissive root license. This establishes only eligibility for further review; no audited repository is imported by this research phase.

| Repository | Inspected HEAD | Exact root license | Intended status |
| --- | --- | --- | --- |
| `coollabsio/jean` | `7c00b90` | Apache License 2.0 | reference only |
| `AnyiWang/OpenCovibe` | `a8f8fdd` | Apache License 2.0 | reference only |
| `octavi42/threadline` | `28e089b` | MIT License | reference only |
| `st0012/cctop` | `0f7baee` | MIT License | selected concept only |
| `johannesjo/parallel-code` | `d000fff` | MIT License | reference only |
| `alamops/agetor` | `7d28f2a` | MIT License | reject |
| `sstraus/tuicommander` | `9d521ad` | Apache License 2.0 | reject |
| `mvmcode/elves` | `ec9b4c2` | MIT License | reference only |
| `openai/codex-plugin-cc` | `db52e28` | Apache License 2.0 | selected protocol reference |
| `boyand/codex-review` | `416204c` | MIT License | reference only |
| `DanMcInerney/architect-loop` | `164d32c` | MIT License | reference only |
| `kenryu42/ralph-review` | `1f9b8b9` | MIT License | selected concept only |
| `standardagents/dmux` | `ce045aa` | MIT License | reject |
| `jazzyalex/agent-sessions` | `fdffdc4` | MIT License | reject |
| `agentclientprotocol/agent-client-protocol` | `e7846aa` | Apache License 2.0 | direct-dependency candidate |

## Required gate before any adoption

1. Record the immutable URL and full revision, exact source files, SPDX identifier/license-file path, copyright/notice text, and intended reuse class in `THIRD_PARTY_NOTICES.md`.
2. Confirm that only the cited isolated module is used; do not copy an app, UI, provider binary, prompt library, or workflow wholesale.
3. Review direct and transitive dependency licenses, security posture, maintenance activity, and API/version stability. Pin every dependency revision.
4. Reimplement ported concepts in Sentinel Rust unless a later source-specific decision explicitly authorizes copying. Preserve MIT/Apache notices when code, rather than a concept, is copied.
5. Add provenance and behavior tests in the same change. A missing notice, unclear scope, incompatible license, unstable required protocol, or failed capability probe rejects the source.

## Provider interfaces

Codex App Server and Claude Code CLI/session interfaces are not open-source code donors merely because a repository integrates them. Sentinel will call only supported interfaces, retain provider-owned authentication, and never copy, redistribute, scrape, or reverse-engineer provider implementation. `codex-plugin-cc` is a protocol/type reference; Claude Code requires a locally authored adapter with current supported-interface validation.

## Attribution plan

The current audit itself needs no shipped third-party notice because it imports no code. If Sentinel later adopts ACP as a dependency, ported cctop preflight logic, ralph-review artifact fields/tests, or Codex plugin types, the implementation commit must add the required `THIRD_PARTY_NOTICES.md` entry and preserve the applicable Apache-2.0 or MIT attribution before merge.
