# Sleep and wake

Barista separates a session's lifetime from the sandbox currently running it.
On a memory-capable runtime, idle compute can disappear while the process state
remains restorable.

## Sleeping today

### TTL

`InstanceSpec` can attach an idle deadline and action:

| Action | Effect |
|---|---|
| `PAUSE` (default) | Capture what the runtime supports and release the sandbox. |
| `STOP` | Preserve disk and lose memory. |
| `DESTROY` | Remove the instance and its resources. |

The current CLI exposes `--ttl-seconds`; it uses the default `PAUSE` action. A
generated API client can set the other actions.

Guest passthrough activity—exec and file operations—and explicit lifecycle work
reset the deadline. A fake-runtime TTL pause falls back to `STOP` and emits a
degradation because the runtime cannot preserve memory.

### Idle hint

TTL sleeps a session after a fixed *quiet* period. The workload usually knows
sooner: an agent harness's turn loop, a request handler's `OnComplete`. The idle
hint lets the workload say so directly, and it is the third pause trigger
alongside an operator's `PauseInstance` and TTL.

```sh
barista create --idle-action pause --image … -- /app/agent
```

`--idle-action` reuses the TTL vocabulary (`pause`, `stop`, `destroy`) and its
degradation semantics: a `pause` hint on a runtime without `memory_snapshot`
becomes a `STOP` with an explicit degradation event, never a silent one. It is
**opt-in** — omitting it means idle declarations have no lifecycle effect at all.

The workload declares by calling `WorkloadService.DeclareIdle` on the socket at
`$BARISTA_WORKLOAD_SOCKET` (see [Guest agent](guest-agent.md)). The Node Agent
reads the declaration on its next health poll — idle-armed instances are polled
every reconcile tick (~1 s) — and acts on it as a journaled operation, emitting
`IDLE_FIRED`. Expect the workload's word to become a pause within roughly one
tick plus one pause op.

Two guards keep a declaration honest, and a declaration failing either is ignored
silently (it is a stale fact, not an error):

- it must be **newer than the current run** (the last start or resume), so a
  resumed guest — whose RAM still holds the pre-pause declaration — does not
  re-pause the session in a loop;
- it must be **newer than the last user activity**, so an `exec` marked
  `user_activity` that lands after the declaration keeps the session running.

### Planned: keep-awake leases

TTL sees platform activity, not arbitrary work inside the workload. A session
waiting on a long external call may therefore look idle.

The design includes scoped keep-awake leases so a workload can declare an
invisible busy period. That endpoint and lease model are **not implemented**.
Today, choose a TTL longer than such work, disable TTL while the caller owns the
busy period, or drive lifecycle explicitly.

### Pause cost

The adopted substrate has no live checkpoint. A memory pause freezes the guest
while memory is copied, measured at roughly **1.1–1.6 seconds per GiB of dirty
memory** on the recorded setup.

Keep the working set intentional and use the API's `pre_snapshot_cmd` to discard
rebuildable state where appropriate. `Checkpoint` does not approximate a live
capture: both implemented runtimes report `live_checkpoint: false`, so it fails
with `CAPABILITY_MISSING`.

## Waking today

### Explicit resume

```sh
barista resume <instance-id>
barista resume <instance-id> --require-memory
```

This is the operator and automation path. `--require-memory` refuses a cold-boot
fallback.

### Scheduled wake

One durable alarm may be attached to an instance:

```sh
barista wake-at <instance-id> 5m
barista wake-at <instance-id> 2026-08-09T09:00:00Z
barista wake-at <instance-id> --clear
```

A due alarm resumes `PAUSED` or starts `STOPPED`. If the instance is already
running, Barista emits `WAKE_FIRED`, clears the alarm, and submits no operation.
Setting a new alarm replaces the previous one.

A firing may be replayed after a crash. Barista binds replay to one lifecycle
operation, but the workload's scheduled action should still be idempotent.

## Planned: wake on request

The product vision is that traffic addressed to a fleet session name resolves
the owner, waits while a sleeping workload restores, and forwards only after
readiness.

The gateway, bounded request parking, single-flight wake collapse, and
hibernating WebSocket behavior are **planned**, not current interfaces. Today a
caller resolves the owner with `barista fleet resolve` and invokes lifecycle
explicitly through a co-located or securely tunnelled Contract A client.

## What happens during a memory resume

Before a configured `post_restore_cmd` runs:

1. The runtime restores the snapshot into a fresh sandbox.
2. The guest agent mixes fresh host entropy and forces a kernel CRNG reseed.
3. The guest clock is stepped to host time.
4. Network reachability is rechecked and a `RESTORED` event reports drift.
5. The post-restore hook gets a chance to reopen external connections.

External sockets and provider connections are not snapshot-safe. Hooks are
Contract A fields; the current create CLI does not expose them.

## When memory cannot be restored

Without `require_memory`, a snapshot-key mismatch or unusable memory snapshot
falls back to a cold boot from the pinned image. The operation and event stream
report the degradation.

With `require_memory`, the request is refused before a partial boot and the
instance remains available for investigation or a later non-strict retry.

## Related

- [Snapshots](snapshots.md)
- [Capabilities and tiers](capabilities-and-tiers.md)
- [Best practices](../best-practices.md)
- [Networking and egress](networking-and-egress.md)
