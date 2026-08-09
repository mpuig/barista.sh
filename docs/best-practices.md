# Best practices

How to build images, hooks, and callers so that a pause is invisible to your
users.

## Build the image

**Pin the digest.** Barista requires it because a mutable tag cannot identify
the root filesystem captured with memory. The digest participates in the
template hash, so changed image bytes invalidate an incompatible restore.

**Keep the working set honest.** Snapshot size and pause cost track *dirty*
memory, and a pause freezes the session at roughly 1.2–1.7 s per GiB. Ask for
the memory the workload needs, not a round number above it.

**Do not put identity in the environment.** Anything in `env` at snapshot time
is frozen into captured memory and returns unchanged on restore. Read mutable
identity from a file or another post-restore source.

**Write a `ready_cmd`.** It is the difference between "the sandbox booted" and
"the workload can serve", and it is a better capture trigger than a fixed settle
delay. Readiness and hooks are `InstanceSpec` fields available through Contract
A; the current `barista create` command does not expose them as flags.

## Write the hooks

**`pre_snapshot_cmd` — drop what you can rebuild.**

```sh
#!/bin/sh
curl -fsS localhost:8080/admin/flush        # flush buffers to disk
curl -fsS localhost:8080/admin/drop-cache   # discard the rebuildable KV cache
```

Snapshotting a cache you can regenerate in 50 ms buys you nothing and costs you
seconds of freeze on every pause.

**`post_restore_cmd` — reconnect everything external.**

```sh
#!/bin/sh
curl -fsS localhost:8080/admin/reconnect    # reopen provider sockets, re-register
```

Sockets, database connections, and file descriptors to external resources never
survive a restore. Entropy and clock are handled for you before this runs.

**Assume the quiesce hook can be cut short.** It is a chance, not a veto: if it
times out, the snapshot proceeds anyway. Make partial quiescing survivable.

**Keep hooks fast and idempotent.** They run on a latency path that a user is
waiting on.

## Design the session

**One workload per session.** If you need a second component, make it a second
session and address it by name. Two processes in one session pause and resume as
one memory image; two sessions do not. Choose that deliberately.

**Budget TTL around invisible work.** A session waiting on a slow model call
makes no passthrough RPCs and looks idle to TTL. Keep-awake leases are planned,
not implemented; today, use a longer TTL or set no TTL while the caller owns the
busy period.

**Set a TTL.** A session with no TTL is a session you are paying for forever.
Start at 5–15 minutes and adjust from measurement.

**Use `require_memory` when a cold boot would be wrong.** The default is a
cold-boot fallback with a degradation event, which is right for most workloads
and wrong for a few. Say which you are.

## Write the caller

**Watch events; do not poll.** `WatchEvents` with a cursor gives you every state
change, every operation step, and every degradation. Polling `GetInstance` gives
you a subset, later.

**Alert on `DEGRADATION`.** It is how Barista tells you something worked, but not
the way you asked. A fleet quietly cold-booting every resume looks healthy on
every other signal.

**Wait for `ready`, not for `RUNNING`.** An instance whose workload has not
finished scheduling is not a resumed session. Timing a resume to `RUNNING`
flatters the number by about a third and hands users a session that is not
there yet.

**Reuse the idempotency key when retrying a timed-out API call.** Use a fresh
key for a new intention. The CLI generates a fresh key per invocation and does
not expose it, so rerunning a CLI command is a new intention rather than a replay.

**Handle `SUBSTRATE_UNAVAILABLE` as "retry later", not as "gone".** It says
nothing about whether your session exists. Sessions that were running are still
running.

**Make alarm work idempotent.** A scheduled wake may fire more than once.

## Operate the fleet

**Give each node's data directory durable local storage.** It holds the
operation journal and the local snapshot tier. Losing it loses memory, not
sessions.

**Expect a node-local pause to pin the next resume to that node.** That is
locality working, not the scheduler being unhelpful. Plan capacity per node with
paused sessions in mind — they cost roughly 0.4 bytes of disk per byte of live
memory, and no CPU or RAM. Note that acquisition has **no capacity check** of
its own today: a node attempts every unowned name it sees, so per-node capacity
is yours to plan rather than the platform's to enforce.

**Upgrade the substrate deliberately.** It changes `runtime_bundle_ref` and
invalidates existing memory snapshots. Sessions cold-boot with a degradation
event rather than failing, but that is a fleet-wide loss of warm state — drain
or recapture on purpose rather than discovering it on a Tuesday.

**Run `barista doctor` in your deploy pipeline.** Discovering at deploy time that a
host only grants the disk-only tier is much cheaper than discovering it at the
first pause.

## Related

- [Concepts](concepts/index.md)
- [Examples](examples/index.md)
- [Limits and performance](platform/limits.md)
