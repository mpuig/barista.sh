# Snapshots

A snapshot records captured state plus the compatibility keys that decide whether
that state can be restored safely.

## Kinds

| Kind | Contents | Resume behavior |
|---|---|---|
| `MEMORY_AND_DISK` | Guest RAM and writable filesystem state | Continue the same process without a guest reboot. |
| `DISK_ONLY` | Writable filesystem state only | Cold-start the workload against preserved disk. |

`Snapshot.kind` is always explicit. `--require-memory` refuses a disk-only pause
or cold-boot resume when continuity is required.

## Capture verbs

| Verb | Source | Freeze promise | Retention |
|---|---|---|---|
| `Pause` | `RUNNING` | May freeze while copying; releases the sandbox | Latest lifecycle snapshot. |
| `Checkpoint` | `RUNNING` | Must not freeze | Refused on both current runtimes because live checkpoint is absent. |
| `CreateSnapshot` | `RUNNING` or `PAUSED` | Declares `froze_workload` for a running source | Retained until deletion or instance destruction. |

`CreateSnapshot` requires a runtime that can capture memory. The tooling-only
`fake` runtime refuses it with `CAPABILITY_MISSING`; use `Stop` when disk-only
state is enough.

## Retained snapshots

```sh
barista snapshot create <instance-id> --name before-migration
barista snapshots --instance <instance-id>
barista resume <instance-id> --snapshot <snapshot-id> --require-memory
barista snapshot delete <snapshot-id>
```

A name is a human label unique within one instance. Restore and deletion always
use the snapshot id.

The current contract restores a snapshot into the instance that owns it. Using
one snapshot to create a different instance—a golden-template fork—is planned
for a later contract and is not a CLI operation today.

## Restore keys

Every memory snapshot records:

| Key | Pins | Failure reason |
|---|---|---|
| `cpu_class` | Host CPU features | `CPU_CLASS_MISMATCH` |
| `template_hash` | Image digest, bundle, resources, and architecture | `SNAPSHOT_INVALIDATED` |
| `runtime_bundle_ref` | Runtime and guest-agent bundle | `BUNDLE_MISMATCH` |

A mismatch cold-boots with a `DEGRADATION` event unless the caller sets
`require_memory`, in which case the node refuses before boot.

## Digest pinning

`template.oci.digest` is required. A mutable tag cannot identify bytes safely
enough for memory restore: restoring memory captured from one image onto a
different root filesystem can pass superficial checks and corrupt the process.

The CLI accepts either form:

```sh
barista create --image ghcr.io/acme/agent:2026-08 --digest sha256:… -- /app/agent
barista create --image ghcr.io/acme/agent@sha256:… -- /app/agent
```

The image string is a label; the digest participates in snapshot identity.

## Restoring the same bytes twice

One retained snapshot can be restored more than once into its owning instance.
Before each resumed workload runs, the guest agent mixes fresh host entropy,
forces a kernel CRNG reseed, and steps the clock. Two restores therefore do not
continue with byte-identical random streams.

## Storage tiers

| Tier | Status | Meaning |
|---|---|---|
| `LOCAL` | Implemented | Snapshot bytes live on the owning node. Fast, but node-local. |
| `OBJECT_STORE` | Reserved/planned | Intended to survive node loss and support warm cross-host migration. |

The fleet bucket currently stores desired state and ownership leases, not memory
snapshots. Losing a node loses its local warm state; another owner may cold-boot
from desired state according to the session's owner-loss policy.

## Hooks

Contract A's `Hooks` fields bracket capture and restore:

```yaml
hooks:
  pre_snapshot_cmd: ["/app/quiesce"]
  pre_snapshot_timeout_ms: 2000
  post_restore_cmd: ["/app/reconnect"]
  post_restore_timeout_ms: 5000
```

This illustrates the protobuf field shape; it is not a manifest accepted by the
CLI. Use a generated API client to configure hooks.

A pre-snapshot hook is a bounded chance to quiesce, not a veto. Timeout outcomes
are recorded and capture proceeds. Post-restore runs after entropy and clock
duties so it can reopen external connections against current time.

## Related

- [Sleep and wake](sleep-and-wake.md)
- [Guest agent](guest-agent.md)
- [Limits and performance](../platform/limits.md)
