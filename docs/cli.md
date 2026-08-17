# CLI commands

`barista` is a thin client over the Node Agent API. Direct-node commands address
an **instance id**. Fleet commands address a stable **session name** in the
coordination bucket.

## Global flags

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--node <addr>` | `BARISTA_NODE` | `127.0.0.1:7070` | Node address: `host:port` or a Unix socket path. |
| `--json` | | off | Machine-readable output. Use this in scripts. |

Mutating commands subscribe to events before submitting, then wait for the
operation to finish. Each invocation generates its own idempotency key. Contract
A clients can supply and reuse a key directly; the CLI does not expose that key
as a flag.

## Direct-node lifecycle

### `barista create`

Create an instance without starting it:

```sh
barista create \
  [--instance-id <ulid>] \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  [--vcpu 1] [--mem-mib 512] [--ttl-seconds 0] \
  [--egress mediated|mediated:http-https-only] \
  [--idle-action pause|stop|destroy] \
  [--require-hardware-isolation] \
  -- <command>...
```

If `--instance-id` is omitted, the CLI generates a ULID and prints it in the
result. The Node Agent requires a digest. You may also use the inline form
`--image ghcr.io/acme/agent@sha256:…` and omit `--digest`.

`--idle-action` opts the instance into workload idle hints: when the workload
calls `DeclareIdle` (see [Guest agent](concepts/guest-agent.md)), the Node Agent
runs that action, with the same degradation semantics as `--ttl` (a `pause`
becomes a `stop`, with a degradation event, on a runtime that cannot preserve
memory). Omit it and idle declarations are ignored.

The protobuf `InstanceSpec` supports more fields than this convenience command,
including disk size, environment, readiness, hooks, labels, and TTL action. Use
a generated API client when you need those fields; they are not CLI flags.

### `barista start` / `barista stop`

```sh
barista start <instance-id>
barista stop <instance-id> [--grace-seconds 10]
```

Starting from `STOPPED` is a cold boot. Stop preserves disk and loses memory.

### `barista pause` / `barista resume`

```sh
barista pause <instance-id> [--require-memory]
barista resume <instance-id> [--snapshot <snapshot-id>] [--require-memory]
```

Pause captures what the runtime supports and releases the sandbox. On `fake`, it
is honestly `DISK_ONLY`; `--require-memory` refuses that downgrade. Resume uses
the latest snapshot unless `--snapshot` names an explicit one.

### `barista checkpoint`

```sh
barista checkpoint <instance-id>
```

Checkpoint promises a live capture. It fails with `CAPABILITY_MISSING` on the
current runtimes because neither reports `live_checkpoint`.

### `barista wake-at`

```sh
barista wake-at <instance-id> 5m
barista wake-at <instance-id> 2026-08-09T09:00:00Z
barista wake-at <instance-id> --clear
```

The time may be an RFC 3339 timestamp with `Z` or a numeric offset, or a relative
`90s`, `5m`, `2h`, or `3d` duration. One alarm exists per instance; setting a new
one replaces the previous alarm.

### `barista destroy`

```sh
barista destroy <instance-id> [--keep-snapshots]
```

## Working inside an instance

### `barista exec`

```sh
barista exec <instance-id> -- <command>...
barista exec <instance-id> --tty=false -- <command>...
```

The command is required. A PTY is allocated automatically when stdin is a
terminal unless `--tty` overrides it. The workload exit code becomes the CLI
exit code.

### `barista cp`

```sh
barista cp ./local.json <instance-id>:/app/config.json
barista cp <instance-id>:/app/out.log ./out.log
```

Exec and copy require a reachable guest agent.

## Snapshots

```sh
barista snapshot create <instance-id> [--name <label>]
barista snapshot delete <snapshot-id>
barista snapshots [--instance <instance-id>]
```

`create` captures a retained snapshot on a memory-capable runtime. A running
source may freeze briefly; the operation reports `froze_workload`. Snapshot
identity is always the id; `--name` is a per-instance human label.

## Forks and capsules

```sh
barista fork <source-snapshot-id> [--target-instance-id <id>] [--require-cow]

barista capsule export <snapshot-id> [--tier local|object-store] [--manifest-out <path>]
barista capsule import --manifest <path> [--tier local|object-store]
barista capsule inspect <capsule-id> [--manifest-out <path>]
barista capsule ls [--lineage <id>]
barista capsule delete <capsule-id>
```

`fork` branches a retained snapshot into a new instance; the source keeps
running and the child comes up with a fresh identity and execution epoch. The
operation reports the measured `actual_fork_mode` (`COW` or `FULL_COPY`);
`--require-cow` fails with `FORK_MODE_UNAVAILABLE` rather than accept a full-copy
freeze.

`capsule export` writes a content-addressed capsule (verify-then-publish);
`--manifest-out` saves the manifest so it can be moved to another node and
`import`ed. `import` verifies every referenced object and the CPU-class
compatibility, then registers the capsule and a restorable snapshot without
booting — restore is a separate `resume`/`fork`. `--tier object-store` requires a
configured backend and fails with `OBJECT_STORE_UNAVAILABLE` otherwise. `delete`
is idempotent and collects objects no live capsule references.

See [Forks and capsules](concepts/forks-and-capsules.md) for the model, exact
compatibility, execution epochs, and the security boundary.

## Inspection

```sh
barista ls
barista get <instance-id>
barista node info
barista doctor
barista events [--instance <instance-id>] [--from-cursor <n>]
```

`node info` reports capabilities without deciding whether they are sufficient.
`doctor` is a strict deployment gate: it exits non-zero if the substrate, guest
channel, journal, or memory-preserving pause capability is unavailable.

## Fleet

Fleet commands talk to the bucket rather than a node. They require
`BARISTA_FLEET_BUCKET` and ambient AWS credentials.

```sh
barista fleet apply <name> --image <image> --digest <sha256:…> \
  [--vcpu 1] [--mem-mib 512] [--ttl-seconds 0] \
  [--on-owner-loss coldboot|hold] -- <command>...
barista fleet ls
barista fleet resolve <name>
```

`apply` writes desired state; nodes compete to acquire it. No command chooses a
node. `resolve` returns the current owner and advertised endpoint, or exits 1
when the name is unowned.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Generic failure or a reason without a dedicated code. |
| `3` | `CAPABILITY_MISSING`. |
| `4` | `CONCURRENT_OPERATION`. |
| `5` | `SUBSTRATE_UNAVAILABLE`; retry later. |
| `6` | `INVALID_SPEC` or `TEMPLATE_NOT_FOUND`; fix the request. |

For `barista exec`, the workload's exit code is preserved.

## JSON output

```sh
barista --json get <instance-id>
barista --json events --instance <instance-id>
```

Result commands emit JSON to stdout. Errors emit JSON to stderr. Streaming
commands emit one object per event or frame.

## Related

- [Node Agent API](api/index.md)
- [Errors](api/errors.md)
- [Examples](examples/index.md)
