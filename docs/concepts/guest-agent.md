# The guest agent

Every sandbox runs a small daemon that Barista injects at create time. It is how the
platform reaches inside a session without your workload linking against
anything.

You never install it, configure it, or write code against it. This page explains
what it does, because two of its duties change how you should build your image.

## What it does

| Duty | Surfaces as |
|---|---|
| Liveness and readiness | `Instance.ready`, `READY_CHANGED` events |
| Exec | `barista exec`, interactive PTY sessions |
| File transfer | `barista cp` |
| Activity tracking | TTL resets |
| Snapshot hooks | `pre_snapshot_cmd`, `post_restore_cmd` |
| Restore duties | Entropy reseed, clock step, `RESTORED` event |

## It dials out, never in

The agent is PID-1-adjacent inside the sandbox. On boot it dials the host and
authenticates with a per-session token. **It never accepts inbound
connections.** There is no port to expose and no listener to secure.

The transport differs per runtime — TCP to the guest address on the hypervisor
substrate, a unix socket under gVisor, an exec bridge on the fake runtime — and
none of that is visible in the API.

If the agent is not present, the node reports `guest_agent: false` and refuses
passthrough calls rather than pretending. A node started without a guest binary
is a node where `barista exec` returns an error, not one where it silently does
nothing.

## Readiness

`ready_cmd` runs inside the guest. Its exit status is the `ready` flag:

```yaml
process:
  start_cmd: ["/app/agent", "--serve"]
  ready_cmd: ["/app/healthcheck"]
```

Use it. It is the difference between "the sandbox booted" and "the workload can
serve", and it is also the right signal for deciding when a golden template is
warm enough to snapshot — better than a fixed settle delay, which is either too
short or wasteful.

## Restore duties

On every restore, before your `post_restore_cmd` and before the workload
observes anything:

1. **Entropy reseed.** Fresh host bytes are mixed into the kernel pool and a
   CRNG reseed is forced. Without both steps, two guests restored from the same
   snapshot draw identical "random" values — the ChaCha key and the reseed timer
   restore byte-identical, so mixing alone is not enough.
2. **Clock step.** The guest clock is set to host time. A restored guest's clock
   is frozen at the moment of the snapshot; the drift is reported on the
   `RESTORED` event.
3. **Network re-verification**, then the `RESTORED` event with its drift
   metrics.
4. **`post_restore_cmd`**, which therefore already sees fresh entropy and a
   correct clock.

Where a duty cannot run — a constrained sandbox without the capability to set
the clock, for instance — it is reported as degraded rather than reported as
success.

## Hooks are a chance, not a veto

`pre_snapshot_cmd` gets a bounded window to quiesce. If it exceeds its timeout,
**the snapshot proceeds** and the outcome is recorded in the snapshot metadata.

This is deliberate. A workload that hangs must not be able to make its own
session unpausable — the platform has to be able to snapshot an uncooperative
guest. Write your quiesce command so that being cut short is survivable.

## Related

- [Snapshots](snapshots.md)
- [Guest Agent API](../api/guest-agent.md)
- [Best practices](../best-practices.md)
