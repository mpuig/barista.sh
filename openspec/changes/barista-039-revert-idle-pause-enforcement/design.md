# Design — barista-039 revert idle_pause_s

## What is removed, and what stays

Removed (all of barista-037's node-side additions):

| Location | Removed |
| --- | --- |
| `barista-fleet/src/desired.rs` | `Desired.idle_pause_s` field + its `Desired::new` init + its test |
| `barista-node-agent/src/lib.rs` | `Agent.last_activity_ms` field + bootstrap init |
| `barista-node-agent/src/reconcile.rs` | the `note_activity` stamp into `last_activity_ms` |
| `barista-node-agent/src/fleet_phase.rs` | `idle_pause_due`, `enforce_idle_pause`, the desired-loop call, and the four idle tests |

Kept, untouched:

- **barista-036** — `Lease.state` and `lease_state_for`. Independent of 037, and
  its `lease_state_for_maps_the_registry_row` test stays.
- **The TTL mechanism** — `ttl_seconds`, `ttl_action`, `enforce_ttl`,
  `resolve_ttl_action`, and `note_activity`'s TTL-deadline reset. This is the
  surviving, already-shipped idle→pause path. `note_activity` keeps resetting the
  TTL deadline; only the extra `last_activity_ms` stamp is removed.

## Why the removal is safe

`enforce_idle_pause` was the sole reader of both `Desired.idle_pause_s` and
`Agent.last_activity_ms`, so removing it leaves no dangling reference. The fleet
crate's other `last_activity_ms` (in `barista-guest-agent`) is unrelated — it is
the guest's own activity timestamp reported over Health, and it predates
barista-037. `enforce_ttl` and `note_activity`'s TTL path are not touched, so the
node's idle→pause behaviour is unchanged by this revert.

## Verification of the surviving mechanism

The point of the revert is that TTL already does the job — so this change is not
done until that is seen to hold on the path production uses (fleet/bucket-
materialised instances), not merely assumed:

- create a session with a short `ttl_seconds` and `ttl_action: PAUSE`;
- leave it idle past the window and confirm it pauses (with the degradation event
  if the runtime cannot keep memory);
- exec into it and confirm the activity resets the deadline (it does not pause on
  the original schedule);
- confirm a resumed session runs again.

Recorded in tasks; `make check` remains the gate for the code removal itself.
