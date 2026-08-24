# Change: barista-051-stamp-lease-state-at-transition

## Why

The lease's run state (`sessions/<name>` → `state: "running" | "paused"`) was
written **only on renewal**. barista-036 established that deliberately and said
so in its own delta: "Because renewal runs each reconciliation pass, a state
transition is reflected within one renewal interval." Its design went further and
rejected the alternative by name — "Thread state through `acquire`/`set_instance`
too … Rejected as machinery the billing granularity does not need: the ≤1 s
self-correction via renewal is below any metering resolution."

That reasoning was sound for the reader barista-036 had. **The reader changed.**
The field was introduced for metering, where a sub-renewal-interval error is
invisible against session-seconds and GiB-hours. It is now also read by a control
plane as a fast path to decide whether a session needs waking before work is
dispatched to it — and at that point the staleness stops being an accounting
rounding error and becomes a correctness input.

The failure this fixes was observed in production. A worker was paused; the next
request read a stamp that still said `running` because no renewal had happened
since; an exec was dispatched on the strength of it; and the node answered
`ERROR_REASON_GUEST_UNREACHABLE: instance … is Paused, not RUNNING`. Deterministic
after an API-driven pause, intermittent after the node's own idle/TTL park —
which is exactly the signature of a cache refreshed on a timer rather than on
change.

barista-cloud has patched its own `invoke` path to stop trusting the stamp, and
three other call paths there still trust it and are, by their own ratified specs,
wrong to fail this way. But a client-side patch is the wrong home for this, for
the reason the engineer who wrote it gave: **a correctness property that depends
on every caller remembering not to trust a cache is one new caller away from the
bug.** The durable fix is for the node to stop publishing a stale value.

### Why this is not simply "call `renew` more often"

Renewal is a heartbeat: it pushes `expires_ms` out, which is the statement "this
owner's reconciler is alive". A transition is not that statement — it is made
from the operation executor, which keeps running perfectly well on a node whose
bucket has gone unreachable. Renewing on transitions would let a node with a dead
reconciler hold a name alive by transitioning instances, so the fix needs a write
that changes the state and nothing else.

### What made this more than a one-line change

Every lease write the node made — `acquire`, `renew`, `set_instance`, `release` —
happened on the reconcile tick, and reconcile ticks are strictly serial. The node
therefore had exactly one lease writer *by construction*, and `fleet_phase::pass`
relied on it in writing: it snapshotted each `Held` up front and applied outcomes
afterwards, "safe because this pass is the map's only mutator".

Stamping at the transition adds a writer on the operation executor's task, which
runs concurrently with the tick. Both writes are conditional on the same ETag, so
the naive version has a losing side that is worse than the bug: a stamp that
consumed the version a renewal was fenced by would make the backend refuse that
renewal, and the node reads a refused renewal as *another node having taken the
session* — so it stops a workload it still owns. **A false fence is worse than
the staleness.** This change therefore serialises every fenced lease write on the
node, and the serialisation is the substantive part of it. The hazard is not
theoretical: with the lock removed, the concurrency test in this change fails 18
runs out of 25.

## What Changes

- A new fenced write, `barista_fleet::lease::stamp_state`, that sets the run
  state and carries every other field — `expires_ms` included — through
  unchanged.
- `fleet_phase::stamp_lease_state`, called from the operation executor on **both
  edges** of every transition: before the substrate is asked to act, and after
  the journal commits the outcome.
- `Fleet::lease_writes`, a mutex that makes the read of a `Held` version, the
  conditional write it fences, and the store of the resulting version one
  critical section. Taken by all five writers.
- `fleet_phase::pass`'s renewal loop re-reads each version inside that critical
  section instead of carrying it in from a pre-loop snapshot.
- The vanished-sandbox reconciler (`RUNNING` → `FAILED`, which does not go through
  the ops executor) stamps its own transitions.
- The ratified requirement is modified to say what a reader may now assume, and —
  more importantly — what it still may not.

## Impact

- Affected specs: `fleet-coordination` (one requirement MODIFIED).
- Affected code: `crates/barista-fleet/src/lease.rs`,
  `crates/barista-node-agent/src/{fleet.rs,fleet_phase.rs,ops.rs,reconcile.rs}`.
- Contracts: **none.** No proto changes; the lease is a bucket JSON object, not a
  contract type. `buf breaking` is unaffected.
- `docs/specs/phase1-runtime-interface.md`: **unaffected.** It says nothing about
  the lease, the bucket, or this field (its only `lease` matches are substrings of
  `release`), and this change adds no instance state and alters no transition in
  its §3.2 machine — it only reads that machine's existing edges. No
  human decision on the higher-ranked source of truth is needed.
- Cost: one extra conditional write per *observable* transition, not two per
  operation — the stamp is skipped when the value would not change, so a pause
  writes once and a resume writes once. ADR-002 §3.3 measures a CAS at ~2 ms
  same-host and ~361 ms to R2 from a laptop; the write is off the wake path and
  off every RPC's latency path, because nothing awaits the operation executor.
- barista-cloud can retire its client-side distrust of the stamp once this ships.
  That is a separate change in that repo and is not implied by this one: the node
  getting more honest does not oblige a consumer to start trusting it again on any
  particular schedule.
