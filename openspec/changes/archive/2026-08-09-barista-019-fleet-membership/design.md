# Design — fleet membership

## Decision 1: ownership is journaled, because a sandbox outlives its agent

The defect is not that leases are in a `Mutex<BTreeMap>`; it is that the map is
the *only* record. A node agent is a process; the sandboxes it created are not.
Kill the agent and the workloads keep running — that is deliberate, and it is
what makes `kill -9` recovery cheap for everything else in this system.

For the fleet it inverts: an agent that restarts holding no memory of what it
owned cannot fence anything, because fencing means "stop the workload for a
session that is no longer mine" and it no longer knows which workloads those
were. The bucket knows who owns a name *now*; only the node knows what it was
running.

So held leases join the journal, in the same SQLite database as everything else
that must survive a restart. One row per held name: the name, the epoch, the
instance realising it. Written when a lease is acquired, updated on renewal
only where the epoch changed, deleted on release or fence.

The alternative — reconstructing ownership by listing the substrate's sandboxes
and matching them against `desired/` — was rejected. It infers intent from
residue, cannot distinguish "I owned this" from "I materialised it before losing
the lease", and fails exactly when the substrate is unreachable, which is when a
node most needs to know what it must not touch.

## Decision 2: recovery reconciles before it acquires, and that order is the change

On start, after the journal's own recovery and before the first fleet pass:

1. Read the journaled leases. This is what this node believed it owned.
2. For each, read `sessions/<name>` from the bucket.
3. **Still ours at our epoch** → resume renewing it; the workload keeps running.
4. **Someone else's, or a later epoch** → we were fenced while we were dead.
   Stop the local workload, delete the row, event `FENCED`.
5. **Absent** → the name was released or never written. Stop the workload; a
   session nobody owns must not keep running on a node that cannot renew it.
6. **The bucket is unreachable** → do nothing, and do not acquire either. This
   is the ratified non-destructive rule, and it is why step 6 exists as its own
   case rather than falling into 5: "I cannot see the record" and "the record is
   gone" are opposite facts.

Acquiring before reconciling would let a node take new names while unknowingly
fenced on old ones — two single-writer violations for the price of one.

## Decision 3: the lease names its instance, and a fence without one is loud

`lease::set_instance` has existed since nap-017 with no callers, so
`sessions/<name>.instance_id` has always been empty, so `self_fence` — which
returns early when it is empty — has always returned early. The FENCED event
fired; nothing stopped. The integration test asserted the event and the lease
map, so it passed.

Three linked fixes:

- Materialisation calls `set_instance` once the instance id is known, fenced by
  the version we hold, so the record says what realises the session.
- `self_fence` treats an empty instance id as an **inconsistency to report**,
  not as nothing to do. A lease we hold with no instance and a running workload
  is a state we cannot explain, and saying so beats silence.
- The test asserts the workload reached a non-running state. A test that cannot
  fail against a fence that does nothing is not testing the fence.

## Decision 4: a fenced stop is confirmed, not fired and forgotten

Today the lease row is dropped and one `Stop` is submitted. If that submission
is refused — a concurrent operation, a substrate blip — the node has forgotten
it ever owned the session and the workload runs on. Under a takeover that is the
split-brain the layer exists to prevent, produced by the error path rather than
the happy one.

The row is therefore removed only once the instance is observed non-running.
Until then the node keeps the row, keeps retrying on later passes, and does not
reacquire. Retrying a stop is cheap and idempotent; forgetting is not.

## Decision 5: the wiring is the last commit

The daemon's bucket configuration and the `fleet_phase::pass` call on the tick
land after everything above, deliberately. At no point should `main` carry a
node that can join a fleet and violate the guarantee — which is the state this
change exists to leave behind, and would be the state it passed through if the
three lines of plumbing went first.

This also means every task before the last one is verifiable without a fleet:
the journal table, the recovery logic and the fencing all have tests that
construct a `Fleet` directly, exactly as nap-017's tests already do.
