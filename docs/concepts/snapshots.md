# Snapshots

A snapshot is a session frozen to disk: its memory, its filesystem, and the keys
that say what it can be restored onto.

## Kinds

| Kind | Contents | Resume behaviour |
|---|---|---|
| `MEMORY_AND_DISK` | Guest RAM plus the writable filesystem layer | The process continues. Same variables, same open files, same uptime. |
| `DISK_ONLY` | The writable filesystem layer | The workload cold-starts against preserved files. |

`Snapshot.kind` always tells you which one you got. A runtime without memory
snapshot support does not fake a memory snapshot — it returns `DISK_ONLY` and
says so. Callers that cannot use a disk-only snapshot pass `require_memory` and
get an error instead.

## How a snapshot is created

| Verb | Source state | Freezes the workload? | Retained? |
|---|---|---|---|
| `Pause` | `RUNNING` | Yes, for the copy | Until the session resumes or is destroyed |
| `Checkpoint` | `RUNNING` | **No** — live checkpoint, session keeps running | Yes |
| `CreateSnapshot` | `RUNNING` or `PAUSED` | Briefly, if the source is running | Yes, until you delete it |

`Checkpoint` requires the `live_checkpoint` capability. Where the runtime does
not have it, `Checkpoint` fails with `CAPABILITY_MISSING`. It does not quietly
pause and resume your session and call the result a checkpoint — the two verbs
make different promises and keep them distinct.

`CreateSnapshot` is the honest middle ground: it works everywhere, and when the
source is running it declares the freeze rather than hiding it. The operation
carries `froze_workload: true`, and `pre_snapshot_cmd` runs first, exactly as it
does for a pause.

### Named snapshots

```sh
barista snapshot create agent-42 --name before-migration
barista snapshots --instance agent-42
barista resume agent-42 --snapshot <snapshot-id>
```

Named snapshots are retained. They survive the session's own lifecycle churn and
are removed only by `barista snapshot delete` or by destroying the session without
`--keep-snapshots`.

Two things fall out of this:

- **Point-in-time recovery.** Take a named snapshot on a schedule and you can
  put the whole session — process state included, not just its database — back
  to how it was on Tuesday.
- **Golden templates.** Prepare one session with the models loaded and the
  caches warm, snapshot it, and restore it as the starting point for many
  sessions. Restores are cheap and independent: the same bytes can be restored
  any number of times.

## Restore keys

Every snapshot records three keys, and all three must match for memory to be
restored:

| Key | What it pins | Failure reason |
|---|---|---|
| `cpu_class` | A hash of the host's CPU feature flags | `CPU_CLASS_MISMATCH` |
| `template_hash` | Image digest + bundle + resources + architecture | `SNAPSHOT_INVALIDATED` |
| `runtime_bundle_ref` | The runtime and guest-agent versions, pinned per build | `BUNDLE_MISMATCH` |

Memory captured on one CPU generation cannot be restored onto another that
lacks its instructions. Memory captured from image A cannot be restored onto the
rootfs of image B. Upgrading the substrate invalidates snapshots taken under the
old one, which is why the bundle is versioned as a unit rather than as loose
parts.

A key mismatch is a cold boot with a `DEGRADATION` event, unless you asked for
`require_memory`. See
[When memory cannot be restored](sleep-and-wake.md#when-memory-cannot-be-restored).

## Digest pinning is required

An image reference with no digest is rejected at create with `INVALID_SPEC`.

This is not pedantry. A tag can be repointed at different bytes while the
template hash stays stable, which makes invalidation fail **silently**: the keys
still match, so a restore puts memory captured from the old image onto the new
image's rootfs and every precondition passes. Requiring the digest removes the
failure mode rather than detecting it.

The `image` field survives as a human-readable label with no role in identity.

## Two restores of one snapshot are not twins

Restoring one snapshot twice would, without intervention, give you two guests
with byte-identical kernel RNG state — two "random" session keys that are the
same key.

Barista handles this as a platform duty, not as your problem. On every restore, the
guest agent mixes fresh host entropy into the kernel pool and forces a CRNG
reseed before your workload runs. Restored twins provably diverge. The response
reports whether the entropy was *credited* or only *mixed*, so a constrained
sandbox that could not do the stronger thing says so instead of reporting
success.

Clock is handled in the same place and the same way: the guest clock is stepped
to host time, and the drift it had accumulated is reported on the `RESTORED`
event.

## Storage tiers

| Tier | Where | Use |
|---|---|---|
| `LOCAL` | The owning node's disk | The default. Fast, and the reason a node-local pause pins the next resume back to that node. |
| `OBJECT_STORE` | A bucket you own | Survives node loss; enables migration between hosts. |

The local tier is the hot path and the one that carries interactive latency. A
paused session costs roughly **0.4 bytes of disk per byte of live memory**, plus
a small sparse overlay, and zero CPU and zero host RAM.

## Quiesce before, reconnect after

Two hooks bracket every snapshot:

```yaml
hooks:
  pre_snapshot_cmd: ["/app/quiesce"]     # flush buffers, drop caches, close what cannot survive
  pre_snapshot_timeout_ms: 2000
  post_restore_cmd: ["/app/reconnect"]   # reopen provider sockets, re-register
  post_restore_timeout_ms: 5000
```

`pre_snapshot_cmd` is a **chance, not a veto**. If it times out, the snapshot
proceeds and the outcome is recorded in the snapshot's metadata — a workload
that hangs cannot make its session unpausable.

## Related

- [Sleep and wake](sleep-and-wake.md)
- [Capabilities and tiers](capabilities-and-tiers.md)
- [Best practices](../best-practices.md)
