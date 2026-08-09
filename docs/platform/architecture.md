# Architecture

Barista is two binaries and a bucket.

```
        ┌──────────┐
client ─┤ gateway  ├─ resolve(name) ──▶ ┌────────────────┐
        └──────────┘                    │ object bucket  │  desired/<name>
                                        │ (yours)        │  leases/<name>
                                        └───────┬────────┘
                                                │ CAS + ETag fencing
                    ┌───────────────────────────┴───────────────────────────┐
                    │                                                       │
            ┌───────▼────────┐                                     ┌────────▼───────┐
            │  Node Agent    │  journal (SQLite WAL)               │  Node Agent    │
            │                │  local snapshot tier                │                │
            │  ┌──────────┐  │                                     │                │
            │  │ runtime  │  │  hypeman · runsc · process · fake    │                │
            │  └────┬─────┘  │                                     │                │
            └───────┼────────┘                                     └────────────────┘
                    │
          ┌─────────▼─────────┐
          │  sandbox          │
          │   guest agent ────┼──▶ dials out, authenticated
          │   your workload   │
          └───────────────────┘
```

## Node Agent

One per host. It owns:

- **The operation journal** — SQLite in WAL mode. Every mutation is written
  before any side effect starts, so a `kill -9` at any point recovers
  deterministically with no orphan sandboxes and no half-created sessions.
- **The state machine** — one in-flight mutating operation per session.
- **The reconciler** — TTL deadlines, wake alarms, lease renewal, orphan sweeps.
- **The local snapshot tier** — snapshot files and their restore keys.
- **Capability reporting** — what this host can actually do, and why.

It is an ordinary container or binary. No CRDs, no operator, no DaemonSet
requirement, no cluster primitives. Privilege buys a better tier; it is not
required to start.

## Runtime

A pluggable layer behind the Node Agent. Barista **adopts** a substrate rather than
building one: hypervisor lifecycle, snapshot mechanics, and memory paging are
not reimplemented.

| Runtime | Substrate | Isolation | Memory snapshot |
|---|---|---|---|
| `hypeman` | microVM (KVM, Virtualization.framework) | Hardware | ✓ |
| `runsc` | gVisor | Shared kernel | ✓ |
| `process` | The host platform | Delegated | ✗ |
| `fake` | Docker | Container | ✗ |

What stays Barista's, on every runtime: readiness probes, snapshot hooks,
restore-time duties, the journal and crash-recovery model, restore-compatibility
keying, and session semantics.

## Guest agent

A small daemon inside every sandbox, injected at create. It dials out and
authenticates with a per-session token, and never accepts inbound connections.
See [The guest agent](../concepts/guest-agent.md).

## The bucket

Coordination and addressing, in one place. `desired/<name>` holds the spec;
`leases/<name>` holds the current owner and epoch. Nodes acquire with
compare-and-swap writes fenced by ETag and epoch, then pull what they acquired.

There is **no control-plane service and no consensus cluster**. Inventory is a
prefix listing. Placement is a rule nodes apply when acquiring, not a component
that assigns.

A single node runs with no bucket at all.

## Gateway

Resolves a name to its owner, routes to it, and holds the request while a
sleeping session restores. Concurrent wakes for one session collapse into one
restore; the parking lot is bounded and sheds explicitly, with headroom reserved
so a stampede cannot starve running sessions. WebSocket connections can be held
across a pause.

## Contracts

Three, all schema-first. The protobuf packages are the only source of truth;
hand-written duplicates of contract types are not supported.

| Contract | Boundary | Consumers |
|---|---|---|
| **A — Node Agent API** | gRPC over TCP or UDS (`barista.node.v1alpha1`) | CLI, gateway, your code |
| **B — Runtime** | Rust trait, in-process | The runtime implementations |
| **C — Guest Agent API** | gRPC over a per-runtime transport (`barista.guest.v1alpha1`) | Node Agent only |

## State classes

Three, managed differently:

| Class | What | Survives via |
|---|---|---|
| Ephemeral | The session's live memory | Memory snapshots |
| Persistent | Files, volumes, databases | Independent of the session lifecycle |
| Platform | Ownership, desired state, inventory | The bucket, and each node's journal |

## Related

- [Fleet coordination](../concepts/fleet-coordination.md)
- [Capabilities and tiers](../concepts/capabilities-and-tiers.md)
- [Node Agent API](../api/index.md)
