# Sessions

A session is Barista's durable unit of work: one long-lived workload whose useful
memory and disk can outlive the sandbox currently running it.

Today that model has two handles:

- **Direct node:** a client-chosen instance ULID, unique on that node.
- **Fleet:** a stable human name, unique in the fleet bucket, whose lease points
  to the owning node and materialised instance id.

The product direction is name-as-the-handle everywhere. The current CLI keeps
the distinction visible instead of pretending the direct Node Agent API already
resolves fleet names.

## Direct instance ids

Direct lifecycle commands use an instance id:

```sh
barista create \
  --instance-id 01ARZ3NDEKTSV4RRFFQ69G5FAV \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  -- /app/agent
barista start 01ARZ3NDEKTSV4RRFFQ69G5FAV
barista exec 01ARZ3NDEKTSV4RRFFQ69G5FAV -- ps
```

Omit `--instance-id` to let the CLI generate one. The operation result prints the
id needed by later direct-node commands.

## Fleet names

A fleet session is declared and resolved by name:

```sh
barista fleet apply checkout-agent \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  -- /app/agent
barista fleet resolve checkout-agent
```

Exactly one node owns a name at a time. Ownership is a conditional lease fenced
by version and epoch. The lease also records the materialised instance id, so a
superseded owner can stop the exact workload it no longer owns.

The planned gateway will make addressing a fleet name sufficient to wake and
route application traffic. It is not implemented today; `fleet resolve` is
coordination and discovery, not ingress.

## Single writer

One mutating operation may be in flight per instance. A conflicting mutation is
refused with `CONCURRENT_OPERATION` rather than interleaved.

Across a fleet, one owner lease exists per name. A node that loses its lease
self-fences its local workload, including after an agent restart. These two
rules keep in-memory state from diverging behind one handle.

## One workload per instance

An instance runs one process tree from one OCI image. There is no pod shape,
sidecar list, or init-container API.

Several processes launched by that workload share one memory image and pause
atomically. Separate sessions do not. Choose that boundary deliberately.

## The spec is immutable

`InstanceSpec` is fixed after create. Change it by destroying and recreating the
instance, or by writing desired state under a new fleet name.

Snapshot compatibility depends on this. A mutable image, resource shape, or
runtime bundle could make captured memory appear compatible with a different
root filesystem.

## What the API spec contains

| Field | Meaning |
|---|---|
| `template` | OCI image label plus required digest identity. |
| `resources` | vCPU, memory, and disk. |
| `process` | Workload command, readiness command, environment, and working directory. |
| `hooks` | Pre-snapshot and post-restore commands with timeouts. |
| `ttl_seconds` / `ttl_action` | Idle deadline and `PAUSE`, `STOP`, or `DESTROY` action. |
| `labels` | Values available to `ListInstances` selectors. |
| `egress` | Optional mediated-egress request, capability-gated by the runtime. |

The `barista create` convenience command exposes only image/digest, CPU, memory,
TTL seconds, egress, hardware isolation, and the workload command. Use a
generated Contract A client for the other fields.

## Ready is not a state

`RUNNING` means the sandbox is up. `Instance.ready` is a separate boolean from
`ready_cmd` and means the workload says it can serve.

Callers that configure `ready_cmd` should wait for `ready`, not merely
`RUNNING`. The future request gateway follows the same rule.

## Related

- [Lifecycle and operations](lifecycle-and-operations.md)
- [Sleep and wake](sleep-and-wake.md)
- [Fleet coordination](fleet-coordination.md)
