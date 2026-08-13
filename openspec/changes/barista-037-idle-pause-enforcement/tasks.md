# Tasks — barista-037 idle-pause enforcement

## 1. The field
- [x] 1.1 Add `idle_pause_s: u32` (`#[serde(default)]`) to `struct Desired` (`crates/barista-fleet/src/desired.rs`); set it in `Desired::new` (`0`).
- [x] 1.2 Test: an absent `idle_pause_s` defaults to `0`; a set value round-trips; `SCHEMA_VERSION` unchanged.

## 2. The node-side idle clock
- [x] 2.1 Add `pub last_activity_ms: Mutex<HashMap<InstanceId, i64>>` to `Agent` (`crates/barista-node-agent/src/lib.rs`) and init it `Default::default()` in `bootstrap`.
- [x] 2.2 In `note_activity` (`reconcile.rs`), stamp `last_activity_ms = now` for the instance (before the `ttl_seconds == 0` early return, so the clock is independent of TTL).

## 3. Enforcement
- [x] 3.1 Add a pure `idle_pause_due(running, last_activity_ms, now_ms, window_s) -> bool` to `fleet_phase.rs`.
- [x] 3.2 Add `enforce_idle_pause(agent, window_s, instance_id)`: return early on `window_s == 0` / empty id; forget the clock when not running; seed on first sight; when due, submit `OpKind::Pause { require_memory: false }` keyed `idle_pause:{id}:{last}` and forget the clock.
- [x] 3.3 Call `enforce_idle_pause(agent, want.idle_pause_s, &held.lease.instance_id)` in the desired loop, before materialisation.

## 4. Tests
- [x] 4.1 `idle_pause_due` unit tests: `0` opts out; not-running never due; within window not due; past window due.
- [x] 4.2 `enforce_idle_pause` against a real `Agent` + a `RUNNING` stub instance: a past clock fires the pause (instance enters `PAUSING`); a fresh clock does not; `idle_pause_s = 0` does not.

## 5. Gate
- [x] 5.1 `openspec validate barista-037-idle-pause-enforcement --strict` passes.
- [x] 5.2 `make check` is green.
