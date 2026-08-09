# Networking and egress

How traffic reaches a session, and what the session is allowed to reach.

## Reaching a session

A session is addressed by name. The gateway resolves the name to its owning
node, and forwards:

```
client ──▶ gateway ──▶ resolve(name) ──▶ owning node ──▶ session
                            │
                            └── asleep? hold the request, restore, then forward
```

Three properties matter to callers:

- **The address is stable across a pause.** The name does not change when the
  session sleeps, moves, or is restored on another host.
- **A request to a sleeping session is latency, not an error.** The gateway
  parks the request while the session restores. See
  [Sleep and wake](sleep-and-wake.md#1-on-request).
- **Sessions never accept inbound connections from the platform.** The guest
  agent dials out and authenticates with a per-session token. Anything reaching
  the workload comes through the gateway.

### WebSockets across a pause

A session can hibernate while holding WebSocket connections. The runtime closes
its side marked as hibernating; the client's socket stays open and idle; the
next client message wakes the session, which is told on wake which connections
are still held.

To the client, an hour-long pause looks like an hour of nobody typing.

## Controlling egress

Agent workloads run code you did not write. Declare what the session may reach:

```sh
barista create agent-42 --egress http-https-only --image … -- /app/agent
```

| Mode | Effect |
|---|---|
| unset | Unrestricted outbound. |
| `all` | All outbound traffic is mediated by the host. |
| `http-https-only` | Only HTTP and HTTPS may leave, and only through the mediated path. Direct TCP to port 443 does not work. |

Enforcement happens in the substrate, at the host boundary. No packet is
inspected by Barista itself.

Mediation is a capability like any other. A runtime that cannot enforce it fails
`CreateInstance` with `CAPABILITY_MISSING`. You never get a sandbox that quietly
came up with open egress because the policy could not be applied.

### Credential brokering

On the mediated path, the workload never holds real credentials. The guest sees
placeholders; the host injects the real credential per destination host, on the
way out.

This matters more for agent workloads than mode enforcement does: an agent that
can be talked into printing its environment cannot print a key it never had.

## Identity

Per-session identity is delivered as a **file**, never as an environment
variable.

The reason is specific to memory snapshots: anything in the environment at
snapshot time is frozen into the captured memory and comes back byte-identical
for every session restored from it. A golden template whose identity lives in
`env` hands the same identity to every session forked from it. A file is read
after restore, so it can be different each time.

## Related

- [Sleep and wake](sleep-and-wake.md)
- [The guest agent](guest-agent.md)
- [Best practices](../best-practices.md)
