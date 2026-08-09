# Known issues

What does not work yet, what is degraded, and what to do instead.

## macOS host-to-session networking

**Impact:** anything that reaches into a session over the network on a macOS
host — including the end-to-end agent scenario.

The substrate assigns guests a subnet the macOS host cannot reach (upstream
`hypeman` #358). Memory pause and resume themselves work on Apple Silicon; only
reachability is affected.

**Workaround:** run the node inside a Linux VM. A Lima configuration is provided
at `.tools/nap-linux.yaml`.

## Live checkpoint

**Impact:** `Checkpoint` is unavailable on the default runtime.

The hypervisor substrate has no live checkpoint — its snapshot-from-running is
pause-copy-resume. Rather than pause your session and call the result a
checkpoint, `CheckpointInstance` fails with `CAPABILITY_MISSING`.

**Workaround:** use `CreateSnapshot`, which does the same capture and *declares*
the brief freeze (`froze_workload: true`). Or run the gVisor runtime, where live
checkpoint is available.

## Tier C is unmeasured

**Impact:** serverless container platforms — Fargate, Azure Container Apps.

The `process` runtime delegates isolation to the host and reports `DISK_ONLY`
honestly. What has not been established is whether a sandboxed runtime can run
in those environments at all, and where snapshots live when the task host is
itself reclaimable.

**Workaround:** treat tier C as disk-only, and keep the snapshot tier on storage
that outlives the task.

## Object-store snapshot tier

**Impact:** losing a node loses its local snapshots.

The remote snapshot tier — which survives node loss and enables cross-host
migration — is demand-driven rather than on the critical path. Locality plus the
local tier covers the current consumers.

Today, when a node dies, another node acquires its session names and cold-boots
them from their desired specs with a loud degradation event. You lose the warm
memory, not the sessions.

## Substrate upgrades invalidate snapshots

**Impact:** fleet-wide loss of warm state, one time, per upgrade.

`runtime_bundle_ref` pins the runtime and guest-agent versions. Upgrading either
one means existing memory snapshots no longer match, and affected sessions cold
boot with a `DEGRADATION` event.

This is the keying working correctly — restoring memory across a substrate
version is how you get subtle corruption. Drain or recapture deliberately rather
than discovering it during an unrelated deploy.

## Degraded runtimes

| Runtime | Degradation | Reported as |
|---|---|---|
| `fake` | No memory snapshots; every pause is `DISK_ONLY`; TTL `PAUSE` falls back to `STOP` | `memory_snapshot: false` |
| `fake` | No hardware isolation | `hardware_isolation: false` |
| `fake`, `process` | No mediated egress | `egress_control: false` |
| Any, without `--guest-bin` | No exec, file transfer, readiness, or hooks | `guest_agent: false` |

None of these are silent. A request that needs a missing capability fails with
`CAPABILITY_MISSING`; a request that can be served more weakly returns a result
that says so.

## Reporting

Check upstream findings in `docs/upstream-hypeman-findings.md` and
`docs/upstream-issues/` before filing a substrate-level problem — several are
already tracked there with reproductions.

## Related

- [Capabilities and tiers](../concepts/capabilities-and-tiers.md)
- [Local development](../local-development.md)
- [Errors](../api/errors.md)
