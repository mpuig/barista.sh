# Capabilities and tiers

One API can describe hosts with different guarantees, but a runtime may claim
only what its active backend can deliver. Capability discovery is how Barista
keeps those differences out of `InstanceSpec` without hiding them.

## Inspect a node

```sh
barista node info
```

Human output has this shape:

```text
node       01J9Z…
arch       aarch64
cpu class  4f2a9c11
agent      0.1.0

runtime    hypeman 0.17.0
substrate  HEALTHY
can        memory-snapshot, disk-snapshot, guest-agent, hardware-isolation
```

Use `--json` for the full machine-readable capability object.

| Capability | Meaning |
|---|---|
| `memory_snapshot` | Pause can preserve guest RAM rather than only disk. |
| `disk_snapshot` | The writable filesystem state can survive. |
| `live_checkpoint` | Capture without freezing the workload. |
| `guest_agent` | Exec, file transfer, readiness, hooks, and restore duties are available. |
| `hardware_isolation` | The workload has its own kernel boundary. |
| `lazy_restore` | Memory pages can be loaded on demand. |
| `cow_fork` | The substrate can fork copy-on-write from a snapshot. |
| `egress_control` | The substrate can enforce the mediated egress contract. |

`barista doctor` is stricter than `node info`: doctor exits non-zero when memory
snapshots are absent because it is the readiness gate for session continuity.

## Requiring a guarantee

```sh
barista create \
  --image ghcr.io/acme/eval:2026-08 \
  --digest sha256:… \
  --require-hardware-isolation \
  -- /bin/eval

barista pause <instance-id> --require-memory
barista resume <instance-id> --require-memory
```

A missing required capability fails with `CAPABILITY_MISSING` before a weaker
result can pass as success.

## Implemented runtimes

| Runtime | Status | Isolation | Memory snapshot | Intended use |
|---|---|---|---|---|
| `hypeman` | Implemented, rank 1 | Hardware microVM | Yes on supported KVM/`vz` backends | Real session pause/resume. |
| `fake` | Implemented, tooling only | Docker container | No | API and lifecycle development. |

`hypeman` reports its active backend and architecture, not the substrate's best
case. It does not report live checkpoint. `fake` reports `DISK_ONLY` and never
serves as snapshot evidence.

## Deferred runtimes

| Runtime | Status | Intended role |
|---|---|---|
| `runsc` | Deferred rank 2 | gVisor shared-kernel density and live checkpoint, gated by T11. |
| `process` | Design direction only | Delegate isolation to a serverless container host and report disk-only behavior. |

Neither value is accepted by `barista-node-agent --runtime` today.

## Deployment tiers

| Tier | Current position |
|---|---|
| Full host control—bare metal, droplets, supported Apple Silicon | `hypeman` provides the implemented memory tier, subject to backend limitations. |
| Kubernetes nodes with host access | Run `hypeman` where `/dev/kvm` is available; the planned `runsc` tier is not implemented. |
| Serverless containers without node/device access | No production runtime is implemented; the planned `process` tier is unmeasured. |

The API shape remains stable across tiers. Availability does not.

## Substrate outages

When the substrate stops answering, the node reports
`SUBSTRATE_HEALTH_UNREACHABLE` and refuses new mutations with
`SUBSTRATE_UNAVAILABLE`.

It does not infer that running workloads died. Existing instances keep their
reported state, and reconciliation does not destroy or reacquire them on the
strength of a control-path outage.

## Related

- [Errors](../api/errors.md)
- [Known issues](../platform/known-issues.md)
- [Local development](../local-development.md)
