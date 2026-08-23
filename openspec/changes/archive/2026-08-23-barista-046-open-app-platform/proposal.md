## Why

Barista already preserves a named session's exact execution state, but an
independent application platform cannot yet branch that state into a new
session, move it between hosts, or inject fresh identity without copying stale
credentials. The open kernel needs these neutral primitives so open-source apps
and third-party services can build on Barista without depending on Barista
Cloud.

## What Changes

- Add a journaled `ForkInstance` operation that creates a new instance from a
  retained snapshot while preserving the source and recording lineage.
- Add a content-addressed, versioned Barista Capsule envelope for exporting and
  importing compatible snapshot state, including an object-store storage tier.
- Add branch-safe restore duties: every descendant receives a fresh execution
  identity, platform-managed grants are rebound to that identity, and prior
  execution epochs are revoked. Exact-memory artifacts remain secret-bearing
  because a workload can copy granted values into its own memory.
- Expose all new behavior through additive protobuf fields/RPCs and capability
  discovery; unsupported runtimes fail loudly or use an explicitly reported
  full-copy fallback.
- Keep app manifests, SDKs, agent adapters, tenancy, registries, sharing policy,
  and billing outside the kernel.

No existing RPC or on-disk artifact is reinterpreted. Capsule import accepts
only the new versioned envelope; old local snapshots remain local snapshots.

## Capabilities

### New Capabilities

- `instance-forks`: branch one retained execution point into a distinct,
  independently owned instance with durable lineage and fresh identity.
- `portable-capsules`: export, verify, import, and restore content-addressed
  session state across compatible Barista nodes.
- `ephemeral-grants`: deliver credentials and branch identity through a
  non-snapshotted channel and rebind them on every restore or fork.

### Modified Capabilities

- `snapshots`: add the object-store tier, immutable content identity, export
  eligibility, and source-independent restore semantics.
- `node-agent-api`: add the schema-first fork/capsule operations, capabilities,
  error reasons, operation journaling, and lineage events.
- `guest-agent`: extend restore duties with connection invalidation, execution
  identity rotation, grant remounting, and a bounded post-fork rebind hook.

## Impact

- **Contracts:** additive changes to `barista.node.v1alpha1` and
  `barista.guest.v1alpha1`; `buf breaking` must remain green.
- **Node Agent:** operation journal, snapshot metadata, import/export, lineage,
  reconciliation, and storage-tier handling.
- **Runtime trait:** adopt the substrate's fork/export mechanisms; Barista must
  not reimplement CoW or memory paging. A full-copy fallback is allowed only
  when reported honestly.
- **Guest Agent:** new restore/fork duty ordering and an ephemeral mount/channel
  that is excluded from snapshot artifacts.
- **Security:** capsule artifacts are treated as secrets. Platform-managed
  grants and connections must not remain valid in two descendants; arbitrary
  credentials copied by a workload remain outside that guarantee.
- **Acceptance:** this change claims no existing Phase-1 T1–T12 test as new.
  Its DoD adds fork isolation, same-source divergence, capsule round-trip,
  tamper refusal, incompatible-host refusal, crash recovery, and no-secret-copy
  integration tests, plus unchanged T3/T5/T8/T9/T10 and `make check`.

## Constitution Check

- **Schema-first:** every wire addition is protobuf-first; no duplicate contract
  types are introduced.
- **Honest capabilities:** CoW, remote storage, and exact-memory portability are
  advertised independently and unsupported demands fail explicitly.
- **Crash-safe operations:** fork, export, import, and capsule deletion are
  idempotent journaled operations with substrate-first cleanup.
- **Adopt the substrate:** the runtime trait delegates fork and snapshot bytes
  to hypeman or another runtime; Barista owns identity, compatibility, lineage,
  and recovery only.
- **Simple by default:** the first implementation may freeze-copy-resume and use
  full immutable objects; incremental chains and live checkpoint remain out of
  scope until measured demand justifies them.
