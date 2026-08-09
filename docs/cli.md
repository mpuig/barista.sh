# CLI commands

`barista` is the front door to a node. It is a thin client over the Node Agent API
with no logic of its own — anything the CLI cannot do is an API gap, not a CLI
gap.

## Global flags

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--node <addr>` | `BARISTA_NODE` | `127.0.0.1:7070` | Node to talk to: `host:port` or a path to its unix socket. |
| `--json` | | off | Machine-readable output. Use this in scripts. |
| `--idempotency-key <key>` | | generated | Reuse a key to make a retry a replay rather than a second intention. |

Every mutating verb subscribes to the event stream, submits, then waits for the
operation to finish — so the command exits when the work is done, not when it is
accepted.

## Lifecycle

### `barista create`

Create a session. Does not start it.

```sh
barista create <name> \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  [--vcpu 1] [--mem-mib 512] [--disk-mib 0] \
  [--ttl-seconds 0] [--ttl-action pause|stop|destroy] \
  [--ready-cmd …] [--pre-snapshot-cmd …] [--post-restore-cmd …] \
  [--env KEY=VALUE]... [--label KEY=VALUE]... \
  [--egress all|http-https-only] \
  [--require-hardware-isolation] \
  -- <command>...
```

`--digest` is required. `--ttl-seconds 0` means no TTL. Everything after `--` is
the workload's command.

### `barista start` / `barista stop`

```sh
barista start <name>
barista stop <name> [--grace-seconds 10]
```

`start` from `STOPPED` is a cold boot — memory was lost at stop. `stop` sends a
graceful signal, waits out the grace window, then kills.

### `barista pause` / `barista resume`

```sh
barista pause <name> [--require-memory]
barista resume <name> [--snapshot <snapshot-id>] [--require-memory]
```

`pause` snapshots memory and disk, then releases the sandbox. `resume` restores
the latest snapshot, or the one you name.

`--require-memory` turns a silent downgrade into an error: on `pause`, fail
rather than take a disk-only snapshot; on `resume`, fail rather than cold-boot.

### `barista checkpoint`

```sh
barista checkpoint <name>
```

Snapshot a running session **without pausing it**. Requires the
`live_checkpoint` capability; fails with `CAPABILITY_MISSING` where the runtime
does not have it.

### `barista destroy`

```sh
barista destroy <name> [--keep-snapshots]
```

## Working inside a session

### `barista exec`

```sh
barista exec <name> -- <command>...
barista exec <name>                    # interactive shell
barista exec <name> --tty=false -- …   # force pipes
```

A PTY is allocated when stdin is a terminal. The workload's exit code becomes
`barista`'s exit code, which is what makes this usable in a script.

### `barista cp`

```sh
barista cp ./local.json <name>:/app/config.json
barista cp <name>:/app/out.log ./out.log
```

## Snapshots

```sh
barista snapshot create <name> [--name <label>]
barista snapshot delete <snapshot-id>
barista snapshots [--instance <name>]
```

Named snapshots are retained until you delete them or destroy the session
without `--keep-snapshots`.

## Scheduled wake

```sh
barista wake-at <name> 2026-08-09T09:00:00Z
barista wake-at <name> --clear
```

One alarm per session. Alarms may fire more than once; make the work idempotent.

## Inspection

```sh
barista ls                                  # sessions on this node
barista get <name>                          # one session, in full
barista node info                           # identity, runtimes, capabilities, resources
barista doctor                              # can this node do its job?
barista events [--instance <name>] [--from-cursor <n>]
```

`barista doctor` asks the node these questions over the API, so it reports on the
machine that runs your sessions — not the machine you typed on.

## Fleet

These talk to the coordination bucket, not to a node, so they need
`BARISTA_FLEET_BUCKET` and the ambient AWS credentials — and they work when no
node is running at all, which is when you most want to ask the fleet what
exists.

```sh
barista fleet apply <name> --image <img> --digest <sha256:…> \
  [--vcpu N] [--mem-mib N] [--ttl-seconds N] \
  [--on-owner-loss coldboot|hold] -- <command>...   # write desired state to the bucket
barista fleet ls                                   # fleet inventory
barista fleet resolve <name>                       # name → owning node
```

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Generic failure, including a failed operation with no specific reason. |
| `3` | `CAPABILITY_MISSING` — the runtime cannot do what you asked. |
| `4` | `CONCURRENT_OPERATION` — another operation is in flight for this session. |
| `5` | `SUBSTRATE_UNAVAILABLE` — the runtime's substrate is not answering. Retry. |
| `6` | `INVALID_SPEC` or `TEMPLATE_NOT_FOUND` — fix the request. |

For `barista exec`, the exit code is the workload's.

Up-front refusals and failed operations produce the same exit code, so a script
does not have to care which way the node said no.

## JSON output

```sh
barista --json get agent-42
barista --json events --instance agent-42
```

`--json` emits one object per result, and one object per event for streaming
commands. Errors are emitted as JSON too, carrying the machine-readable reason.

## Related

- [Node Agent API](api/index.md)
- [Errors](api/errors.md)
- [Examples](examples/index.md)
