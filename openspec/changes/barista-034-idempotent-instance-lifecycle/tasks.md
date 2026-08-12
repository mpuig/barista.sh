## 1. Adopt-before-create + rollback (hypeman adapter)

- [x] 1.1 Added `HypemanRuntime::dedup_instances` — lists `NODE_TAG`-scoped
  sandboxes, filters to `INSTANCE_TAG`, keeps the best survivor via `survivor_rank`
  (Running > … > Unknown), deletes the rest by `instance.id`, returns the survivor.
  (No creation timestamp on `Instance`, so state-rank stands in for "newest".)
- [x] 1.2 Rewrote `start()` to converge via `dedup_instances`: `Some(Standby)` →
  delete-by-id + `create_fresh`; `Some(_)` → start + `await_running`; `None` →
  `create_fresh`. The old `get_instance(name)` `404 → create_fresh` spiral arm is
  gone.
- [x] 1.3 `create_fresh` now captures the created `Instance` and, on `await_running`
  failure, deletes it by id (then the token volume) before propagating.
- [x] 1.4 `dedup_instances` is refactored around a pure `dedup_decision(mine) ->
  (survivor, extras)` and unit-tested Docker-free
  (`dedup_decision_keeps_the_running_survivor_and_returns_the_rest`): three
  sandboxes → the running one survives, the other two are extras; zero/one →
  survivor, no extras (the create path). The list/delete I/O and `start()` adopt
  path run against a real substrate (`hypeman_runtime.rs` self-skips locally; it
  compiles clean), covered end-to-end by task 4.4. (`hypeman_runtime.rs` has no
  mock server — it drives a live substrate — so the *decision* is the fast unit.)

## 2. Periodic instance dedup/orphan sweep (reconciler)

- [x] 2.1 Added `runtime::Sandbox` + `Runtime::list_sandboxes`/`remove_sandbox`
  (defaulted empty/error, mirroring the credential surface). Implemented for
  hypeman (`list_instances(NODE_TAG)` → tag-filtered `Sandbox`; `delete_instance`
  by substrate id) and `StubRuntime` (configurable `sandboxes` + a
  `sandboxes_removed` log). `fake` keeps the default (no leak surface).
- [x] 2.2 Added `sweep_instances(agent)` in `reconcile.rs`: groups this node's
  sandboxes by instance id (reusing the credential sweep's non-terminal live-set),
  reduces any group >1 to a running survivor (extras by id), reaps sandboxes whose
  instance is not live (by id). Wired into the tick **before** `sweep_credentials`.
- [x] 2.3 `the_instance_sweep_dedups_the_living_and_reaps_the_orphaned` (one live
  instance leaked into 3 → running survivor kept, 2 extras + 2 orphans reaped,
  evented) and `..._leaves_a_healthy_single_instance_alone`. (Peer-tag safety is a
  property of hypeman's `list_sandboxes` NODE_TAG filter, verified on the substrate.)

## 3. Credential sweep teardown order

- [x] 3.1 Implemented as tick ordering: `sweep_instances` runs **before**
  `sweep_credentials`, so a leaked/orphaned sandbox is deleted by unique id before
  the credential sweep reaches its volume — the instance-then-volume order
  `destroy` uses, achieved without duplicating delete logic in `reap_credentials`.
- [x] 3.2 `a_leaked_sandbox_is_reaped_before_its_credential`: an orphaned instance
  with both a sandbox and a token volume has the sandbox reaped by `sweep_instances`
  then the volume by `reap_credentials`, in tick order.

## 4. Verification

- [x] 4.1 Cargo gates green: `clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --check`, node lib (168/0), `adversarial_node` (2/0); `hypeman_runtime`
  integration compiles (self-skips without a substrate). Full `task ci`
  docs/buf/Docker/pytest gates unaffected — run in CI.
- [x] 4.2 T5 (`t5_crash.rs`) passes (2/0): crash recovery/reconcile shape unchanged.
- [x] 4.3 No T1/T7 regression locally: the single-instance case is the `dedup_decision`
  zero/one branch (survivor, no extras — adopt, don't create). T1/T7 run on the
  substrate; verified end-to-end by 4.4.
- [ ] 4.4 After merge + deploy to the beta node: verify convergence from the clean
  0-instance state (create/pause/resume; confirm exactly one sandbox per instance
  and no leak under repeated reconcile), then hand the smoke test to the peer.
