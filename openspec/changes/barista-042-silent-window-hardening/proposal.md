## Why

The security review of 2026-08-14 left two follow-ups open — the M3 residual
(partition observability) and the M4 residual (write-stream inactivity) — and
they share one shape: a window in which the platform is doing the right thing
and saying nothing, which is the property the constitution forbids
("degradation is always explicit").

1. **A node partitioned from the coordination bucket runs unobserved past the
   point where its leases can expire.** When a lease renewal errors, the fleet
   phase keeps the session and retries — correct, and ratified ("coordination
   unavailability is non-destructive"; stopping every session on the strength
   of a blip is the failure mode the requirement names). But the only trace is
   a `warn!` per pass. Once the bucket has been continuously unreachable for
   longer than the lease TTL, every lease this node failed to renew has expired
   from the fleet's point of view, and another node may legally take over any
   of its names. From that moment two writers may exist for a single-writer
   session. Write-safety holds — fencing is the ETag, not the clock, and the
   losing node self-fences at its first contact after the partition heals —
   but the whole episode is invisible outside the node's own log: no event, no
   signal a consumer or operator watching the platform's declared surfaces can
   see.

2. **A guest `WriteFile` stream that stops making progress stays open
   forever.** `service.rs::write_file` loops `inbound.next().await` with no
   bound, so a client that opens a write and then sends nothing holds the RPC —
   and the open file handle — indefinitely, on the guest and on the host
   relaying it. This is an inactivity problem, not a size problem: bytes are
   already bounded by the sandbox's own disk budget, and ENOSPC reports the
   overrun.

Now, because the review surfaced them and both are one-enforcement-point fixes:
one degradation event at a principled threshold, one timeout at the only
unbounded wait on the guest's file surface.

## What Changes

- The Node Agent's fleet phase SHALL report, as a degradation event per held
  session, when the coordination bucket has been continuously unreachable for
  renewals for longer than the lease TTL — the exact moment takeover becomes
  possible, so the threshold is the protocol's own number, not an invented one.
  Emitted once per unreachability episode, not per pass; a successful renewal
  ends the episode so a later partition reports again. The report changes no
  behaviour: the sessions are kept, exactly as the ratified requirement
  demands.
- The guest agent SHALL bound the gap between consecutive `WriteFile` frames
  and end a stream that goes quiet with an explicit `DEADLINE_EXCEEDED`,
  releasing the RPC and the file handle. The bound is per frame gap, never
  total size or total duration — an upload that keeps sending chunks never
  meets it.
- **Roads not taken, recorded deliberately.** A K×TTL auto-self-fence (stop
  the workloads after the partition outlasts some multiple of the TTL) was
  rejected: a node cannot distinguish a global bucket outage — where no other
  node can acquire anything, takeover is impossible, and self-fencing would
  stop every session on the node for zero safety gain — from an asymmetric
  partition where it alone is cut off. The honest, non-destructive answer is
  to say what may be happening and keep running. A `WriteFile` byte cap was
  likewise rejected: it would duplicate the filesystem's own bound with an
  invented constant, and the sandbox's disk budget plus ENOSPC already answer
  the size question.
- Not breaking: no proto, no metadata key, no in-sandbox path changes. The one
  observable change on the guest is that a write stream idle beyond the bound
  fails instead of hanging forever — and a hang was never a contract anyone
  could rely on.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `fleet-coordination`: strengthen "Coordination unavailability is explicit and
  non-destructive" so the *explicit* half extends past the node's own log — a
  partition that outlasts the lease TTL is reported through the event surface,
  once per episode, per held session, while the non-destructive half stays
  exactly as ratified.
- `guest-agent`: add a liveness requirement to the file surface — a `WriteFile`
  stream that stops making progress is ended with an explicit status rather
  than holding the RPC and file handle open forever. `Exec` is deliberately
  excluded (interactive sessions are legitimately idle for long stretches; see
  design.md D4).

## Impact

- **Code**: `barista-node-agent/src/fleet.rs` (episode state on `Fleet`, the
  pure transition rule), `fleet_phase.rs` (track renewal outcomes, emit the
  degradation); `barista-guest-agent/src/service.rs` (the inactivity bound and
  the generic-stream extraction that makes it testable). One new integration
  test file (`tests/fleet_partition.rs`). No dependency changes beyond a
  test-only tokio feature (`test-util`, so the timeout test does not sleep 60
  real seconds).
- **Acceptance tests**: claims none of T1–T12 as new. Neither piece touches a
  lifecycle verb, a snapshot path, or the happy-path channel; the WriteFile
  happy path is pinned unchanged by the new tests. DoD is `make check` plus the
  targeted tests in tasks.md.
- **Contracts**: none. No `v1alpha1` proto is touched; `DEADLINE_EXCEEDED` is
  an existing gRPC status a streaming RPC may already return.

## Constitution Check

- **Schema-first**: no contract type is added or duplicated.
- **Honest capabilities / explicit degradation** (§I): the change's whole
  point, twice over — a dual-execution window becomes a named event, and a
  wedged stream becomes a named status.
- **Crash-safe by construction** (§I): the degradation rides the existing
  event journal; the episode state is deliberately in-memory only (a restart
  re-observes the partition within one pass — see design.md D2).
- **Simple by default** (§IV): each fix is the smallest that closes its window,
  and the rejected larger designs (auto-self-fence, byte cap) are named above
  with the concrete reason each is wrong, not merely bigger.
- **Human control** (§V): both behaviours were reviewed and approved as
  follow-ups from the 2026-08-14 security review; this proposal records that
  decision in the ratifiable form the workflow requires.
