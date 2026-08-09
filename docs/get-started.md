# Getting started

Run a node, create a session, pause it, and watch it resume with its memory
intact. This takes about ten minutes.

## Before you begin

You need:

- A Linux host with `/dev/kvm`, or an Apple Silicon Mac. Both run the full
  memory tier. Anything else runs the degraded disk-only tier — see
  [Capabilities and tiers](concepts/capabilities-and-tiers.md).
- The `barista` CLI and the `barista-node-agent` binary.
- A container registry you can pull from.

## 1. Start a node

A node is one host running `barista-node-agent`. It owns a data directory (the
operation journal and the node identity) and talks to a runtime.

```sh
barista-node-agent \
  --data-dir /var/lib/barista \
  --listen 127.0.0.1:7070 \
  --runtime hypeman \
  --guest-bin /usr/local/lib/barista/barista-guest-agent
```

The node prints `LISTENING 127.0.0.1:7070` once it is bound.

`--guest-bin` is the static guest agent Barista injects into every sandbox. Without
it the node reports `guest_agent: false` and refuses exec, file transfer, and
hooks rather than pretending to support them.

## 2. Check the node

```sh
barista doctor
```

```
✓ node reachable            127.0.0.1:7070
✓ node identity             01J9Z… (aarch64, 4f2a9c11)
✓ substrate for 'hypeman'   healthy (0.17.0)
✓ guest agent               injectable
✓ memory snapshots          available
```

`barista doctor` asks the node these questions over the API, so it describes the
machine that runs your sessions rather than the machine you typed on. If any
line fails, fix it before continuing — a node that cannot keep memory will
happily accept a pause and give you back a cold boot.

## 3. Create a session

```sh
barista create agent-42 \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  --mem-mib 2048 \
  --ttl-seconds 900 \
  -- /app/agent --serve
```

What each part means:

- `agent-42` is the session name. It is unique fleet-wide and is how you reach
  the session forever after.
- `--digest` pins the image bytes. Barista requires it: a tag can be repointed at
  different bytes tomorrow, and a snapshot restored onto the wrong rootfs is a
  silent corruption, not an error.
- `--ttl-seconds 900` means "if nothing touches this session for 15 minutes,
  pause it". TTL resets on activity.
- Everything after `--` is the workload's command.

Start it:

```sh
barista start agent-42
```

## 4. Work in the session

```sh
barista exec agent-42 -- python -c 'print("hello from inside")'
barista exec agent-42            # interactive shell, PTY allocated automatically
barista cp ./context.json agent-42:/app/context.json
barista cp agent-42:/app/out.log ./out.log
```

Every passthrough call counts as activity and resets the TTL.

## 5. Pause it

```sh
barista pause agent-42
```

```
agent-42  PAUSED  snapshot 01J9Z… (MEMORY_AND_DISK, 612 MB)
```

The session now holds no sandbox process, no CPU, and no host memory. What is
left on the node is a snapshot file and a journal row.

If you need to be certain memory was kept, ask:

```sh
barista pause agent-42 --require-memory
```

With `--require-memory`, a node that can only take a disk-only snapshot fails
with `CAPABILITY_MISSING` and leaves the session running, instead of handing you
a snapshot that will cold-boot.

## 6. Resume it

```sh
barista resume agent-42
```

```
agent-42  RUNNING  restored from 01J9Z… in 372 ms
```

The workload continues from where it stopped. Before it observes anything, the
guest agent has reseeded the kernel RNG and stepped the clock, and your
`post_restore_cmd` hook has run so the workload can reopen sockets that did not
survive the pause.

Verify it for yourself — the session's uptime keeps counting from before the
pause, because nothing rebooted:

```sh
barista exec agent-42 -- cat /proc/uptime
```

## 7. Clean up

```sh
barista destroy agent-42                    # session and snapshots
barista destroy agent-42 --keep-snapshots   # keep the snapshots to restore later
```

## Next steps

- [Concepts: sessions](concepts/sessions.md) — names, ownership, and what a
  session actually is.
- [Concepts: sleep and wake](concepts/sleep-and-wake.md) — TTL, alarms, and
  wake-on-request.
- [Best practices](best-practices.md) — what to do in your image so a pause is
  invisible to your users.
- [Examples](examples/index.md) — a full agent session that sleeps between
  turns.
