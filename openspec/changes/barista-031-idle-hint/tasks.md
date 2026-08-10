# Tasks: barista-031-idle-hint

## 1. Contracts

- [x] 1.1 `guest.proto`: add `WorkloadService { rpc DeclareIdle }` (empty
      request/response messages) and `HealthResponse.idle_declared = 6`,
      with comments carrying the no-auth-same-trust-domain reasoning and the
      "agent records, node decides" split.
- [x] 1.2 `node.proto`: add `optional TtlAction idle_action = 10` to
      `InstanceSpec` (absent = ignore; present semantics = TTL's, including
      `UNSPECIFIED` = PAUSE) and `EVENT_TYPE_IDLE_FIRED = 9`.
- [x] 1.3 Regenerate (`task gen`); descriptor diff additive-only
      (`task breaking` clean).

## 2. Guest agent

- [x] 2.1 Serve `WorkloadService` on `/run/barista/workload.sock`;
      `DeclareIdle` records the timestamp in agent state; management RPCs
      are not registered on this listener (delta scenario 2).
- [x] 2.2 Inject `BARISTA_WORKLOAD_SOCKET` into the workload env at
      `start_cmd` spawn (absent when the surface did not come up).
- [x] 2.3 Report `idle_declared` in `Health`; unit tests for both scenarios
      of the guest-agent delta (`tests/workload_idle.rs`).

## 3. Node agent

- [x] 3.1 `should_probe`: probe every tick when the spec arms `idle_action`.
- [x] 3.2 Enforcement in the reconcile tick (`enforce_idle`, driven off the
      readiness probe's own `Health`): act on a declaration passing both
      guards, resolving through `resolve_ttl_action` and running the same
      journaled op path TTL uses; `idle_fired` (+ degradation) emitted once via
      the submission callback, so a replay never re-fires it.
- [x] 3.3 Persist the run-epoch timestamp (`instances.run_epoch_ms`, stamped on
      every transition to RUNNING) — a journal fact, not guest input.
- [x] 3.4 CLI: `barista create --idle-action pause|stop|destroy`.

## 4. Tests, measurement, docs

- [x] 4.1 Fake-runtime integration (`tests/idle_hint.rs`, manual-tick for
      determinism): opt-in respected; PAUSE degrades to STOP with both events;
      guard (b) holds under an interleaved `user_activity` exec. The workload
      declares via the guest binary's new `declare-idle` reference client.
- [x] 4.2 Hypeman-gated integration (`tests/idle_hint.rs`, background
      reconciler for realistic latency; macOS-ignored per hypeman #358):
      declare → memory pause → resume → no re-pause loop (guard a) → fresh
      declaration pauses again; measured hint→paused latency logged. Runs on
      the Linux beta node.
- [x] 4.3 Docs: `docs/concepts/sleep-and-wake.md` (the third pause trigger,
      its guards, its latency), `docs/concepts/guest-agent.md`
      (`BARISTA_WORKLOAD_SOCKET`, env-var-absent means unsupported), CLI
      reference (`docs/cli.md`), and the API reference
      (`docs/api/index.md`, `docs/api/guest-agent.md`).
- [x] 4.4 `openspec validate barista-031-idle-hint` passes;
      `openspec validate --all --strict` 20/20; `buf lint`, `buf breaking`,
      `cargo fmt`, `cargo clippy -D warnings`, `task docs --strict`, and the
      test suite all green (node-agent 281 passed / 0 failed / 9 ignored;
      guest-agent + CLI clean; Python round-trip 2 + scenario 5). Claims no
      Phase 1 acceptance test. NB: `gen-check` (a `git diff` gate) stays red
      until the regenerated proto is committed.
