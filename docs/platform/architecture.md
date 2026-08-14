# Architecture

Barista's implemented core is a CLI/API client, one Node Agent per host, an
adopted runtime, an injected guest agent, and an optional coordination bucket.

```text
local client / CLI
        │ Contract A: gRPC on loopback or UDS
        ▼
┌──────────────────┐       optional       ┌────────────────────┐
│ Node Agent       │◀────────────────────▶│ object-store bucket │
│ journal + FSM    │   desired + leases   │ fleet coordination  │
└────────┬─────────┘                      └────────────────────┘
         │ Contract B
         ▼
┌──────────────────┐
│ runtime          │  hypeman or fake
└────────┬─────────┘
         ▼
┌──────────────────┐
│ sandbox          │
│ guest agent ─────┼── Contract C channel
│ workload         │
└──────────────────┘

planned: public gateway ── resolve(name) ── wake, wait for ready, route
```

## Node Agent

One Node Agent owns:

- the SQLite WAL operation journal and crash recovery;
- the instance state machine and one-mutation-at-a-time rule;
- TTL, scheduled wake, fleet renewal, fencing, and orphan reconciliation;
- local snapshot metadata and compatibility checks;
- runtime health and capability reporting.

Contract A is intentionally loopback-only today because it has no remote-caller
authentication. Cross-host deployments need a co-located caller or a secure
tunnel/proxy owned by the deployment.

## Runtime

Barista adopts sandbox and snapshot substrates behind Contract B. It does not
reimplement hypervisor lifecycle, memory paging, or snapshot mechanics.

| Runtime | Status | Isolation | Memory snapshot |
|---|---|---|---|
| `hypeman` | Implemented, rank 1 | Hardware microVM through KVM or Virtualization.framework | Supported on capable backends. |
| `fake` | Implemented, tooling only | Docker container | No; disk-only degradation. |
| `runsc` | Deferred rank 2 | gVisor shared kernel | Intended for live checkpoint; not implemented. |
| `process` | Design direction | Host platform | Intended disk-only tier; not implemented or measured. |

Barista owns the semantics around the substrate: readiness, hooks, restore-time
duties, compatibility keys, operation journaling, and explicit degradation.

## Guest agent

A small injected daemon provides readiness, exec, file transfer, activity
tracking, hooks, and restore duties. Connection direction is
transport-dependent: on `hypeman` it binds a listener inside the VM that the
host dials, wrapped in per-instance mutual TLS; on `fake` (and the deferred
`runsc` path) the host reaches it through the runtime's exec bridge or a unix
socket, with no inbound network port in the sandbox. The workload never links
the agent.

## Coordination bucket

Fleet mode stores two object classes:

- `desired/<name>` — serialized `InstanceSpec` plus fleet policy;
- `sessions/<name>` — current owner, epoch, endpoint, and materialised instance.

Nodes renew, self-fence, acquire, and materialise through compare-and-swap writes
with ETag/epoch fencing. Ownership survives a node-agent restart through the
local journal.

There is no control-plane service, scheduler service, or consensus cluster.
Placement is currently first successful acquisition with no capacity check. A
single node constructs no fleet module when no bucket is configured.

## The gateway layer

The gateway is not part of this repository, but it exists: the hosted control
plane ([beta.barista.sh/docs](https://beta.barista.sh/docs/)) implements this
layer — it resolves a fleet name, wakes a paused session on request, waits for
readiness, and forwards application requests to the address the node's ingress
publishes. Hibernating WebSockets remain future work. On this site the
boundary stays the same: everything below Contract A is the engine, everything
above it is a consumer.

## Contracts

| Contract | Boundary | Current consumers |
|---|---|---|
| **A — Node Agent API** | `barista.node.v1alpha1`, gRPC over loopback TCP or UDS | CLI and generated clients. |
| **B — Runtime trait** | In-process Rust interface | `hypeman` and `fake`. |
| **C — Guest Agent API** | `barista.guest.v1alpha1` over runtime transport | Node Agent only. |

The protobuf packages are the only wire-contract source. Hand-written duplicate
contract types are not supported.

## State classes

| Class | Examples | Survives through |
|---|---|---|
| Ephemeral | Live process memory | Local memory snapshots. |
| Persistent | Writable filesystem or external data | Runtime disk semantics and external storage. |
| Platform | Journal, desired state, ownership | Node data directory and optional bucket. |

## Related

- [Sessions](../concepts/sessions.md)
- [Fleet coordination](../concepts/fleet-coordination.md)
- [Capabilities and tiers](../concepts/capabilities-and-tiers.md)
- [Node Agent API](../api/index.md)
