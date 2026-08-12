## 1. Runtime capability + reconcile state

- [x] 1.1 Added `Runtime::enumerates_sandboxes(&self) -> bool` (default `false`),
  `true` in hypeman. `StubRuntime` gets a **configurable** `enumerates_sandboxes`
  field (default `false`) so the pass only fires for tests that opt in — otherwise
  every stub test with a `RUNNING` instance and no configured `sandboxes` would
  fail it. `fake` keeps the default.
- [x] 1.2 Added `Agent::vanished_sandbox_counts:
  Mutex<HashMap<InstanceId, u32>>`, beside `credential_sweep`.

## 2. The reconcile pass

- [x] 2.1 Added `reconcile_vanished_sandboxes(agent)` in `reconcile.rs`. Returns
  early unless `enumerates_sandboxes()`. Builds the present-sandbox instance-id set,
  and for each `RUNNING` journal instance: present → `counts.remove` (reset); absent
  → increment; at `VANISHED_SANDBOX_THRESHOLD` (=3) → `set_instance_state(FAILED)` +
  `state_changed(FAILED)` + a `degradation` naming the vanish, then drop the count.
  Counts pruned to the current `RUNNING` set each pass.
  *(Kept its own `list_sandboxes` call rather than threading `sweep_instances`'s Vec
  — it avoids changing `sweep_instances`'s signature + barista-034's tests, and a
  second cheap local list per tick is negligible. Each pass independently gates on
  its own successful enumeration, which is what the spec requires.)*
- [x] 2.2 Acts only on a **successful** enumeration: a `list_sandboxes` or registry
  error is a no-op with counts untouched (an error is not an empty inventory).
  Wired into the tick after `sweep_instances`.

## 3. Tests (via `StubRuntime`)

- [x] 3.1 `a_running_instance_whose_sandbox_vanished_is_failed_after_the_debounce`:
  a `RUNNING` instance absent from the sandbox set stays `RUNNING` for `K-1` passes,
  then becomes `FAILED` with a degradation naming it; a present sandbox on an
  intervening pass resets the count (no premature fail).
- [x] 3.2 `a_present_sandbox_and_a_non_enumerating_runtime_fail_no_one`: a
  `RUNNING` instance whose sandbox is present is never failed; and with
  `enumerates_sandboxes() == false` (or an enumeration error), no instance is
  reconciled regardless of the journal.

## 4. Verification

- [x] 4.1 clippy -D warnings, fmt --check, node lib (170/0) — barista-034 sweep +
  credential-sweep tests still green.
- [x] 4.2 T5 (`t5_crash.rs`) passes (2/0).
- [x] 4.3 No regression to barista-034's `sweep_instances` tests (the two passes
  share the inventory and the tick).
- [x] 4.4 Verified live on the beta node (deployed from merged main): created a
  session (`RUNNING`, 1 sandbox), deleted its sandbox out-of-band (`DELETE
  /instances/{name}` → 204); still `RUNNING` immediately after, then `FAILED`
  after ~7s (the K=3 debounce), exactly as designed — no longer a phantom.
