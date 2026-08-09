# Change: nap-005-hypeman-backend

## Why

This change delivers the differentiator: real memory pause/resume, via the
substrate ratified in **ADR-001 v2 §13.7** — `hypeman` as rank 1, **adopted, not
built**. `nap-004-runtime-substrate-spike` established by measurement that the
semantics work (a 1.5 GiB session resumes in ~2 s with `/proc/uptime` continuing,
on a laptop), that idle cost is free in compute, and that ~35,600 lines of
substrate would otherwise have to be reimplemented for zero differentiation.

What remains is *our* half: the Contract B backend, the snapshot verbs and their
keying, the restore-time guest duties, and honest degradation where hypeman
cannot do what the contract describes.

## What Changes

- Implement the `hypeman` runtime behind the `Runtime` trait: an OpenAPI 3.1 →
  Rust client (upstream ships Go and TypeScript SDKs only), instance create with
  `--entrypoint`/`--env` guest-agent injection, node-scoped tags, and the guest
  channel riding hypeman's `Exec` stream — nap-003's `bridge` mode transfers with
  a different `open_bridge()`.
- Implement snapshots: `Pause` = `standby`, `Resume` = `restore`, snapshot records
  with `cpu_class` / `template_hash` / `runtime_bundle_ref` keying, and restore
  preconditions with machine-readable reasons. hypeman digest-pins the image and
  pins hypervisor/kernel version per instance; **everything else about
  compatibility is ours** (spike §2.3).
- **`Checkpoint` degrades honestly.** hypeman has no live checkpoint —
  snapshot-from-running is `standby → copy → restore` (spike §2.1) — so the
  backend reports `live_checkpoint: false` and `CheckpointInstance` fails with
  `CAPABILITY_MISSING`. This is the nap-002 honest-degradation path, not new code.
- Resume preconditions + **cold-boot fallback** (B42): snapshot-related failures
  fall back to a template cold boot with a degradation event; `require_memory:
  true` opts out.
- Restore-time duties in the guest agent: entropy reseed, clock step, network
  re-check, then `post_restore_cmd` — ordered before the workload observes
  resumption.
- Substrate availability is surfaced, not hidden: while `hypeman-api` is down,
  mutations fail with an explicit reason and a degradation event. The daemon is
  control plane only (spike §5 — `SIGKILL` leaves sessions running and it
  re-adopts them), so this affects management, never running sessions.
- Acceptance tests delivered: **T1 (hypeman), T3, T6 (true pause), T8, T9**, plus
  the measurement carried over from the spike (its task 3.4).

## Capabilities

### New Capabilities
- `runtime-hypeman`: the hypeman backend, its capability surface, and how
  substrate unavailability is reported.
- `snapshots`: pause/resume semantics, snapshot records and keying, restore
  preconditions, cold-boot fallback, and restore-time guest duties.

### Modified Capabilities
- `instance-lifecycle`: `PAUSED`/`RESUMING` become reachable with real memory
  semantics; TTL `PAUSE` becomes a true pause instead of a `STOP` fallback.

## Out of scope — deferred with the rank-2 tier

**`Checkpoint` (live snapshot), and therefore T2, are not claimed by Phase 1.**
Both need `runsc` or firecracker's B31/B38 path (ADR-001 v2 rank 2), deferred
until a consumer needs a snapshot without pausing. **T11** goes with it, since it
gates the `runsc` tier's viability. Recorded in the constitution (v1.3.0) and in
spec §9 rather than left implicit — a deferred test that still looks claimed is
exactly the silent gap the constitution forbids.

## Impact

- New crate module `runtime-hypeman`, plus an OpenAPI→Rust client: a second
  codegen toolchain beside buf/prost, pinned to hypeman API `0.3.0` (pre-1.0, so
  expect churn).
- New node prerequisite: a running `hypeman-api`. On macOS it additionally needs
  `caddy` and `e2fsprogs`, neither documented upstream (spike §1.1) — the node's
  preflight should check for them and say so.
- Two records of one instance (hypeman metadata + Nap's journal); reconciliation
  extends nap-003's node-scoped `list_labeled` sweep rather than being new.
- **`T12`'s premise changes**: hypeman reports `hardware_isolation: true`, so the
  `CAPABILITY_MISSING` case needs a runtime that lacks it (`fake`), and a
  *positive* `require_hardware_isolation` path becomes testable for the first time.
- Depends on: `nap-003-guest-agent`, and ADR-001 v2 (ratified).
