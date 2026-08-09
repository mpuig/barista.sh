# Local development

Run a node locally, develop against Contract A, and keep the tooling runtime
separate from real snapshot semantics.

## Choose a runtime

| Runtime | Where | Memory snapshots | Use it for |
|---|---|---|---|
| `hypeman` | Linux with `/dev/kvm`; Apple Silicon with limitations | Yes on supported backends | Pause, resume, restore duties, and snapshot measurements. |
| `fake` | Anywhere Docker runs | No | API shape, CLI behavior, lifecycle logic, and events. |

Only these two runtimes are implemented. `runsc` and `process` are deferred
tiers, not selectable node-agent values.

## Start a fake node

```sh
barista-node-agent \
  --data-dir ./.barista \
  --listen 127.0.0.1:7070 \
  --runtime fake
```

Point the CLI at it:

```sh
export BARISTA_NODE=127.0.0.1:7070
barista node info
```

`barista doctor` intentionally fails on `fake`: doctor is the deployment gate
for memory-preserving sessions, while `node info` is capability inventory.

The fake runtime is tooling only. It reports `memory_snapshot: false`; a direct
pause records `DISK_ONLY`, and resuming cold-boots. A TTL whose action is
`PAUSE` falls back to `STOP` with a degradation event.

## Start a memory-capable node

```sh
export BARISTA_HYPEMAN_TOKEN_FILE=/path/to/hypeman-token

barista-node-agent \
  --data-dir ./.barista \
  --listen 127.0.0.1:7070 \
  --runtime hypeman \
  --hypervisor cloud-hypervisor \
  --guest-bin .tools/guest/barista-guest-agent
```

Use `vz` on macOS, or `cloud-hypervisor`/`firecracker` on Linux. The node refuses
to construct the `hypeman` runtime without `--guest-bin` because the guest binary
must be delivered as a substrate volume.

## Build the guest agent

The guest agent is a static Linux/musl binary even when the developer host is
macOS:

```sh
task guest-bin
```

The result is cached at `.tools/guest/barista-guest-agent`. Tests requiring the
binary skip with an explicit reason when it is absent.

## macOS limitations

Apple Silicon with the `vz` backend can preserve memory. The upstream guest
network remains unreachable from the macOS host (`hypeman` #358), so exec, file
transfer, readiness, and the end-to-end agent scenario need a Linux host.

The repository includes a Lima configuration for that path:

```sh
limactl start .tools/nap-linux.yaml
```

See [Known issues](platform/known-issues.md) before treating a macOS pause as an
end-to-end session test.

## Tests

The Node Agent suite uses `fake` by default:

```sh
cargo test -p barista-node-agent
```

Select the adopted substrate explicitly for tests that need memory:

```sh
BARISTA_TEST_RUNTIME=hypeman cargo test -p barista-node-agent
```

A capability-dependent test skips with a reason when the selected runtime lacks
that capability. A green fake-runtime run is not snapshot evidence.

The repository gate is:

```sh
make check
```

### CI does not replace the local VM

`scripts/check_skips.sh` fails CI on any skip outside an allowlist, and that
allowlist deliberately permits `hypeman-api not reachable` and `no hypeman
token`. So **a green CI run means "everything the `fake` tier can prove,
passed"** — the rank-1 substrate's absence is a permitted skip, not a failure.
Everything needing `hypeman` — T3, T7, T8, T9, T12's positive case, the whole
`hypeman_runtime` suite, the guest channel's mutual TLS — runs nowhere but a
machine with a live substrate.

Whether CI *could* host one was measured rather than assumed
(`kvm-probe` workflow, 2026-08-09, since deleted):

| | `ubuntu-latest` (x86_64) | `ubuntu-24.04-arm` |
|---|---|---|
| `/dev/kvm` exists | yes | **no** |
| writable without a udev rule | no | — |
| writable after GitHub's udev rule | yes | — |
| CPU virtualisation extensions (`kvm-ok`) | yes | — |
| a guest boots under `-accel kvm` | **yes** | — |

So a hosted x86_64 runner can host a hypervisor, given the documented udev rule;
a hosted arm64 runner has no KVM device at all.

Two consequences worth stating before anyone acts on the first column. A CI
substrate would be **x86_64**, while every measurement this project has taken —
restore latency, the ADR-001 evidence, ADR-001 v2's own "accepted with a known
gap" note — is arm64/`vz`. That makes CI a valuable *second* architecture, not a
substitute for the first. And KVM being available is necessary, not sufficient:
`hypeman` still has to install and run there, with `mkfs.erofs` and `caddy` and
its configuration, on an ephemeral runner that pays a cold image pull every job.
That is a spike of its own.

## Optional fleet membership

A single node needs no bucket. To join a fleet, add both the coordination bucket
and an endpoint peers can reach:

```sh
barista-node-agent \
  --data-dir ./.barista \
  --listen 127.0.0.1:7070 \
  --runtime fake \
  --fleet-bucket "$BARISTA_FLEET_BUCKET" \
  --fleet-advertise 127.0.0.1:7070
```

Credentials come from the ambient AWS chain. Omitting `--fleet-bucket` means the
fleet module is not constructed. Contract A currently remains loopback-only, so
an endpoint used from another host needs a deployment-owned secure tunnel or
co-located caller; the planned gateway is not available yet.

## Related

- [Getting started](get-started.md)
- [Capabilities and tiers](concepts/capabilities-and-tiers.md)
- [Fleet coordination](concepts/fleet-coordination.md)
