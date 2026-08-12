## Why

A single fleet session **leaked 18 hypeman VMs** on the beta node, exhausting its
network-bandwidth budget so hypeman began refusing **all** new instances
(`insufficient network bandwidth`) — a node-wide denial of service. Verified in
code and in production evidence (a cross-repo review + the node's own hypeman
request log).

The root cause is that **hypeman instance creation is not idempotent**:

- `create_instance` POSTs `/instances` with a name but **no id**
  (`runtime/hypeman/client.rs`), while the volume path passes `&id=` precisely
  because "names are NOT unique — creating twice by name yields two … and then
  every lookup by name fails" (`client.rs`, the `create_volume_from_archive`
  comment).
- `start()` dedups only by name: `get_instance(name)` → on `404` it calls
  `create_fresh` (`runtime.rs`). Hypeman has **no distinct "ambiguous" status** —
  its request log over the leak window shows only `200` and `404`, and a name that
  does not uniquely resolve surfaces as **404**. So once two VMs share a name,
  every lookup is a `404` → `start()` creates **another** → a self-reinforcing
  spiral (the 18). A TOCTOU across concurrent reconciles or a restart can seed the
  first duplicate, and there is no periodic sweep that can bring N same-named VMs
  back down — the reconciler sweeps *credentials* only, and `delete_instance` by
  an ambiguous name maps `404 → success` while deleting nothing.

Now, because it is a live node-wide DoS in the deployed build, and it cannot
self-heal: once leaked, the VMs stay until deleted by unique id by hand (which is
how the node was recovered).

## What Changes

- **Adopt-by-id before create.** Before creating a sandbox, the hypeman runtime
  SHALL list existing sandboxes tagged with this instance's id (scoped to this
  node), adopt the survivor, and **delete any extras by their unique substrate
  id** — creating only when none exist. So a create converges to exactly one
  sandbox per instance instead of spawning another.
- **Periodic instance dedup/orphan sweep.** The reconciler SHALL periodically
  enumerate this node's sandboxes, reduce any instance with more than one sandbox
  to one (deleting extras by id), and delete sandboxes whose instance is not live
  in the journal — the same zero-orphan invariant the credential sweep already
  has, extended from volumes to instances. This heals leaks the create path
  missed and closes the TOCTOU window.
- **Fix the credential sweep's teardown order.** The credential sweep deletes the
  *volume only*, so a still-mounted leaked VM returns `409` forever (the recurring
  5-minute WARN). It SHALL delete the instance before the volume, the order
  `destroy` already uses ("so the volume is never pulled out from under a VM still
  mounting it").
- **Roll back the VM on `await_running` failure.** A create whose readiness wait
  fails SHALL delete the sandbox it just created, rather than leaving it stranded.
- Not a contract change: no proto, no gRPC surface. `fake`/`runsc` are unaffected
  (the defect and fix are hypeman-adapter specific).

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `runtime-hypeman`: ADD that hypeman instance creation is **convergent** — a
  create/adopt resolves to exactly one sandbox per instance (extras deleted by
  unique id), and a failed readiness wait rolls the sandbox back. This is the
  hypeman-specific idempotency the volume path already has and the instance path
  lacks.
- `node-agent-api`: ADD, to the zero-orphan invariant, that reconciliation
  periodically reaps **orphaned and duplicate instances** by unique id — not only
  orphaned credentials — and that the credential sweep removes a still-held
  volume's instance before the volume.

## Impact

- **Code**: `runtime/hypeman/runtime.rs` (adopt-by-id in the create/start path,
  rollback), `runtime/hypeman/client.rs` (delete-by-id already exists; capture the
  substrate id from create if the design persists it), `reconcile.rs` (new
  instance sweep + credential-sweep ordering), possibly `db.rs` (a
  `substrate_instance_id` column, if the design keeps the id rather than relying on
  the tag). No dependency changes.
- **Acceptance tests**: claims none of T1–T12 as new, but it protects the ones
  that create instances (T1, T7) from the leak. DoD is `make check` plus the new
  gap tests below.
- **Contracts**: none. No `v1alpha1` proto or gRPC surface changes.

## Constitution Check

- **Schema-first**: no contract type added or duplicated.
- **Crash-safe by construction** (§I): the fix strengthens the reconciler's
  zero-orphan invariant, which is exactly this principle; the new sweep is
  journaled-idempotent like the existing credential sweep.
- **Honest capabilities** (§I): a leaked VM is a silent capability loss (the node
  stops accepting work); making creation convergent and the sweep effective is
  making that failure impossible rather than tolerated.
- **Simple by default** (§IV): the minimal fix maintains "≤1 sandbox per instance"
  via adopt + sweep without persisting a new id; design.md weighs that against the
  heavier "persist the substrate id and drive every op by it," and names why the
  minimal invariant is sufficient to close the DoS.
- **Human control** (§V): a real production incident and a lifecycle-behavior
  change, so it is proposed for ratification.
