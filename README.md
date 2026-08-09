# Barista

**Always ready. Only awake when it matters.**

Barista is session-centric compute for long-lived agents, development
environments, and interactive workloads. It keeps a session's memory, disk, and
working context together while its compute rests.

**Release the sandbox. Keep the session.** On a memory-capable runtime, Barista
preserves the live process and releases its CPU and host memory. When work
returns, it resumes the same process from the same point—not a reconstruction
from logs, transcripts, or application state.

## Why Barista

Long-running agents and interactive environments accumulate valuable in-memory
context. Traditional compute gives them two choices: stay running while idle,
or restart and rebuild that context later.

Barista separates the lifetime of a session from the lifetime of its sandbox.
A session can outlive the compute running it, so workloads remain ready without
remaining active.

- **Continue instead of reconstructing.** Resume variables, buffers, loaded
  models, and agent context as part of the same live process.
- **Release idle compute.** A paused session keeps its snapshot and metadata,
  not a running sandbox.
- **Use any OCI image.** Workloads run as ordinary processes and do not link to
  a Barista SDK.
- **Address fleet work by name.** A stable session name resolves to one owning
  node; direct-node lifecycle uses the materialised instance id today.
- **Run on infrastructure you own.** A single host needs no cluster or external
  control plane. Fleets coordinate through an object-store bucket.
- **Fail honestly.** If a runtime cannot provide a requested guarantee, Barista
  reports the missing capability instead of silently doing something weaker.

## The session model

A Barista session is a **long-lived, single-writer workload** built from a
digest-pinned OCI image. Direct-node commands address an instance ULID; fleet
coordination adds the stable human name.

```sh
INSTANCE_ID=01ARZ3NDEKTSV4RRFFQ69G5FAV

barista create \
  --instance-id "$INSTANCE_ID" \
  --image ghcr.io/acme/agent:latest \
  --digest sha256:… \
  -- /app/agent

barista start "$INSTANCE_ID"
barista exec "$INSTANCE_ID" -- /app/say "plan the migration"
barista pause "$INSTANCE_ID" --require-memory
barista resume "$INSTANCE_ID" --require-memory
```

The lifecycle has four deliberate moves:

1. **Create** — register an immutable workload specification under an instance
   id, directly or from fleet desired state.
2. **Run** — execute commands, transfer files, and keep useful context in place.
3. **Pause** — capture memory and disk, then release the sandbox.
4. **Resume** — restore the process, perform restore-time duties, and continue.

In fleet mode, exactly one node owns a session name at a time. A superseded
owner self-fences its materialised workload rather than leaving two writers.

## Where it fits

Barista is designed for work whose value accumulates in memory:

- **Cloud agent harnesses** that should survive long gaps between turns.
- **Long-running agents and workers** that wait for tasks, approvals, or
  schedules.
- **Development and preview environments** that should wake with tools and
  processes already in place.
- **Session-affine interactive services** where rebuilding context costs more
  than restoring it.

It is not a general-purpose platform for stateless services or static sites.
Those workloads gain little from memory-preserving sessions.

## Architecture

Each host runs a Node Agent. The agent owns the session state machine, a
crash-safe operation journal, local snapshots, and capability reporting. A
pluggable runtime provides sandbox isolation and snapshot mechanics; Barista
adopts those substrates rather than reimplementing them.

A small guest agent handles readiness, commands, file transfer, snapshot hooks,
and restore-time work inside each sandbox. The implemented runtimes are
`hypeman` and the tooling-only `fake`. For multi-node deployments, nodes
coordinate ownership through compare-and-swap leases in an object-store bucket.
Contract A remains loopback-only; cross-host callers need a deployment-owned
secure path until the planned request gateway ships. There is no scheduler
service or consensus cluster.

```text
client / CLI
     │
     ▼
Node Agent ─── operation journal + session lifecycle
     │
     ▼
runtime ───── isolated sandbox
                    │
                    ├── guest agent
                    └── your workload

optional fleet bucket ─── desired sessions + ownership leases
```

Read the [architecture guide](docs/platform/architecture.md) for the component
model and contract boundaries.

## Vision

Barista treats the session name as the durable unit of compute. Addressing that
name should be enough to create the work, find its owner, or wake it. Placement,
isolation, and lifecycle mechanics stay behind the session interface.

The goal is a platform where an agent, runtime, or environment can wake in three
ways:

- **On command** from an operator, script, or recovery procedure.
- **On schedule** from a durable alarm attached to the session.
- **On request** when traffic arrives for the session name—the planned gateway
  edge, not a current interface.

Across every runtime, Barista keeps the same rule: guarantees must stay visible.
Memory preservation, hardware isolation, live checkpointing, networking, and
egress control are capabilities—not labels.

## Design principles

- **Exact state over imitation.** A memory-preserving resume must continue the
  same process without a guest reboot.
- **Crash-safe by construction.** Mutations are journaled, idempotent, and
  recoverable after abrupt process death.
- **One contract across runtimes.** Consumers use the same API while capability
  discovery exposes differences between hosts.
- **Adopt the substrate.** Barista owns session semantics, not hypervisor
  lifecycle, memory paging, or snapshot implementation.
- **Simple deployment first.** One node works without fleet infrastructure;
  adding hosts adds a bucket, not a control-plane service.

## Getting started

Follow the [getting started guide](docs/get-started.md) to run a node, create a
session, pause it, and verify that its memory survives the restore.

Useful references:

- [Concepts](docs/concepts/index.md)
- [CLI reference](docs/cli.md)
- [Node Agent API](docs/api/index.md)
- [Examples](docs/examples/index.md)
- [Capabilities and tiers](docs/concepts/capabilities-and-tiers.md)
- [Limits and performance](docs/platform/limits.md)
- [Known issues](docs/platform/known-issues.md)

## Contributing

Barista is Apache-2.0 and developed in the open. Before proposing a change,
read:

- [`CONTRIBUTING.md`](CONTRIBUTING.md) for the OpenSpec workflow and quality
  gate.
- [`CLAUDE.md`](CLAUDE.md) for the project constitution.
- [`GOVERNANCE.md`](GOVERNANCE.md) for decision-making and amendments.
- [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) for community expectations.
- [`SECURITY.md`](SECURITY.md) for private vulnerability reports.

The project definition of done is:

```sh
make check
```

## License

Apache-2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
