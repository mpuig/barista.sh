# Tasks: nap-015-named-snapshots

## 1. Contract

- [x] 1.1 `CreateSnapshot` RPC + optional `name` on the snapshot record +
      workload-frozen marker on the operation, all additive; regenerate;
      `buf breaking` green

## 2. Node agent

- [x] 2.1 `ops`: new op kind — RUNNING → `CHECKPOINTING` → RUNNING (the state
      the machine already has), no transition from PAUSED; concurrency guard
      applies (design decision 2); quiesce hook before capture, outcome
      recorded on the snapshot (reuses pause's task-4.4 plumbing)
- [x] 2.2 Backend call is nap-010's `Runtime::create_snapshot`; journal write
      with the same keys as pause, in the same transaction shape, plus `name`
- [x] 2.3 Frozen marker keyed on `live_checkpoint: false`, not on the runtime
      name (design decision 1); substrate 409 on duplicate name surfaces as a
      conflict naming the name
- [x] 2.4 Retention: verify the TTL/lifecycle sweeps never touch named
      snapshots; `DestroyInstance` semantics with and without `keep_snapshots`
      covered for them
      — **verified with a finding.** The lifecycle walk (pause → resume → stop
      → start) and `DestroyInstance { keep_snapshots: true }` — which is what
      `reconcile::enforce_ttl` submits — leave a named snapshot listed and
      restorable. `keep_snapshots: false` is a **no-op today**: `OpKind::Destroy`
      ignores the flag entirely, so no path other than `DeleteSnapshot` removes
      any snapshot row. That satisfies the delta spec as written ("removed *only
      by* `DeleteSnapshot` or by destroying without `keep_snapshots`") but
      contradicts design decision 4's claim that "both paths already exist".
      Left unimplemented deliberately: it would change behaviour for *every*
      snapshot, not just named ones, and needs a policy call this change has no
      mandate for (does a substrate delete failure fail the destroy?).

## 3. CLI

- [x] 3.1 `nap snapshot create <id> [--name <n>]`; names in `nap snapshots`

## 4. Verification (DoD)

- [x] 4.1 Stub-level: concurrency conflict; PAUSED capture leaves PAUSED and
      no frozen marker; duplicate name refused
- [x] 4.2 Substrate-gated: the PITR loop (create named → work → resume by id →
      the later work is gone, memory restored); frozen marker exactly when
      source was RUNNING
      — **run against the rank-1 substrate and passing**
      (`the_pitr_loop_returns_the_session_to_its_named_point`, zero skips), so
      the green is the real path and not a self-skip.

      It had to run from *inside* the Lima VM: the node agent reaches the guest
      agent on the substrate's internal `10.100.0.0/16`, which is unroutable
      from macOS (hypeman #358) — confirmed environmental by running the same
      binary at `HEAD` with the change stashed and getting identical failures.
      The VM needed rustup 1.94.1 (`rust-toolchain.toml`) plus
      `build-essential`, since rustup's minimal profile ships no linker, and the
      build uses `CARGO_TARGET_DIR=target-linux` to keep off the host's
      artifacts. Run with `--test-threads=1`: concurrent microVMs exhaust the
      substrate's disk-I/O allocation and fail as `insufficient_resources`,
      which is indistinguishable from a real defect at a glance.
- [x] 4.3 Drift test: `createInstanceSnapshot` name field + 409 conflict
      pinned
- [x] 4.4 `make check` green
