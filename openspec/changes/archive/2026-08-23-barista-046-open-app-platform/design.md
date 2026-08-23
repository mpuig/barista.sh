## Context

See proposal.md — Why. Contract A already has retained snapshots, explicit
compatibility keys, idempotent operations, a runtime capability map, and restore
duties. The rank-1 substrate already exposes fork beneath Barista (BRD B39 and
ADR-001 v2 §13.7), while Phase 1 deliberately deferred fork and the remote tier.
The new app ecosystem consumes neutral execution mechanisms only; its manifest
and Host API remain outside this repository.

## Goals / Non-Goals

**Goals:**
- Make exact state branchable and movable without coupling the node to apps,
  tenants, registries, or a particular object-store vendor.
- Preserve source state, deterministic recovery, honest capability negotiation,
  and compatibility checks.
- Give descendants fresh platform identity and make the limits of exact-memory
  secret hygiene explicit.

**Non-Goals:**
- Live checkpoint (T2), incremental snapshot chains, cross-architecture memory
  translation, tenant policy, public sharing, or semantic Lift.
- Resource/process overrides during fork; v1 clones the source spec exactly.
- Detecting or removing arbitrary secrets that workload code copied into RAM.

## Decisions

### D1 — Extend Contract A; do not add an app-facing API here

Add protobuf-first fork, export, and import verbs to
`barista.node.v1alpha1`. `barista-apps` owns the public Host API and maps it to
Contract A. This preserves the constitution's schema-first boundary and the
one-way commercial seam. The simpler alternative—have apps call Contract A—is
insufficient because it exposes a privileged node API and cannot represent a
multi-tenant host safely.

### D2 — Fork is a new target instance, not resume with an alias

`ForkInstance(source_snapshot_id, target_instance_id)` journals a distinct
instance whose immutable spec matches the source except for identity and
lineage. This follows B39 while preserving Barista's named-session ownership.
Overloading `ResumeInstance` is superficially smaller but would violate its
stable-instance semantics and make idempotency and cleanup ambiguous.

The runtime returns an actual mode (`COW` or `FULL_COPY`) and whether capture
froze the source. Barista delegates the bytes to hypeman; it owns journal,
identity, compatibility, and recovery. A caller can require CoW.

### D3 — Capsule manifest is deterministic protobuf plus immutable objects

The capsule envelope is a deterministic serialization of a new protobuf
manifest. It references typed immutable blobs by digest and length. The capsule
id is the digest of canonical manifest bytes. Local-directory and object-store
backends share the same logical layout; storage URLs and credentials never enter
the manifest. This implements R-SNAP-2 without making S3 an API concept.

A tarball-only design is simpler but cannot deduplicate shared objects, verify
partial uploads, or let Cloud retain one blob for several lineages. Incremental
chains remain deferred; v1 exports full immutable objects.

### D4 — Import is verify-then-register; restore remains a separate operation

Import stages objects under temporary identities, verifies every digest, then
atomically registers the capsule/snapshot record. Restore performs compatibility
checks before allocating a sandbox. Keeping import separate makes retries and
forensic inspection possible and prevents a corrupt upload from creating a
half-started instance.

### D5 — Execution epochs secure platform grants, not arbitrary memory

Every boot/resume/fork receives a new execution epoch over Contract C. The
platform-managed grant carrier is non-persistent on disk and its authority is
bound to that epoch; the prior epoch is revoked before readiness. Restore duties
follow BRD B26/B48: reseed, clock, replace grant carrier, invalidate mediated
connection handles, run `post_restore`/rebind, then readiness.

Exact memory can contain copies made by the workload, so capsules remain
secret-bearing and `safe_grant_rebind` is a narrow capability. Claiming the
kernel can scrub all secrets would be simpler messaging and false semantics.

### D6 — Reference counting is logical and crash-safe

Snapshot and capsule records reference immutable objects. Deletion removes the
logical record first only after a durable decrement/GC intent; physical object
collection is retryable and never removes an object with a live reference.
Leaking an unreferenced object temporarily is preferable to deleting live state,
matching the existing substrate-before-journal truth rule.

## Risks / Trade-offs

- [A copied static API token remains usable in both children] → Capsules are
  secret-bearing; only platform-mediated, epoch-bound grants receive the safety
  guarantee. Apps must use grant references for safe branching.
- [Full-copy fork pauses a large source] → Report mode and freeze duration;
  callers may require CoW and fail closed.
- [Object upload succeeds but journaling crashes] → Stage by digest, recover
  from the operation journal, and garbage-collect unreferenced staged objects.
- [Runtime upgrades invalidate capsules] → Pin `runtime_bundle_ref` and refuse
  exact restore before boot; no silent cold boot for capsule restore.
- [Scope crosses the old Phase-1 boundary] → Treat this as the consumer-driven
  Phase 3+ work the BRD reserved; ratification is required before apply.

## Migration Plan

1. Land additive proto and journal schema support with all capabilities false.
2. Add local immutable-object storage and full-copy fork tests.
3. Wire hypeman native fork and report measured semantics.
4. Add object-store export/import behind explicit node configuration.
5. Add execution epochs and guest rebind duties; only then advertise safe grant
   rebinding.
6. Keep existing local snapshots and clients unchanged; rollback disables the
   new capabilities but does not reinterpret existing state.

## Open Questions

- Whether the first object-store backend should support server-side copy as an
  optimization; the contract and task breakdown do not depend on it.

