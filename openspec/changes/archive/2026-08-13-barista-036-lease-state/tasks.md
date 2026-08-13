# Tasks — barista-036 lease state

## 1. The field
- [x] 1.1 Add `state: Option<String>` to `struct Lease` (`crates/barista-fleet/src/lease.rs`), `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- [x] 1.2 Add `state: None` to every existing `Lease { .. }` literal (acquire ×2, renew, set_instance, release) and the test literals, so the crate compiles.

## 2. Stamp on renewal
- [x] 2.1 Add a `state: Option<String>` parameter to `renew` that overrides the carried value in the struct literal.
- [x] 2.2 In `fleet_phase::pass` renewal loop, look up `current.lease.instance_id` in the registry, map its state to `"running"`/`"paused"` (a pure helper), and pass it to `renew`.
- [x] 2.3 Update `renew`'s other callers/tests (lease.rs unit tests, fleet integration tests) to pass a state argument.

## 3. Tests
- [x] 3.1 Extend the round-trip test: a lease with `state = Some("paused")` survives the bucket; an unset `state` is omitted from the JSON.
- [x] 3.2 Unit-test the `InstanceState -> &str` mapping (running → "running"; paused/pausing/stopped → "paused").
- [x] 3.3 A fleet-phase test asserting a renewal of a paused instance's lease stamps `state = "paused"`, and a running instance's stamps `"running"`.

## 4. Gate
- [x] 4.1 `openspec validate barista-036-lease-state --strict` passes.
- [x] 4.2 `make check` is green.
