# barista-036 — stamp the session's run state on the lease

## Why

The Phase 5 gateway meters usage from the S3 lease: a metering collector reads
every live lease and, for each, decides whether to accrue **session-seconds**
(the session is running) on top of the **memory-GiB-hours** it always accrues.
That decision needs one bit the lease does not carry today. `struct Lease` has
`owner / epoch / expires_ms / endpoint / instance_id` and nothing about whether
the instance behind the lease is running or paused.

So the gateway reads `None` and bills **every live lease as running** — including
a KVM-paused session that gives its memory back and stops doing work. A session
the platform paused to save the user money still bills session-seconds. This is
the node half of barista-cloud change `bar-022` (task 2); the gateway half
(metering, `bucket.live_leases()`, `/fleet` and session-detail surfacing) is
already merged and degrades to "behave as running" until the node stamps the
field — so this change is what makes the paused-billing signal real, and it is a
no-op until it lands.

## What Changes

- `struct Lease` gains an optional `state: Option<String>` (`"running"` |
  `"paused"`), serialized only when set — a lease written before this change,
  and a node that predates it reading a newer record, both stay valid.
- The reconciler's fleet phase stamps `state` on **every lease renewal** from the
  instance's real state at that heartbeat: `"paused"` when the local instance is
  paused (or the session is held without a running local instance), else
  `"running"`.

## Impact

- Spec: `fleet-coordination` gains one requirement — the lease reflects the
  session's run state.
- Code: `crates/barista-fleet/src/lease.rs` (field + `renew` stamps it);
  `crates/barista-node-agent/src/fleet_phase.rs` (renewal computes the state).
- Contract: no proto change — the lease is a bucket JSON object, not a
  `v1alpha1` message. Additive and back/forward compatible by construction.
- Consumers: the gateway begins metering paused leases correctly the moment a
  node running this code renews their leases.
