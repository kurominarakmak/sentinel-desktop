# Phase 3 Claude Code Provider Validation

## Deterministic coverage

The native adapter's fake-Claude suite covers executable detection, new session
start, structured `stream-json` output, opaque session-ID persistence, owned
process cancellation, resume without duplicate sessions, malformed output,
exit handling, and Sentinel restart reconciliation. Provider events never
transition the Sentinel task to completion.

## Local provider probe: 2026-08-12

The local executable was found at `/Users/manpao/.local/bin/claude` and reported
`2.1.201 (Claude Code)`. Its `--help` confirms the supported `--print`,
`--output-format stream-json`, and `--resume` interfaces used by Sentinel.

The opt-in probe created a new temporary Git repository and used a harmless
no-file prompt. Claude emitted a valid `system/init` event with a session ID,
followed by structured `assistant` and `result` events reporting
`authentication_failed` / `Not logged in · Please run /login`. No account or
configuration change was attempted, and no Sentinel or user repository was
modified.

## Phase 3 closure

Phase 3 is **COMPLETE WITH AUTH-LIMITED VALIDATION**:

- Real Claude Code executable/version detection passed.
- Supported `stream-json` and `--resume` interfaces were confirmed.
- Real CLI JSON framing (`system/init`, `assistant`, and `result`) was observed.
- Deterministic start, stream, resume, cancel, malformed-output, exit, and
  restart-recovery tests pass.

The local CLI is not logged in, so authenticated new-session execution,
provider streamed work, same-session resume, in-flight cancellation, clean
successful lifecycle exit, and provider-backed restart reconciliation could
not be tested. No login or account modification was attempted. Authenticated
Claude smoke/reconciliation is deferred to Phase 8 or until credentials become
available; it does not block Phase 3 closure.

## Adapter hardening observed during the probe

The terminal outcome is derived only after the owned child's stdout reader has
drained. This preserves the final `result` event if it arrives immediately
before process exit and still leaves Sentinel task lifecycle authority outside
the provider stream.
