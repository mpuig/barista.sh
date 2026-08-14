# barista-045 — renewal-lock-scope

## Why

The 2026-08-14 code-quality review found that the fleet pass's renewal loop
holds the `fleet.held` mutex across every `renew()` bucket round-trip
(`fleet_phase.rs:88–134`). The lock scope is invisible in healthy operation,
but during a bucket outage each renewal waits out the store client's failure
path while the guard is held, so the map is locked for roughly
*(held leases × per-renew wait)* per pass — and everything else that needs the
map waits behind it. The most important waiter is `Agent::fleet_info`
(`lib.rs:241`), which serves the contract's status surface: precisely when an
operator is diagnosing a partition, the node's own status query stalls. The
same file already shows the correct shape — the acquire phase and
`release_sweep` (`fleet_phase.rs:468–471`) both snapshot under a brief lock,
do bucket I/O outside it, and re-take the lock briefly to apply — so this is
the one remaining phase holding the map across the network.

A second, smaller finding from the same review rides along: `hex16` in
`node_info.rs:51` slices `bytes[..8]` and would panic on input shorter than
8 bytes. Every current caller passes a 32-byte SHA-256 digest, so this is a
totality fix, not a live bug.

## What Changes

- The renewal loop in `fleet_phase::pass` snapshots the held `(name, Held)`
  pairs under a brief lock, performs every `renew()` outside the lock, and
  re-takes the lock briefly to apply each outcome (insert on `Held`, remove on
  `Fenced`) — matching the pattern the acquire phase and `release_sweep`
  already use. Renewal semantics, fencing decisions, run-state stamping
  (barista-036), and outage-episode accounting (barista-042) are unchanged;
  only the lock scope moves.
- A deterministic regression test in the `fleet_partition.rs` style: a store
  whose operations park on a barrier; while a renewal is parked mid-pass,
  `fleet_info` must still answer.
- `hex16` becomes total (no panic on short input), behavior-identical for all
  current callers.
- **Not** in scope, deliberately: the criterion bench suite and the
  comment-policy line in `docs/best-practices.md` — the review's two optional
  items — and any change to renewal ordering or concurrency (renewals stay
  serial; parallelising them is a different trade-off nobody has asked for).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `fleet-coordination`: one **added** requirement — a coordination-backend
  wait SHALL NOT block the node's own status surface. This is a genuine
  liveness property the regression test enforces, not a restatement of the
  implementation. It is written as an added requirement rather than a
  modification of "Coordination unavailability is explicit and
  non-destructive" because barista-042's delta to that requirement is merged
  but not yet synced into the main spec, and two unsynced MODIFIED deltas to
  the same requirement would clobber each other at sync time.

## Impact

- `crates/barista-node-agent/src/fleet_phase.rs` — renewal loop lock scope.
- `crates/barista-node-agent/src/node_info.rs` — `hex16` totality.
- `crates/barista-node-agent/tests/` — one new integration test (new file or
  a case in `fleet_partition.rs`, decided in design).
- No proto, CLI, guest, or runtime changes. No behavior change on any healthy
  path; on the outage path the only observable difference is that status
  queries answer promptly.

## Acceptance tests claimed (DoD)

None of T1–T12: this change touches no lifecycle behavior those tests cover.
Its definition of done is `make check` plus the new liveness regression test,
which becomes part of the standard suite.

## Constitution check

- **Schema-first**: no contract types touched; `FleetInfo` is produced from
  the same data as before.
- **Adopt the substrate, own the session layer**: no substrate interaction
  changes; the bucket protocol (acquire/renew/release, fencing) is untouched.
- **Honest capabilities**: unchanged — outage handling still reports
  `backend_unavailable` and the barista-042 episode events exactly as before.
- **Crash-safe by construction**: no journaled operation is added, moved, or
  reordered; the pass remains the single mutator of the in-memory lease map.
- **Simple by default (§IV)**: the fix copies an existing in-file pattern
  rather than introducing new machinery; the simpler alternative — leaving the
  lock held — is exactly the defect.
