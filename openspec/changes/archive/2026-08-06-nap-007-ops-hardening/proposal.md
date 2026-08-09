# Change: nap-007-ops-hardening

## Why

A deep review of the delivered `nap-node-agent` / `nap-guest-agent` code found
defects in invariants that nap-002 and nap-003 **claim in their ratified specs**.
Two are serious enough to fix ahead of further feature work:

- **A lost `Create` race bricks an instance id.** `ops::submit` does its
  idempotency lookup, in-flight check, transition check and two inserts under
  five separate locks. When two creates race, the loser journals its operation
  and *then* fails the `instances` PRIMARY KEY, leaving an operation row `QUEUED`
  forever — and `has_inflight_op` then rejects every subsequent operation on that
  instance until the daemon restarts, because only crash recovery fails stale
  ops. The spec says operations are journaled "before any side effect begins" and
  that a replayed key returns the original operation; neither holds under
  concurrency.
- **One unresponsive guest starves TTL enforcement for every instance.** The
  reconciler iterates serially and awaits `connect` + `Health` with no timeout,
  so a single wedged channel freezes TTL expiry node-wide. `instance-lifecycle`
  promises lease-style expiry; a stalled loop silently suspends it.

The rest are smaller but of the same character — places where the code degrades
silently instead of loudly, which the constitution forbids outright.

This is a **corrective change against delivered code**, not a step in the Phase 1
feature sequence (§II), and it interleaves with `nap-005-hypeman-backend` rather
than following it.

## What Changes

**Correctness**

- `submit` becomes one transaction under one lock: idempotency lookup, conflict
  check, transition check, operation row and instance row commit together or not
  at all. A UNIQUE violation on `idempotency_key` resolves to a **replay**, which
  is what the spec already promised.
- Reusing an `idempotency_key` with a **non-matching** request is rejected as
  `INVALID_SPEC` instead of silently returning the unrelated original.
- Readiness probing is bounded by a timeout and cannot starve TTL enforcement.
- A TTL-triggered operation that fails emits a degradation event naming TTL as
  the trigger, rather than leaving a lease-less instance with an unexplained
  `FAILED`.
- The create executor **fails the operation** when the guest token cannot be read,
  instead of proceeding with an empty token and a confusing downstream failure.
- `WatchEvents` survives broadcast lag: a lagging subscriber is re-synchronised
  from its last cursor rather than having its stream silently stop.
- Crash recovery no longer records `STOPPED` when the runtime stop failed; the
  instance is marked `FAILED` with the reason, so the registry never claims a
  state it did not reach.

**Honesty of the record** (comments and docs that overstate what the code does)

- The guest token's threat model: it travels in the sandbox's environment and is
  readable at `/proc/1/environ` by any same-uid process — exactly the processes
  the channel's auth is described as defending against. The `0600` socket is what
  actually provides that defence. The comments claim more than the token buys.
- `bootstrap.rs` documents a writable tmpfs at the socket path that the `fake`
  runtime never mounts; the agent simply `create_dir_all`s, which fails on a
  read-only rootfs.
- `exec`'s module contract promises that reading to the `exit` frame has seen all
  output, while a 500 ms drain cap can truncate a large buffered tail.

**Not changing:** `ExecStart.user_activity`. The review asks for an explicit
`false` to be honoured, but proto3 gives a bare `bool` no presence, so "honour
false" means *omitting* the field stops resetting the TTL — and every ordinary
caller omits it, so active sessions would expire mid-use. That is strictly worse
than a probe occasionally extending a lease. The real fix is `optional bool` in a
later contract revision, already recorded as deferred in spec §10.

## Capabilities

### Modified Capabilities
- `node-agent-api`: operation submission becomes atomic under concurrency;
  key-reuse semantics defined; crash recovery may not claim an unreached state;
  the event stream may not silently stop.
- `instance-lifecycle`: TTL enforcement may not be starved by one instance, and a
  failed TTL action is reported.

## Impact

- No contract change: `nap.node.v1alpha1` and `nap.guest.v1alpha1` are untouched.
- Every correctness fix ships with a regression test **demonstrated to fail
  first**, since a test written after a fix mostly proves the fix compiles.
- Concurrency tests are the hard part: the existing `NAP_TEST_STEP_DELAY_MS`
  window is reused where it helps.
- Depends on: nap-002 and nap-003 (archived); interleaves with nap-005.
