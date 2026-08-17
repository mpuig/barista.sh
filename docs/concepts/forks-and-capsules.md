# Forks and capsules

Forks branch a retained snapshot into a new instance; capsules move that state
between nodes. Both are neutral execution mechanisms — Barista owns identity,
lineage, compatibility, and recovery; the substrate owns the bytes; and app,
tenant, registry, and sharing concepts live in `barista-apps` and Barista Cloud,
never here.

## Forking

`ForkInstance(source_snapshot_id, target_instance_id)` creates a new,
independently owned instance from a retained snapshot. The source keeps running.

```sh
barista fork <source-snapshot-id> --target-instance-id <child-id>
barista fork <source-snapshot-id> --require-cow      # fail rather than freeze
```

The child clones the source's spec exactly except for identity and lineage. It
comes up `RUNNING` with a fresh guest identity and a new execution epoch (see
[Execution epochs](#execution-epochs)).

### Fork mode is measured, never assumed

The runtime reports what it actually did on the operation's `actual_fork_mode`:

| Mode | Meaning | Source frozen? |
|---|---|---|
| `COW` | Copy-on-write: the child shares the source's memory until it writes | No |
| `FULL_COPY` | The source's bytes were copied | Yes — while copying |

`--require-cow` fails closed with `FORK_MODE_UNAVAILABLE` rather than accept a
full-copy freeze the caller did not ask for. A full-copy fork reports the freeze
on the operation's `degraded` field; it is never silent. A runtime that cannot
fork at all refuses with `FORK_MODE_UNAVAILABLE`.

Barista does not reimplement copy-on-write or memory paging — the substrate does
(ADR-001 §13.7). The node owns the journaled operation, the child's identity,
its lineage, and crash recovery.

### Lineage

Every forked or capsule-restored instance records where it came from on
`Instance.lineage`: a stable `lineage_id` grouping a source and its descendants,
the `source_snapshot_id` or `source_capsule_id`, and the `parent_instance_id`.
Lineage is durable on the instance row and announced as a `LINEAGE_RECORDED`
event; it is never reconstructed from the event log.

## Capsules

A **capsule** is a content-addressed, portable envelope for snapshot state: a
deterministic manifest plus the immutable objects it references by digest and
length. The capsule id is the digest of the manifest's canonical serialization,
so the same logical state has the same id on every node.

```sh
barista capsule export <snapshot-id> --manifest-out capsule.pb
barista capsule import --manifest capsule.pb
barista capsule inspect <capsule-id>
barista capsule ls [--lineage <id>]
barista capsule delete <capsule-id>
```

### Verify-then-publish

Export reads the snapshot's objects, stages each into the local immutable-object
store, verifies its length and digest, and only then registers the capsule. A
capsule becomes visible only after **every** object verifies. Re-exporting the
same snapshot is idempotent by content id: identical objects deduplicate and the
capsule id is unchanged.

Import stages nothing it cannot verify: every object named by the manifest must
be present and match its digest and length, the manifest schema must be
understood, and the CPU class must match this node — or the import is refused
(`CAPSULE_VERIFICATION_FAILED` / `CAPSULE_INCOMPATIBLE`). Import registers the
capsule and a restorable snapshot; it does **not** boot anything. Restore is a
separate `ResumeInstance` or `ForkInstance` against the registered snapshot.

### Exact compatibility

A capsule carries the same restore-compatibility keys a snapshot does —
`cpu_class`, `template_hash`, `runtime_bundle_ref`, and `kind`. Exact restore
requires all of them to match; a mismatch fails with `CAPSULE_INCOMPATIBLE`
rather than silently cold-booting. Pin `runtime_bundle_ref` so a runtime upgrade
cannot quietly invalidate a capsule.

### Storage tiers

The local directory tier is always available. The object-store tier
(`--tier object-store`) requires a configured backend and the
`object_store_snapshots` capability; an unmet demand fails with
`OBJECT_STORE_UNAVAILABLE` rather than silently falling back to local. A remote
object becomes visible only after every required object verifies.

### Deletion and garbage collection

Deletion is logical first (crash-safe): the capsule record and its reference
decrements commit in one transaction, then the physical bytes are collected. An
object is **never** removed while any live capsule references it, so a shared
object survives until its last capsule is deleted. `DeleteCapsule` is idempotent;
deleting an absent capsule is a no-op success.

## Execution epochs

Every boot, resume, and fork issues a fresh **execution epoch** — a globally
unique, monotonic number bound to the instance. Two sibling forks never share
one. Issuing a new epoch revokes the prior one: a platform-mediated grant bound
to an older epoch (a prior run, or a sibling) is refused with `EPOCH_REVOKED`.

Platform-mediated grants travel through a **grant carrier** delivered fresh on
every restore over Contract C and bound to the new epoch. The carrier lives only
in the runtime's RAM-backed mount (`/run/barista/grant-carrier`), so it has no
disk-snapshot representation, and it is replaced on every restore before the
post-restore rebind hook runs — so the workload reconnects using the new epoch's
grant, not the revoked one.

### The honest limit

Epoch rotation replaces *platform-mediated* grants. It says nothing about values
a workload copied into its own memory: an exact-memory snapshot captures those,
so **a capsule is secret-bearing regardless**. `safe_grant_rebind` is a narrow
capability about mediated grants — never a claim that the kernel scrubbed every
secret from RAM. Treat capsule artifacts as secrets.

## Crash safety

Fork, export, import, and capsule deletion are journaled, idempotent operations.
On boot the node reconciles the immutable-object store with the journal: it
sweeps staging files a crashed upload left behind and collects objects whose last
reference is gone, and it never deletes an object with a live reference. A fork
interrupted mid-flight converges its half-made target to `FAILED` (leaving the
sandbox reapable) and leaves the source untouched.

## The boundary with `barista-apps`

This node exposes neutral mechanisms only. It has no notion of an app, a tenant,
a registry, a public/private share, or a billing event. `barista-apps` defines
the public Host API and maps it onto these Contract A verbs; Barista Cloud
implements that Host API as a governed multi-tenant provider. A capsule's id and
compatibility keys are the seam: apps and providers build lineage trees, sharing
policy, and evaluation on top of the mechanisms documented here, without the node
learning any of it.

## Known substrate limitation: forked-guest network identity

On a memory-fork the substrate assigns the child a new host-side network
identity, but the forked guest resumes with the **source's** in-VM IP (its
network was configured once at boot, and a fork does not re-run boot). Until the
substrate reconfigures the forked guest's network, a service inside the fork is
not reachable at the child's advertised address, even though the fork is
`RUNNING`, correctly identified, and isolated. This is a substrate behavior, not
a node one — tracked in
`docs/upstream-issues/07-forked-guest-keeps-source-network-identity.md` and
reported upstream. Every other fork guarantee (lineage, measured mode, source
preservation) holds.
