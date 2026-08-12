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
- [ ] 1.4 Tests (hypeman adapter, no live substrate): adopt an existing sandbox
  (no second create); two same-tagged sandboxes reduce to one, deleting the extra
  by id; a `Standby` survivor rebuilds; `await_running` failure deletes the created
  sandbox. Drive via a mockable client / injected instance list.

## 2. Periodic instance dedup/orphan sweep (reconciler)

- [ ] 2.1 Add the runtime surface the sweep needs, mirroring the credential sweep's
  `list_credentials`/`remove_credential`: enumerate this node's sandboxes with
  their substrate id, instance-id tag, and state, and delete a sandbox by substrate
  id. Implement for hypeman (via `list_instances`/`delete_instance`-by-id), `fake`,
  and `StubRuntime`.
- [ ] 2.2 Add `sweep_instances(agent)` in `reconcile.rs` beside `sweep_credentials`:
  group this node's sandboxes by instance-id; reduce any group with >1 to one
  (delete extras by id); delete any sandbox whose instance is not **live** in the
  journal (by id), reusing the credential sweep's live-set so a transitional
  (mid-create) instance is never reaped. Wire it into the tick.
- [ ] 2.3 Tests (via `StubRuntime`): a duplicate reduces to one by id; an orphan
  (terminal/unknown instance) is reaped; a live/transitional instance is left
  alone; a sandbox without this node's tag (a peer's) is never touched.

## 3. Credential sweep teardown order

- [ ] 3.1 In `reap_credentials`, delete the sandbox(es) tagged with the credential's
  instance id (by substrate id) before `remove_credential` deletes the volume — the
  instance-then-volume order `destroy` already uses.
- [ ] 3.2 Test: a volume still mounted by a sandbox that outlived its instance is
  released (the sweep removes the sandbox first), rather than the perpetual 409 the
  production node showed.

## 4. Verification

- [ ] 4.1 `make check` cargo gates: `clippy --workspace --all-targets -- -D
  warnings`, `cargo fmt --check`, node lib + guest suites, the existing
  `hypeman_runtime` and reconcile/credential-sweep tests.
- [ ] 4.2 T5 (`t5_crash.rs`) still passes — recovery/reconcile unchanged in shape.
- [ ] 4.3 No T1/T7 regression: the single-instance happy path adopts its one
  sandbox on cold boot rather than creating a second (covered by 1.4).
- [ ] 4.4 After merge, deploy to the beta node and verify convergence from the
  known-clean 0-instance state (create/pause/resume a session; confirm exactly one
  sandbox per instance and no leak under repeated reconcile).
