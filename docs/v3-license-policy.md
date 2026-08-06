# Sentinel V3 License Policy

## Allowed by default

Apache-2.0, MIT, BSD-2-Clause, BSD-3-Clause, ISC, and Zlib code may be considered subject to source-specific verification, notices, dependency security review, and compatibility with the repository's Apache-2.0 license. Dual-licensed work may be used only by selecting and documenting a compatible permissive option.

## Requires explicit review or rejection

Reject by default: unknown/custom licenses; source without a license; non-commercial/no-derivatives/research-only terms; proprietary source code; AGPL, GPL, LGPL, MPL, EPL, and other reciprocal/copyleft licenses; patent or field-of-use restrictions; and licenses whose generated artifacts create unreviewed obligations. An exception requires a written maintainer/legal decision before code enters the repository. The policy is intentionally conservative; no claim of legal advice is made.

## Interface versus source-code use

Calling a documented provider CLI, App Server, SDK, or protocol does not authorize copying its implementation. Codex and Claude Code integrations must use supported interfaces, must preserve their provider-owned authentication, and must not embed, scrape, decompile, or redistribute provider implementation code. Official documentation is a compatibility reference and must be recorded by URL/version/date when it materially shapes an adapter.

## Required provenance record

Every adopted dependency, copied snippet, ported module, generated asset, or vendored source requires an entry in `THIRD_PARTY_NOTICES.md` containing source URL, immutable revision/version, license/SPDX identifier, notice/copyright, use classification, affected Sentinel files, modifications, and attribution placement. Package-manager lockfiles do not replace this record. Review must also establish that licenses of transitive runtime and build dependencies are acceptable.

## Enforcement

Phase 8 adds automated dependency/license inventory plus a CI check requiring complete notices for vendored/ported code. Pull requests adding external material must include the provenance update and must be rejected when attribution, license clarity, or isolation is incomplete. Do not remove upstream notices. If a later audit discovers an incompatible source, stop distribution of the affected artifact, isolate the dependency, and replace or remove it through a dedicated remediation change.
