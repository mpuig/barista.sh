# Tasks — barista-039 revert idle_pause_s

## 1. Remove the code
- [x] 1.1 `barista-fleet/src/desired.rs`: remove `idle_pause_s` field, its `Desired::new` init, and its test.
- [x] 1.2 `barista-node-agent/src/lib.rs`: remove `Agent.last_activity_ms` and its bootstrap init.
- [x] 1.3 `barista-node-agent/src/reconcile.rs`: remove the `note_activity` stamp into `last_activity_ms` (keep the TTL-deadline reset).
- [x] 1.4 `barista-node-agent/src/fleet_phase.rs`: remove `idle_pause_due`, `enforce_idle_pause`, the desired-loop call, and the four idle tests; keep `lease_state_for` and its test (036).

## 2. Bookkeeping
- [x] 2.1 Withdraw the unarchived `openspec/changes/barista-037-idle-pause-enforcement` change directory.

## 3. Gate
- [x] 3.1 `openspec validate barista-039-revert-idle-pause-enforcement --strict` passes.
- [x] 3.2 `make check` is green.

## 4. Verify the surviving mechanism on beta
- [x] 4.1 A session with short `ttl_seconds` + `ttl_action: PAUSE` pauses when idle.
- [x] 4.2 An exec/activity resets the TTL deadline (no premature pause).
- [x] 4.3 A resumed session runs again; record the observation.
