# The guest agent

Every sandbox runs a small daemon that Barista injects at create time. It is how the
platform reaches inside a session without your workload linking against
anything.

You never install or configure it, and with one optional exception (the idle
declaration below) you never write code against it. This page explains what it
does, because a few of its duties change how you should build your image.

## What it does

| Duty | Surfaces as |
|---|---|
| Liveness and readiness | `Instance.ready`, `READY_CHANGED` events |
| Exec | `barista exec`, interactive PTY sessions |
| File transfer | `barista cp` |
| Activity tracking | TTL resets |
| Snapshot hooks | `pre_snapshot_cmd`, `post_restore_cmd` |
| Restore duties | Entropy reseed, clock step, `RESTORED` event |
| Idle declaration | `WorkloadService.DeclareIdle`, `IDLE_FIRED` events |

## Who connects to whom depends on the transport

The transport is runtime-specific and hidden behind Contract C, and so is the
connection direction:

- On **`hypeman`** the sandbox is a VM with an address. The guest agent binds a
  TCP listener inside the VM (port 7071) and the **host dials in**. Because
  every sibling VM on the host shares the substrate's `default` network, that
  port is reachable by other sandboxes on the same machine — which is exactly
  why the channel authenticates with a per-session token and is wrapped in
  per-instance mutual TLS (barista-021/032). The network listener is off
  entirely unless the runtime asks for it (`BARISTA_GUEST_TCP_PORT`); the agent
  always also serves its in-sandbox unix socket.
- On **`fake`** (and the deferred `runsc` path, through a bind-mounted unix
  socket) the host reaches the agent over the substrate's exec bridge, so no
  inbound network listener exists in the sandbox at all.

Either way the workload never links against the agent, and the channel is
authenticated with per-instance material. If the agent is not present, the node
reports `guest_agent: false` and refuses passthrough rather than pretending. A
node started without a guest binary is a node where `barista exec` returns an
error, not one where it silently does nothing.

## Readiness

`ready_cmd` runs inside the guest. Its exit status is the `ready` flag:

```yaml
process:
  start_cmd: ["/app/agent", "--serve"]
  ready_cmd: ["/app/healthcheck"]
```

Use it when driving Contract A directly. It is the difference between "the
sandbox booted" and "the workload can serve", and it is a better capture signal
than a fixed delay. The current create CLI does not expose `ready_cmd`; configure
it through a generated API client.

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

## Declaring idle

This is the one surface your workload may call. When it knows it has no work in
flight — an agent turn finished, a request handler returned — it can say so, and
an instance created with `--idle-action` is paused (or stopped/destroyed, per
policy) without waiting out a TTL. See [Sleep and wake](sleep-and-wake.md) for
the action and its guards.

The agent injects the socket path as `BARISTA_WORKLOAD_SOCKET` when it spawns
your `start_cmd`. The whole client is one gRPC call — `grpcurl` suffices, or run
the agent binary that already ships in the sandbox:

```sh
[ -n "$BARISTA_WORKLOAD_SOCKET" ] && barista-guest-agent declare-idle
```

The socket is unauthenticated: it is reachable only from inside your sandbox,
whose single trust domain your workload already shares. It serves *only*
`DeclareIdle` — the management RPCs (exec, files) are not reachable on it.

**If `$BARISTA_WORKLOAD_SOCKET` is unset, the surface is unsupported** — a
sandbox whose agent predates it. Treat its absence as "hints unavailable", not
an error, and fall back to a TTL.

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
