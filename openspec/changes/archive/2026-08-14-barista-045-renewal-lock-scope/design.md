# barista-045 — design

## Context

See proposal.md — Why. The one design-relevant fact: within
`fleet_phase::pass`, the renewal loop (phase 1) is the only place left that
holds the `fleet.held` guard across bucket I/O. The acquire phase
(`fleet_phase.rs:227`, `:258`) and `release_sweep` (`:468–471`, `:494–498`,
`:567`) already snapshot under a brief lock, do the round-trip outside, and
re-take the lock briefly to apply. `pass` is the single mutator of the map —
it is called only from the reconcile tick (`reconcile.rs:118`), one task,
ticks strictly serial — and the only other lock site in the daemon is the
read in `Agent::fleet_info` (`lib.rs:241`).

## Goals / Non-Goals

Goals: the two spec scenarios (status answers while the backend stalls; the
answer is a snapshot of whole outcomes), with zero change to renewal,
fencing, run-state stamping, or outage-episode semantics.

Non-goals: concurrent renewals; any change to `barista-fleet`'s
acquire/renew/release protocol; any change to how long an outage pass takes
(the defect was *who waits*, not how long the pass runs).

## Decisions

1. **Snapshot–renew–apply, copying the in-file precedent.** Under a brief
   lock, collect the `(name, Held)` pairs (this also decides
   `renewals_attempted`); drop the guard; for each pair, read
   `lease_state_for` and call `renew()` with no lock held; re-take the lock
   briefly to apply the outcome. This is `release_sweep`'s exact shape, so
   the file ends up with one lock discipline instead of two.
   *Alternative considered:* guard the apply step with an epoch re-check in
   case the entry changed mid-renewal. Rejected (Constitution IV): `pass` is
   the single mutator and ticks are serial, so nothing can touch the entry
   between snapshot and apply — the check would be dead code implying a
   concurrency that does not exist. The invariant is stated in a comment at
   the snapshot site instead, so a future second mutator has to read it.

2. **Renewals stay serial.** `join_all` over renewals would shorten an
   outage pass, but it changes the backend load pattern and outcome ordering
   for a problem nobody has — once the lock is dropped, pass duration harms
   no one but the pass. The simpler serial loop stays.

3. **Apply per outcome, not batched at the end.** Re-taking the lock once
   per outcome (as `release_sweep` does) keeps each applied result visible
   to `fleet_info` immediately and directly realises the spec's
   whole-outcomes-only snapshot scenario. A batched apply would be one lock
   cheaper and strictly staler; not worth it.

4. **Test: a new integration test file, `fleet_status_liveness.rs`,** rather
   than a case in `fleet_partition.rs` — that file is about partition
   *semantics* (what a cut decides); this one is about *liveness* (what a
   stall may not block). A `StallableStore` wraps `InMemory` the way
   `PartitionableStore` does, except ops park on a `tokio::sync::Notify`
   when armed and the store signals when an op is parked. Sequence: acquire
   a lease unstalled → arm the stall → spawn `pass` → await the store's
   "parked" signal (no sleeps, deterministic) → assert `fleet_info` answers
   within a short timeout and reports the held lease → release the barrier →
   assert the pass completes and the lease was renewed. Before the fix this
   test deadlocks the status call for as long as the stall; after it, it
   passes.

5. **`hex16` totality**: `bytes.iter().take(8)` replaces `bytes[..8]` —
   total, and byte-identical output for every input of 8 bytes or more
   (all current callers pass 32-byte SHA-256 digests; spec §10.3 context in
   `node_info.rs`). No test beyond the compiler: the change is
   an expression swap with no observable behavior to pin.

## Risks / Trade-offs

- [Status can now be read mid-pass between outcomes] → Not new exposure:
  `fleet_info` always raced the pass at outcome granularity; decision 3
  keeps reads whole-outcome coherent, and fencing stops still happen after
  the loop via `fenced_names` exactly as today.
- [A future second mutator of `fleet.held` would silently invalidate the
  race-free apply] → The invariant comment at the snapshot site names the
  single-mutator assumption; any change adding a mutator must confront it.
- [Stall-test flakiness] → The store signals the parked state; the test
  never infers "stalled" from elapsed time.

## Migration Plan

None. In-memory lock scope only — no wire, journal, or bucket format
change; deploys as an ordinary binary roll, no ordering constraints.
