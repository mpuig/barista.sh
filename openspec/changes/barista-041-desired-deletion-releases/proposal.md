# Change: barista-041-desired-deletion-releases

## Why

Deleting `desired/<name>` does nothing today. The fleet phase renews every
lease it holds and scans `desired/` only to *acquire*, so a name whose
desired record is gone keeps its lease renewed forever and its workload (or
its corpse) in place. Live evidence (control plane, 2026-08-13, bar-027 task
7): the name `counter` on the beta fleet is wedged — its desired record was
deleted, its lease still points at a dead instance
(`01KZXY6MQ6V9S10S6DD2RG6ZFZ`), and after >120 s nothing had released it; the
demo had to ship under a different name. Creation is a bucket write that some
node converges on; deletion must be the same shape, or the fleet's names are
consumed permanently.

## What Changes

- The fleet pass grows a **release sweep**: after a successful `desired/`
  listing, a lease this node holds for a name that listing does not contain
  is torn down — the local instance destroyed through the ordinary journaled
  ops path, converging one operation per pass — and the lease released
  (fenced, expiry-zeroed, never deleted) only once the teardown is
  **observed** complete.
- The name set used by the sweep comes from the listing's keys, so a desired
  record that exists but cannot be parsed still counts as desired — an
  unreadable record must not become a destroyed session.
- The sweep also covers the restart shape: a lease journaled by this node
  that the bucket still shows as ours but no longer appears in `desired/` is
  re-acquired (a renewal for a live own lease) and then torn down the same
  way — otherwise `recover`'s "the next pass re-acquires it" is false for
  exactly these names and the workload runs unowned forever.
- A failed or unreachable listing releases and destroys **nothing** — the
  ratified "coordination unavailability is non-destructive" requirement
  already forbids it, and the pass already returns early on that path.

## Capabilities

### Modified Capabilities

- `fleet-coordination`: new requirement — deleting the desired record
  releases the name: owner-side teardown before release, fenced release,
  nothing destroyed on the strength of an unreadable record or an outage.

## Impact

- `crates/barista-node-agent`: `fleet.rs` (`desired()` returns the listing's
  name set alongside the parsed records), `fleet_phase.rs` (the sweep + its
  pure decision function; `PassReport` gains `released`), tests
  (`fleet_release.rs` integration on an in-memory conditional-write store;
  decision-table unit tests).
- `crates/barista-fleet`: none — `release` already exists and is fenced.
- Un-wedges the live case: the beta node's first pass after upgrade finds
  `counter` held-but-undesired and converges it to released. A wedge whose
  owner is dead needs nothing new — with no renewals the lease expires by
  TTL; the operator's last resort for a lease with a live, unupgraded owner
  is deleting `sessions/<name>` from the bucket by hand.

## Constitution Check

- **Crash-safe by construction**: teardown is ordinary journaled `Destroy`
  ops with `(name, epoch)`-derived idempotency keys; a crash mid-sweep
  replays into the same operations, and the lease is only forgotten after
  the journal shows the instance gone — the fence-and-confirm rule, applied
  to release.
- **Honest capabilities / non-destructive outages**: absence is only acted
  on when the bucket answered; an unreadable record is presence, not
  absence.
- **Single-writer**: release is a conditional write fenced by the held
  version — a superseded owner's release is refused by the backend, so a
  new owner's lease can never be clobbered (the invariant the lease module
  already states; the sweep adds no unfenced write).
- **Simple by default**: no new bucket objects, no tombstones; deletion *is*
  the signal, and the sweep is one decision function plus existing verbs.

## Acceptance

Claims no Phase 1 acceptance test (T1–T12). Definition of done: `make check`
green; the integration test proves delete-desired ⇒ instance destroyed,
lease released (expired record, epoch intact), journal row dropped, and the
name immediately re-acquirable; the decision unit tests pin the unreadable-
record and fencing-row exclusions.
