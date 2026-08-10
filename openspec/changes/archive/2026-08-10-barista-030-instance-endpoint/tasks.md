# Tasks: barista-030-instance-endpoint

## 1. Contract

- [x] 1.1 Add `InstanceNetwork { string address = 1; }` and
      `InstanceNetwork network = 11;` on `Instance` in
      `proto/barista/node/v1alpha1/node.proto`, with comments stating the
      populated-only-while-RUNNING and absent-means-unavailable semantics.
- [x] 1.2 Regenerate (`task gen`); descriptor diff is additive only
      (`task breaking` clean: new message + new field, no renumbering).

## 2. Runtime seam (Contract B)

- [x] 2.1 Add `Runtime::workload_address(&self, h: &Handle) ->
      Result<Option<String>>` with default `Ok(None)`
      (`runtime/mod.rs`), documented as a per-moment workload property.
- [x] 2.2 Implement it for `hypeman` (`runtime/hypeman/runtime.rs`): resolve
      the instance's IP from the substrate per call, mirroring
      `channel.rs`'s per-connect resolution; map substrate errors to
      `Ok(None)` + WARN per design decision 5.

## 3. Service

- [x] 3.1 Populate `network` in `service.rs` for `GetInstance` when journal
      state is `RUNNING`; leave absent otherwise
      (`NodeAgentService::instance_to_proto`).
- [x] 3.2 Populate it for `ListInstances` with a bounded concurrent fan-out
      (`futures_util` `buffered`, `WORKLOAD_ADDRESS_CONCURRENCY = 8`, the
      reconcile probe pattern; order preserved).

## 4. Tests and docs

- [x] 4.1 Hypeman-gated integration test: running instance reports a
      non-empty address and the guest-agent port accepts TCP at it; paused
      instance reports none (delta scenarios 1–2) —
      `tests/instance_endpoint.rs`. Always-on state-gating + error-degradation
      coverage in `service.rs` unit tests, since the substrate-gated test
      self-skips on most machines.
- [x] 4.2 Fake-runtime test: running instance reports no `network` (delta
      scenario 3) — `tests/instance_endpoint.rs`.
- [x] 4.3 Docs: field reference in `docs/api/index.md`, one honest paragraph
      in `docs/concepts/networking-and-egress.md` (what the address is, what it
      is not — no ports, no cross-host claim).
- [x] 4.4 `openspec validate barista-030-instance-endpoint` passes;
      `openspec validate --all --strict` 20/20; `buf lint`, `buf breaking`,
      `cargo fmt`, `cargo clippy -D warnings`, `task docs --strict`, and the
      workspace test suite (node-agent 262 passed / 0 failed) all green.
      Claims no Phase 1 acceptance test. NB: `gen-check` (a `git diff` gate)
      stays red until the regenerated proto is committed — the regeneration
      itself is done and additive.
