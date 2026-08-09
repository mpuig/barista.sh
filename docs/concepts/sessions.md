# Sessions

A session is the unit of compute in Barista: a **named**, **single-writer**,
**long-lived** workload built from one OCI image.

## The name is the handle

Every session has a name that is unique across your fleet. You never need an
opaque id:

```sh
barista create checkout-agent --image ghcr.io/acme/agent --digest sha256:… -- /app/agent
barista exec checkout-agent -- ps aux
barista resume checkout-agent
```

Three properties follow from the name being the handle:

- **Addressing creates.** Writing desired state for a name that does not exist
  yet is what "creating a session" means fleet-wide. Some node picks it up and
  materialises it.
- **Addressing wakes.** If the session exists but is asleep, addressing it
  restores it. You do not have to know it was asleep.
- **Exactly one live session per name.** Ownership is a lease held by one node.
  Two callers addressing `checkout-agent` reach the same process, never two
  copies with diverging state. See [Fleet coordination](fleet-coordination.md).

Barista does use identifiers internally — an instance id per materialisation, a
snapshot id per capture — and the API and CLI will show them to you in listings
and events. They are diagnostics. The name is the contract.

## Single writer

A session runs one workload, and one caller mutates it at a time.

- One in-flight mutating operation per session. A second `pause` while a
  `resume` is running is refused with `CONCURRENT_OPERATION` rather than
  interleaved. Reads and passthrough calls are not affected.
- One owner node per name, enforced by compare-and-swap leases with epoch
  fencing. A node whose lease lapses stops the local workload rather than
  running a second copy.

This is what makes in-memory state a safe place to keep things. Two writers plus
a memory snapshot is a state-divergence machine; one writer plus a memory
snapshot is a session that survives being interrupted.

## One workload per session

A session runs a single process tree from a single image. There is no pod shape,
no sidecar list, no init container.

If you need a second component, run it as a second session and let them address
each other by name. The reason is not minimalism for its own sake: memory
snapshots make "two processes in one session" and "two sessions" genuinely
different — the first pair pauses and resumes atomically as one memory image,
the second pair does not. That is a decision you should make explicitly rather
than inherit from a packaging convention.

## The spec is immutable

The `InstanceSpec` you supply at create — image digest, resources, commands,
hooks, TTL, egress policy, labels — does not change for the life of the session.
To change it, destroy the session and create it again, or create a new named
session and migrate.

The reason is snapshot identity. A snapshot's restore key is derived from the
template that produced it. A spec that could drift underneath a snapshot would
make the key a lie, and a lie in the restore key means restoring memory captured
from image A onto the rootfs of image B, with every precondition passing.

## What a session is made of

| Field | Meaning |
|---|---|
| `template` | The OCI image, **pinned by digest**. The tag is a label; the digest is the identity. |
| `resources` | vCPU, memory, disk. Memory size sets the ceiling on snapshot size and pause cost. |
| `process` | `start_cmd` (the workload), `ready_cmd` (the readiness probe), `env`, `workdir`. |
| `hooks` | `pre_snapshot_cmd` (quiesce before a snapshot) and `post_restore_cmd` (reconnect after one), each with a timeout. |
| `ttl_seconds` / `ttl_action` | How long idle before the platform acts, and what it does — `PAUSE` (default), `STOP`, or `DESTROY`. |
| `egress` | Whether outbound traffic is mediated by the host, and in what mode. |
| `labels` | Arbitrary key/value pairs. Selectable in `ListInstances`. |

## Ready is not a state

A running session carries a separate `ready` boolean, produced by running
`ready_cmd` inside the guest. `RUNNING` means the sandbox is up; `ready` means
the workload says it can serve.

Keep them distinct in your callers. A gateway should wait for `ready`, not for
`RUNNING` — an instance whose workload has not finished scheduling is not a
resumed session, and timing a resume to `RUNNING` will flatter your numbers by
about a third.

## Related

- [Lifecycle and operations](lifecycle-and-operations.md)
- [Sleep and wake](sleep-and-wake.md)
- [Fleet coordination](fleet-coordination.md)
