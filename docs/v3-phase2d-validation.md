# Phase 2D Codex Capability And Smoke Validation

## Capability negotiation

The supported adapter accepts the observed legacy initialize response without a
`protocolVersion`, plus declared protocol versions `1` and `2`. It rejects
explicitly declared older (`0`) and newer (`999`) versions. Deterministic fake
fixtures cover all of those shapes.

## Real local smoke

On 2026-08-12, the opt-in smoke test used local `codex-cli 0.147.0` and a new
temporary Git repository. It initialized `codex app-server`, started a thread,
started a bounded no-file-change turn, retained normalized durable events,
resumed the thread, and shut down the owned child. It made no account-level or
reset request and did not target the Sentinel workspace or another repository.

The interrupt request was delivered after the tiny turn had already completed,
so Codex returned its normal RPC inactive-turn response. This validates the
request path but is not evidence of an in-flight real cancellation.

## Remaining Phase 2D limitations

- Real in-flight interrupt/cancel still needs a controlled long-running
  account-safe fixture or provider-supported test mode.
- Real restart/reconciliation and App Server death/recreation remain covered by
  deterministic fixtures; they need repeatable real-provider evidence before
  Phase 2 can be marked complete.
