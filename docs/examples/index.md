# Examples

These examples use the implemented CLI. Direct-node examples address an instance
ULID; fleet examples address a stable session name.

- [An agent instance that sleeps between turns](#an-agent-instance-that-sleeps-between-turns)
- [Declare a fleet session](#declare-a-fleet-session)
- [Schedule a wake](#schedule-a-wake)
- [Point-in-time recovery](#point-in-time-recovery)
- [Drive a node from Python](#drive-a-node-from-python)
- [Planned patterns](#planned-patterns)

## An agent instance that sleeps between turns

Use a long-running workload as the session. `exec` sends work to it; an exec
process is not itself the durable workload.

```sh
INSTANCE_ID=01ARZ3NDEKTSV4RRFFQ69G5FAV

barista create \
  --instance-id "$INSTANCE_ID" \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  --mem-mib 2048 \
  --ttl-seconds 600 \
  -- /app/agent --serve

barista start "$INSTANCE_ID"
barista exec "$INSTANCE_ID" -- /app/say '{"prompt":"summarise the migration plan"}'
barista pause "$INSTANCE_ID" --require-memory
barista resume "$INSTANCE_ID" --require-memory
barista exec "$INSTANCE_ID" -- /app/say '{"prompt":"now do step two"}'
```

The workload must own its conversation state. The repository's T7 scenario
verifies continuity by comparing an in-memory transcript digest and guest uptime
across repeated pauses.

Readiness and snapshot hooks are fields on Contract A's `InstanceSpec`, but the
current `barista create` convenience command does not expose them. Configure
those fields through a generated API client when the workload needs a readiness
probe or reconnection hook.

## Declare a fleet session

Fleet commands use a human session name and talk directly to the bucket:

```sh
export BARISTA_FLEET_BUCKET="s3://acme-barista-fleet"

barista fleet apply preview-pr-1487 \
  --image ghcr.io/acme/app:pr-1487 \
  --digest sha256:1f0c… \
  --vcpu 2 \
  --mem-mib 4096 \
  --ttl-seconds 1800 \
  -- /app/server --port 8080

barista fleet ls
barista fleet resolve preview-pr-1487
```

A node acquires and materialises the desired session. Nothing in `apply` chooses
a node. `resolve` reports the owner and advertised endpoint.

This is coordination and discovery, not public ingress. Contract A is
loopback-only by default, and the request gateway that will wake and route by
name is planned work. Until then, use a co-located client or a secure tunnel
owned by your deployment.

## Schedule a wake

One alarm can be attached to a direct-node instance:

```sh
INSTANCE_ID=01ARZ3NDEKTSV4RRFFQ69G5FAV

barista wake-at "$INSTANCE_ID" 2h
barista wake-at "$INSTANCE_ID" 2026-08-09T09:00:00Z
barista wake-at "$INSTANCE_ID" --clear
```

Setting an alarm replaces the previous one. A due alarm resumes a paused
instance or starts a stopped one. Alarm-driven work should be idempotent because
a firing may be replayed after a node-agent crash.

## Point-in-time recovery

On a memory-capable runtime, capture a retained point and restore it later:

```sh
barista snapshot create "$INSTANCE_ID" --name before-migration
barista snapshots --instance "$INSTANCE_ID"
barista resume "$INSTANCE_ID" --snapshot <snapshot-id> --require-memory
barista snapshot delete <snapshot-id>
```

A named snapshot is still addressed by id. Capturing a running instance may
briefly freeze it; inspect the operation's `froze_workload` field in JSON output.

## Drive a node from Python

`--json` exposes operation results without requiring a language-specific SDK:

```python
import json
import subprocess

IMAGE = "ghcr.io/acme/agent:2026-08"
DIGEST = "sha256:9b2c0f…"


def barista(*args):
    completed = subprocess.run(
        ["barista", "--json", *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


created = barista(
    "create", "--image", IMAGE, "--digest", DIGEST,
    "--", "/app/agent", "--serve",
)
instance_id = created["instance_id"]

barista("start", instance_id)
barista("pause", instance_id, "--require-memory")
resumed = barista("resume", instance_id, "--require-memory")
print(resumed["state"], resumed["degraded"])
```

A failing command emits JSON to stderr and uses the documented exit codes.

## Planned patterns

Two useful patterns depend on interfaces that are not implemented yet:

- **Wake-on-request preview environments.** Fleet naming and ownership work, but
  the gateway that holds an incoming request while a session resumes is planned.
- **Golden templates for new sessions.** The current API can restore an existing
  instance from one of its retained snapshot ids. Creating a different instance
  from that snapshot is reserved for a later contract.

They remain product direction, not commands to copy today.

## Related

- [CLI reference](../cli.md)
- [Best practices](../best-practices.md)
- [Fleet coordination](../concepts/fleet-coordination.md)
- [Known issues](../platform/known-issues.md)
