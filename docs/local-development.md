# Local development

Run a node on your own machine, develop against the API, and know which parts of
what you see are real.

## Choose a runtime

| Runtime | Where it runs | Memory snapshots | Use it for |
|---|---|---|---|
| `hypeman` | Linux with `/dev/kvm`, macOS on Apple Silicon | ✓ | Anything involving pause, resume, or snapshots. |
| `fake` | Anywhere Docker runs | ✗ | API shape, CLI ergonomics, lifecycle logic, event handling. |

The `fake` runtime is **tooling only**. It is honest about it — it reports
`memory_snapshot: false`, every pause returns `DISK_ONLY`, and TTL `PAUSE` falls
back to `STOP`. Never read it as a reference for snapshot semantics.

## Start a node

Fake runtime, for API work:

```sh
barista-node-agent --data-dir ./.barista --listen 127.0.0.1:7070 --runtime fake
```

Real substrate, for snapshot work:

```sh
barista-node-agent \
  --data-dir ./.barista \
  --listen 127.0.0.1:7070 \
  --runtime hypeman \
  --hypervisor cloud-hypervisor \
  --guest-bin .tools/guest/barista-guest-agent
```

| Flag | Default | Notes |
|---|---|---|
| `--data-dir` | required | Journal and node identity. Permissions are restricted on create. |
| `--listen` | `127.0.0.1:7070` | Or `--uds <path>` for a unix socket. Port 0 gets an ephemeral port. |
| `--runtime` | `fake` | Defaults to `fake` deliberately: a node that means to keep memory has to ask for it by name. |
| `--hypervisor` | `cloud-hypervisor` | `vz` on macOS; `cloud-hypervisor` or `firecracker` on Linux. |
| `--guest-bin` | `BARISTA_GUEST_BIN` | Without it, the node reports `guest_agent: false` and refuses passthrough. |

Point the CLI at it:

```sh
export BARISTA_NODE=127.0.0.1:7070
barista doctor
```

## Build the guest agent

The guest agent is injected into every sandbox as a **static musl binary**, so
it has to be built for `linux/musl` even when you develop on macOS:

```sh
task guest-bin     # builds in Docker, caches in .tools/guest/
```

Roughly a minute the first time, then cached until the sources change. Tests
that need it self-skip when it is missing rather than failing obscurely.

## The macOS story

`hypeman` on Apple Silicon uses Virtualization.framework and does real memory
snapshots — a 60-second pause and resume works on a laptop.

What does not work yet is host-to-guest networking: guests are assigned a subnet
the macOS host cannot reach (upstream `hypeman` #358). Anything that needs to
reach into a session over the network — including the end-to-end agent scenario
— is Linux-only until that lands. See [Known issues](platform/known-issues.md).

The practical setup is a Linux VM. A Lima configuration is provided:

```sh
limactl start .tools/nap-linux.yaml
```

## Running the acceptance tests

The suite runs against `fake` by default:

```sh
cargo test -p barista-node-agent
```

Against the real substrate:

```sh
BARISTA_TEST_RUNTIME=hypeman cargo test -p barista-node-agent
```

Tests that need a capability the selected runtime lacks **skip with a reason
naming it**, rather than passing against the degraded path. A green run on
`fake` is not evidence about snapshots.

## The quality gate

```sh
make check
```

Fail-closed: spec validation plus the project quality gate. Nothing is
considered done until it passes.

## Working without a bucket

A single node needs no coordination backend. Every verb works exactly as it does
in a fleet, and nothing reports degradation, because nothing is degraded.
Configure a bucket only when a second node could contend for the same names.

## Related

- [Getting started](get-started.md)
- [Known issues](platform/known-issues.md)
- [Capabilities and tiers](concepts/capabilities-and-tiers.md)
