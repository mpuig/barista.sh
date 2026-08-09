# Known issues

Current limitations, their observable effect, and the available fallback.

## macOS host-to-guest networking

**Impact:** exec, file transfer, readiness, hooks, and the end-to-end agent
scenario from a macOS host.

The `vz` backend can preserve memory on Apple Silicon, but the substrate guest
subnet is not reachable from the host (`hypeman` #358).

**Workaround:** run the node in Linux. The repository includes
`.tools/nap-linux.yaml` for Lima.

## No live checkpoint runtime

**Impact:** `barista checkpoint` is unavailable on both selectable runtimes.

`hypeman` captures a running guest through pause-copy-resume, and `fake` has no
memory capture. Both report `live_checkpoint: false`, so `CheckpointInstance`
fails with `CAPABILITY_MISSING` rather than freezing and calling the result live.

**Workaround:** use `barista snapshot create` on `hypeman`; inspect
`froze_workload` because a running source freezes briefly. The deferred `runsc`
tier is intended to carry live checkpoint after its compatibility gate.

## Mediated egress is not proven

**Impact:** either current runtime refuses `--egress mediated...`.

The contract and capability gate exist, but substrate enforcement has not been
established. `hypeman` and `fake` report `egress_control: false`; create fails
with `CAPABILITY_MISSING` rather than starting unrestricted.

**Workaround:** omit the policy only when the runtime's default network is
acceptable, or enforce isolation outside Barista. Do not interpret omission as a
Barista egress policy.

## No request gateway

**Impact:** no transparent wake on HTTP/WebSocket traffic and no Barista-managed
public ingress.

Fleet names can be applied and resolved, but request parking, readiness-aware
forwarding, and hibernating connections are planned work.

**Workaround:** use explicit or scheduled resume, then connect through a
co-located client or deployment-owned secure tunnel/proxy.

## Contract A is loopback-only

**Impact:** a Node Agent cannot directly expose its unauthenticated gRPC API on a
remote interface.

This is deliberate. Remote caller authentication has not shipped.

**Workaround:** co-locate the caller, use a Unix socket, or provide a secure
tunnel/proxy at the deployment boundary.

## Deferred runtime tiers

`runsc` and `process` appear in platform design but are not accepted by
`barista-node-agent --runtime`. Serverless container environments therefore have
no production runtime today, and the gVisor live-checkpoint tier remains gated by
T11.

## Local-only memory snapshots

**Impact:** losing a node loses its warm memory state.

The fleet bucket stores desired sessions and leases, not memory snapshots. A new
owner can cold-boot from desired state with a degradation event when
`on_owner_loss=coldboot`, or hold without materialising when policy is `hold`.

The object-store memory tier and warm cross-host migration are planned.

## Substrate upgrades invalidate snapshots

`runtime_bundle_ref` is a restore compatibility key. Changing the runtime or
guest-agent bundle invalidates existing memory snapshots, causing an explicit
cold-boot fallback unless `require_memory` was set.

Drain or recapture deliberately before an upgrade when warm state matters.

## Tooling runtime degradation

| Runtime/setup | Limitation | Reported as |
|---|---|---|
| `fake` | No memory snapshots or hardware isolation | `memory_snapshot: false`, `hardware_isolation: false` |
| `fake` | Direct pause is disk-only; TTL pause falls back to stop | `DISK_ONLY` plus degradation event |
| Either current runtime | No proven mediated egress | `egress_control: false` |
| `fake` without `--guest-bin` | No exec, copy, readiness, or hooks | `guest_agent: false` |

`barista doctor` exits non-zero on disk-only nodes. Use `barista node info` when
you deliberately want capability inventory rather than a session-readiness gate.

## Upstream reports

See [`../upstream-hypeman-findings.md`](../upstream-hypeman-findings.md) and the
[`../upstream-issues/`](../upstream-issues/README.md) filing drafts before opening a new
substrate report.

## Related

- [Capabilities and tiers](../concepts/capabilities-and-tiers.md)
- [Local development](../local-development.md)
- [Errors](../api/errors.md)
