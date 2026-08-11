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

## Provider-backed interrupt attempt

On 2026-08-12, the opt-in `real_local_app_server_interrupts_an_in_flight_turn`
test used the same authenticated local CLI in a newly initialized temporary Git
repository. It requested the harmless shell command `sleep 30`, did not modify
the repository, and polled Sentinel's durable event stream while the supported
`start_turn` call was in progress. The provider completed the turn in under a
second on two attempts, before Sentinel observed a `turn_started` event or had
an interruptible turn identifier. A following `turn/interrupt` is therefore
correctly rejected as an inactive-turn RPC error.

The supported public adapter currently waits for the terminal turn notification
before returning `CodexTurn`; it has no start-only handle that would let the
supervisor issue `turn/interrupt` while that call remains in flight. The real
provider did not honour the prompt's requested wait/sleep as a stable timing
fixture. Deterministic interruption fixtures remain the authoritative coverage
for cancellation persistence.

## Provider-backed owned-server death attempt

On 2026-08-12, the opt-in
`real_local_app_server_recovers_after_owned_child_death` test started a real
thread, persisted its Sentinel session metadata, completed a harmless
no-file-change turn, and sent `TERM` only to the PID returned by that test's
owned `CodexAppServer`. Sentinel detected the child exit within one second and
started a fresh App Server. The fresh provider rejected `thread/resume` with an
RPC error for that durable thread. This was reproduced after the thread had a
completed turn; it is not merely a thread-without-history case.

Clean shutdown followed by a new App Server had previously resumed the same
thread successfully. Forced process death consequently has no repeatable
real-provider reconciliation guarantee with `codex-cli 0.147.0`, even though
the Sentinel durable state contains one session and the deterministic
death/recreation fixtures pass. The adapter currently preserves the provider
error as `CodexError::RpcError`, so this validation cannot distinguish a
provider-local recovery restriction from a more detailed provider cause.

## Phase 2 closure

Phase 2 is **COMPLETE WITH KNOWN LIMITATIONS**. The completed validation
evidence is:

- Real App Server initialize, thread start, turn start, normalized durable
  streaming, and clean-shutdown thread resume passed in a disposable temporary
  Git repository.
- Deterministic in-flight `turn/interrupt` coverage passed and persists the
  normalized cancellation outcome.
- Abrupt owned-App-Server exit is detected, the owned process is recreated,
  and deterministic crash/recovery coverage passes without duplicate durable
  sessions or event sequences.

## Known provider limitations deferred to Phase 8

- A real in-flight interrupt could not be observed: real turns completed before
  interruption, despite harmless wait/sleep prompts. The deterministic fixture
  remains the reliable cancellation proof.
- After abrupt owned-App-Server death, the real provider currently returns
  `RpcError` to `thread/resume`, although clean-shutdown resume passes. This is
  not a Phase 2 completion blocker.

Phase 8 recovery hardening should inspect persisted provider and Sentinel state
with supported `thread/read`, `thread/list`, and status surfaces before deciding
whether to resume the durable thread, mark it recoverable, or start a fresh
thread. It must keep Sentinel's durable workflow state authoritative and avoid
creating duplicate sessions or events.
