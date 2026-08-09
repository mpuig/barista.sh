# Capabilities and tiers

Barista runs on hosts that can do very different things — a bare-metal box with KVM,
a managed Kubernetes node pool, a serverless container with no device access.
One API spans all of them, and the difference lives in **capability discovery**,
never in your session spec.

The rule: **degradation is always explicit.** A runtime that cannot do what you
asked says so. It never does something weaker and reports success.

## Ask the node

```sh
barista node info
```

```
node    01J9Z…  aarch64  cpu_class 4f2a9c11
runtime hypeman 0.17.0   healthy
        memory_snapshot     ✓      hardware_isolation  ✓
        disk_snapshot       ✓      lazy_restore        ✓
        live_checkpoint     ✗      cow_fork            ✓
        guest_agent         ✓      egress_control      ✓
resources  32 vCPU  128 GiB  (allocatable: 28 vCPU, 112 GiB)
fleet      bucket configured, 14 leases held
```

| Capability | Means |
|---|---|
| `memory_snapshot` | A pause keeps guest memory. Without it, every pause is `DISK_ONLY`. |
| `disk_snapshot` | The writable filesystem layer can be captured. Every runtime has this. |
| `live_checkpoint` | A snapshot can be taken without pausing the workload. |
| `guest_agent` | Exec, file transfer, readiness probes, and hooks are available. |
| `hardware_isolation` | Workloads run in their own kernel, not a shared one. |
| `lazy_restore` | Memory pages fault in on demand rather than being read up front. |
| `cow_fork` | A session can be forked copy-on-write from a snapshot. |
| `egress_control` | Outbound traffic can be mediated by the host. |

## Requiring a capability

Two flags turn a silent downgrade into a loud refusal:

```sh
barista create untrusted-eval --require-hardware-isolation --image … -- /bin/eval
barista pause agent-42 --require-memory
barista resume agent-42 --require-memory
```

- `--require-hardware-isolation` — a node whose runtime cannot honour it fails
  with `CAPABILITY_MISSING` and **never creates the session**, rather than
  placing untrusted code on a shared kernel.
- `--require-memory` on pause — fail rather than accept a snapshot that could
  not keep memory.
- `--require-memory` on resume — fail rather than cold-boot when the memory
  cannot be restored.

Hardware isolation is not a degradation, it is a user-visible property. Ask for
it when the workload's tenancy demands it, and leave it off when it does not.

## Deployment tiers

Which capabilities you get depends on how much of the host you control.

| Tier | Hosts | Runtime | Best snapshot |
|---|---|---|---|
| **A — full host control** | Bare metal, droplets, macOS/Apple Silicon | `hypeman` (KVM or Virtualization.framework) | `MEMORY_AND_DISK` + hardware isolation |
| **B — cluster with node access** | AKS, EKS, OpenShift | `hypeman` on node pools with `/dev/kvm`; `runsc` via a privileged DaemonSet | `MEMORY_AND_DISK`, shared kernel |
| **C — serverless containers** | Fargate, Azure Container Apps | `process` — no sandbox of Barista's own, isolation delegated to the host platform | `DISK_ONLY` |

Three things hold across all three:

1. **The Node Agent is an ordinary container.** No CRDs, no operator, no cluster
   primitives. Privilege buys you a better tier; it is not required to start.
2. **One spec everywhere.** The same `InstanceSpec` is valid on every tier. What
   changes is what the node reports back.
3. **You learn your tier before you commit.** `barista doctor` and `barista node info`
   report the tier this host granted and why, at deploy time — not at your first
   pause.

## Runtimes

| Runtime | Isolation | Memory snapshot | Use |
|---|---|---|---|
| `hypeman` | Hardware (microVM) | ✓ | The default. Any OCI image runs as a VM. |
| `runsc` | Shared kernel (gVisor) | ✓ | Where density matters more than hardware isolation, or where you need live checkpoint. |
| `process` | Delegated to the host | ✗ | Tier C, where no sandbox can be created. Reports `DISK_ONLY` honestly. |
| `fake` | Docker | ✗ | **Tooling only.** For developing against the API on a laptop. Never a reference for snapshot semantics. |

## Substrate outages

If the runtime's substrate stops answering, the node reports
`SUBSTRATE_HEALTH_UNREACHABLE` and refuses new mutations with
`SUBSTRATE_UNAVAILABLE`.

It does **not** conclude that anything died. Sessions that are running keep
running and keep being reported as `RUNNING`; nothing is released, destroyed, or
reacquired on the strength of an outage. A blip in the control path is not
evidence about the data path.

## Related

- [Errors](../api/errors.md)
- [Fleet coordination](fleet-coordination.md)
- [Known issues](../platform/known-issues.md)
