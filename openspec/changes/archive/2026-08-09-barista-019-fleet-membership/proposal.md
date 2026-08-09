# Change: barista-019-fleet-membership

## Why

nap-017 built the coordination protocol and left it unreachable. A repository
review found the gap and was right about all of it: `Agent::bootstrap` sets
`fleet: None` unconditionally, the daemon takes no bucket configuration, and
`reconcile::tick` never calls `fleet_phase::pass`. `BARISTA_FLEET_BUCKET`
configures the CLI and nothing else, so no node has ever acquired, renewed or
materialised a session.

Wiring it is three lines of plumbing. **Shipping it safely is not**, and that is
why this is a change rather than a patch.

The same review found that the fleet, if switched on today, could not keep the
promise the whole layer exists for. Held leases live only in a
`Mutex<BTreeMap>` on the `Fleet` struct. A node agent that is killed and
restarted therefore has:

- surviving workloads, because a sandbox outlives its agent by design; and
- no record whatsoever connecting those workloads to the leases it used to hold.

If another node took a name during the gap, the restarted agent will never fence
the old workload, because it does not know it owned it. Two live writers for one
single-writer session, indefinitely — the exact condition the ratified
requirement "at most one running workload beyond a renewal interval" forbids.

Three smaller defects compound it, all with the same root: ownership is not
durable.

1. A lease is created with an empty `instance_id`, and `lease::set_instance`
   — the function that would fill it in — has no callers. So the record that is
   supposed to say which workload realises a session usually does not.
2. `self_fence` returns immediately when the instance id is empty, which after
   (1) is most of the time. The FENCED event is emitted; nothing is stopped.
3. The fencing integration test asserts the event and the lease map, and never
   asserts that the fenced workload actually reached a non-running state. It
   passes against a fence that does nothing, which is how (2) survived review by
   its own author.

## What Changes

- **A node joins a fleet from configuration**: bucket URL and advertise address
  on the daemon, `Fleet` constructed when present, fleet phase on the tick. The
  absence of configuration stays laptop mode by construction (nap-017 design
  decision 6) — this change must not make "no bucket" a code path.
- **Ownership becomes durable.** Leases held by this node are journaled, so a
  restarted agent knows what it believed it owned before it can act on anything
  else. Recovery reconciles that record against the bucket *before* the first
  acquire: a name we held and no longer own is a workload to stop, not a
  surprise to discover later.
- **`instance_id` is written into the lease** when a session is materialised,
  via the existing `set_instance`, so fencing has something to act on.
- **`self_fence` refuses to be a no-op.** An empty instance id becomes a loud
  inconsistency rather than an early return, and the stop is confirmed rather
  than fired once and forgotten.
- **The fencing test asserts the workload stopped**, not merely that an event
  was emitted.

## Capabilities

### Modified Capabilities
- `fleet-coordination`: ownership survives a restart, and the single-writer
  obligation is stated in terms a crashed node can satisfy.
- `node-agent-api`: the daemon's fleet configuration surface.

## Impact

- `crates/barista-node-agent`: `main` (configuration), `lib` (construct the
  fleet, recover ownership), `db` (a table for held leases), `reconcile` (the
  phase on the tick), `fleet_phase` (recovery, set_instance, honest fencing).
- `crates/barista-fleet`: no protocol change expected; `set_instance` finally
  gets its caller.
- Docs: `concepts/fleet-coordination.md` loses the status banner this change
  exists to remove.

## Constitution Check

- **Crash-safe by construction**: this is the whole change. Ownership joins the
  journaled state, and recovery has a defined first move.
- **Honest capabilities**: a fence that emits an event and stops nothing is the
  silent-degradation failure in its purest form; it is what this fixes.
- **Simple by default**: the simpler option — wire the fleet up and document the
  restart hazard — was rejected. A documented single-writer violation is not a
  degraded mode, it is the guarantee not holding.

## Acceptance

Claims no Phase 1 numbered test. DoD: a node restarted while another node has
taken its session stops the orphaned workload without operator action, proven by
an integration test that kills and restarts an agent rather than dropping a
`Fleet`; the existing suite still passes bucketless; `make check` green **with a
live substrate**, since a dead-port run hides exactly the tests that matter.
