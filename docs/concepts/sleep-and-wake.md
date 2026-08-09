# Sleep and wake

A session that is not doing anything should not cost anything, and should lose
nothing by resting. Barista owns both edges of that cycle: deciding when a session
sleeps, and bringing it back.

## Sleeping

### TTL

Set `ttl_seconds` on the session. When nothing has touched it for that long, the
platform performs `ttl_action`:

| Action | Effect |
|---|---|
| `PAUSE` (default) | Memory and disk are snapshotted; the sandbox is released. |
| `STOP` | Clean shutdown. Disk is preserved, memory is lost. |
| `DESTROY` | The session and its resources are removed. |

Activity resets the deadline. Activity means guest passthrough — `Exec`,
`ReadFile`, `WriteFile` — and any explicit lifecycle verb. You get a
`TTL_WARNING` event before the deadline fires, which is your chance to keep the
session up if it is busy in a way Barista cannot see.

### Keep-awake leases

TTL infers idleness from RPC traffic, and inference is wrong in one specific,
common case: a session waiting on a slow external call makes no RPCs and looks
idle. It is then frozen precisely when it is about to become useful, and the
caller pays a pause plus a resume for nothing.

Let the workload declare the busy period instead of hoping the heuristic holds.
The guest agent exposes a keep-awake lease on a local endpoint inside the
sandbox, so any language can take one without linking against anything:

```sh
lease=$(curl -fsS -XPOST localhost:7999/keep-awake)   # held while the work runs
run_the_model_call
curl -fsS -XDELETE "localhost:7999/keep-awake/$lease"
```

A keep-awake lease is scoped, counted, and released when its holder settles.
Two variants exist: one blocks the idle timer from arming at all, one allows the
timer but blocks the final stop so an in-flight flush can finish. It is
deliberately not a boolean flag you set and clear by hand — a flag that someone
forgets to clear wedges the session awake forever, and the counter's names show
up in the timeout log when something fails to drain.

### The freeze is real, and it scales

On a substrate without live checkpoint, a pause is stop-copy-resume. The session
is genuinely frozen while its memory is written out, at roughly **1.2–1.7
seconds per GiB of dirty memory**. A 4 GiB agent is unavailable for about five
seconds each time it idles out.

Two consequences for design:

- Keep sessions as small as their working set genuinely requires. Snapshot size
  tracks memory *entropy*, not memory size — 1.5 GiB of a real session
  compresses to about 600 MB, while 1.5 GiB of random bytes does not compress at
  all and costs several times as much to move.
- Discard rebuildable state before the snapshot, in `pre_snapshot_cmd`. A KV
  cache you can regenerate in 50 ms should not be in your snapshot.

Where a live checkpoint is available, `Checkpoint` takes a snapshot without
pausing. Where it is not, `Checkpoint` fails with `CAPABILITY_MISSING` rather
than silently pausing your session. See
[Capabilities and tiers](capabilities-and-tiers.md).

## Waking

Waking is the platform's job. In descending order of how often it happens:

### 1. On request

A request addressed to a sleeping session's name wakes it. The gateway resolves
the name to its owner, holds the client connection, triggers the restore, and
forwards the request when the workload is ready. The client sees latency, not an
error.

Concurrent requests for the same sleeping session collapse into one wake:
N callers arriving at once cost one restore, not N. The parking lot is bounded
and sheds explicitly when full, with headroom reserved so that a stampede for
one sleeping session cannot starve sessions that are already running.

WebSocket connections can be held across a pause. The runtime closes its side
marked as hibernating, the client's socket stays open and idle, and the next
client message wakes the session — which is told, on wake, which connections are
still held.

### 2. On schedule

Set an alarm and the platform wakes the session at that time, with no inbound
request at all:

```sh
barista wake-at agent-42 2026-08-09T09:00:00Z
barista wake-at agent-42 --clear
```

This is what makes "check back at 9am" a property of the session rather than a
cron job somewhere else that poke it.

One alarm per session. If you need several schedules, multiplex them yourself —
keep your own table and set the alarm to the earliest entry.

Alarm handlers must be idempotent. An alarm **may fire more than once**: a node
that crashes between "due" and "handled" replays the firing on recovery. Barista
derives the idempotency key from the alarm instance, so a replayed firing
produces one resume rather than two, but your workload should still be written
so that doing the work twice is harmless.

An alarm that fires on a session which is already running produces an event, not
an error. The state the alarm wanted is the state that exists.

### 3. On the explicit verb

```sh
barista resume agent-42
```

`Resume` is the operator's path and the machinery underneath the other two. Use
it in scripts, tests, and recovery procedures.

## What happens during a wake

In order, before your workload observes anything:

1. The snapshot is restored and the guest's memory is back.
2. The guest agent **reseeds the kernel RNG** with fresh host entropy. Without
   this, two restores of one snapshot would mint identical "random" secrets.
3. The guest agent **steps the clock** to host time. A restored guest's clock is
   frozen at the instant of the snapshot.
4. Network reachability is re-verified and a `RESTORED` event is emitted,
   carrying the measured clock drift.
5. Your `post_restore_cmd` runs, so the workload can reopen sockets and
   reconnect to providers. External connections never survive a restore.

Only then does the session serve traffic.

## When memory cannot be restored

Restore is an optimisation, never a correctness dependency. If a snapshot cannot
be used — the host's CPU class does not match, the runtime bundle changed, the
template was invalidated, the image is corrupt — Barista **cold-boots the session
from its template**, records the degradation on the operation, and emits a
`DEGRADATION` event. A bad snapshot does not take a session down.

If your workload genuinely cannot tolerate a cold boot, opt out:

```sh
barista resume agent-42 --require-memory
```

You get `FAILED_PRECONDITION` with a specific reason, and no partial boot. The
snapshot is left alone, so you can investigate and try again.

## Related

- [Snapshots](snapshots.md)
- [Networking and egress](networking-and-egress.md)
- [Best practices](../best-practices.md)
