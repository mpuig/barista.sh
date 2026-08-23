# Lifecycle and operations

Every change you request is a durable, idempotent operation. Every state a
session passes through is visible. Nothing happens that you cannot follow.

## The state machine

```
                    ┌──────────────── Checkpoint ───────────────┐
                    ▼                                           │
CREATING → CREATED → STARTING → RUNNING ──────────→ CHECKPOINTING
                        ▲          │   │
                        │          │   └─ Pause →  PAUSING → PAUSED
                        │          │                            │
                        │        Stop → STOPPING → STOPPED      │
                        │                  │                    │
                        └──── Start ───────┘        Resume → RESUMING → RUNNING

any transitional state → FAILED        any state → Destroy → DESTROYING → DESTROYED
```

| State | Means |
|---|---|
| `CREATED` | Configuration is journaled. No sandbox exists yet. |
| `RUNNING` | The sandbox is up. Check `ready` separately for whether the workload can serve. |
| `CHECKPOINTING` | Transient. A live snapshot is in progress; the workload never stopped, and the session returns to `RUNNING` by itself. |
| `PAUSED` | **Zero sandbox resources.** Only snapshot files and metadata remain. |
| `STOPPED` | Clean shutdown. Disk preserved, memory lost. `Start` from here is a cold boot. |
| `FAILED` | The failing operation is recorded. `Destroy` is always legal. |
| `DESTROYED` | Terminal. |

`STOPPED` also carries **why**: a workload that exited on its own reports its
exit code, distinctly from a session that was stopped by request. For a
cron-shaped agent that wakes, works, and exits, that reason is the result.

## Operations

Every mutating call — create, start, stop, pause, resume, checkpoint, destroy,
delete snapshot — returns an `Operation` immediately and does the work
asynchronously:

```json
{
  "op_id": "01J9Z…",
  "kind": "resume",
  "instance_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  "state": "OPERATION_STATE_RUNNING",
  "current_step": "restoring memory",
  "created_at": "2026-08-08T10:14:02Z"
}
```

Follow it with `GetOperation`, or watch the event stream. The CLI does the
latter for you: `barista resume <instance-id>` subscribes first, submits, then waits, so
an operation that finishes instantly cannot slip past you.

### Operation states

| State | Means |
|---|---|
| `QUEUED` | Journaled. Nothing has been done yet. |
| `RUNNING` | Executing, and `current_step` says where it has got to. |
| `AWAITING_INPUT` | Paused, waiting for input — typically a human's — and will carry on once it has it. `current_step` says what it is waiting for. |
| `DONE` | The work happened. |
| `FAILED` | Something went wrong. `error` carries the reason. |
| `CANCELED` | Deliberately called off, and nothing went wrong. Its result is refused — but see below: this does not mean the work was stopped. |

`AWAITING_INPUT` is **still in flight**: it holds its session, a second mutating
call is still `CONCURRENT_OPERATION`, and a node restart resolves it like any
other unfinished operation. It is reported separately from `RUNNING` because a
wait for a person is unbounded — a timeout tuned to how long a substrate takes
would fire on operations that are behaving perfectly.

`CANCELED` is reported separately from `FAILED` because a cancellation carries no
`error`: it needs no retry, no alert, and no bug report. The CLI exits `7` for
one — non-zero, because the operation did not deliver, but distinct from every
failure. The reason it was called off is on the operation's journal row and on the
`OPERATION_PROGRESS` event, not in `error`.

#### Cancelling records an outcome; it does not stop the work

`CancelOperation` calls an operation off. What that buys is precise, and it is
worth reading before relying on it:

- **What it does.** The operation is recorded `CANCELED`, and its result is
  refused — the executor's finalization cannot overwrite the outcome you were
  given, and cannot advance the session on the strength of it.
- **What it does not do.** It does not interrupt work already under way. A
  substrate call in flight runs to completion, and its side effect may land after
  the cancellation is recorded. Cancelling a `stop` does not keep the workload
  running.
- **What it leaves behind.** A cancellation does not move the session, so one
  whose operation was cancelled mid-flight stays in the transitional state its
  submission recorded — `STOPPING`, `PAUSING`, `RESUMING`. Nothing converges that
  while the node is up; it resolves on a restart, or you can `destroy` the
  session, which is legal from any state.

So a cancelled operation means "this operation's answer will not be used", not
"nothing happened". If you need the second, there is no verb for it yet.

### Journaled before anything happens

An operation is written to the node's journal — SQLite in WAL mode — **before**
any side effect starts. If the node is `kill -9`ed mid-flight, the journal is
replayed on restart and every operation either resumes from its last durable
step or is marked `FAILED` with its cleanup executed.

The invariant this protects is worth stating plainly: **no orphan sandboxes, and
no half-created sessions invisible to the API.** Reconciliation sweeps
everything the platform created for a session — sandboxes, volumes, credentials
— not just the parts that are easy to enumerate.

### Idempotency

Every mutating Contract A call takes an `idempotency_key`. Replaying an API
request with the same key returns the original operation instead of doing the
work again.

The CLI generates a fresh key per invocation and does not expose it. Two
separate `barista stop` commands are therefore two intentions. An API client
retrying one timed-out request should retain and reuse its key.

### One at a time

One in-flight mutating operation per session. A conflicting call fails with
`CONCURRENT_OPERATION` rather than interleaving with the one in progress.
`Destroy` may cancel what is running.

## Events

`WatchEvents` streams everything the node does, with a monotonic cursor per
node:

| Event | Fires when |
|---|---|
| `STATE_CHANGED` | A session enters a new state. |
| `OPERATION_PROGRESS` | An operation advances a step. |
| `READY_CHANGED` | `ready_cmd`'s verdict flips. |
| `TTL_WARNING` | A TTL deadline is approaching. |
| `DEGRADATION` | Anything was downgraded — a disk-only snapshot, a cold-boot fallback, a duty that could not run. |
| `WAKE_FIRED` | A scheduled alarm fired. |
| `RESTORED` | Post-restore duties are complete, with clock-drift metrics. |
| `FENCED` | This node lost a fleet lease and is stopping the superseded workload. |

```sh
barista events                          # everything on this node
barista events --instance <instance-id> # one direct-node instance
barista events --from-cursor 41827      # replay from where you stopped
```

Hold the cursor and you can resume the stream after a disconnect without missing
anything. The journal has a retention window: a cursor older than its floor is
refused with `CURSOR_TOO_OLD` rather than served a stream that silently skips
deleted events. Resynchronise with `ListInstances` and carry on from the current
cursor.

`DEGRADATION` is the event to alert on. It is how Barista tells you that something
worked, but not the way you asked.

## Related

- [Errors](../api/errors.md)
- [Node Agent API](../api/index.md)
- [Capabilities and tiers](capabilities-and-tiers.md)
