# Getting started

Run a node, create an instance, pause it, and verify that its guest did not reboot.
The complete walkthrough requires Linux with `/dev/kvm`; the current macOS
substrate can snapshot memory but cannot carry the host-to-guest path used by
`exec`.

## Before you begin

You need:

- `barista`, `barista-node-agent`, and the static `barista-guest-agent` binary.
- A running, authenticated `hypeman-api` on a Linux host with `/dev/kvm`.
- Access to the OCI image used below.

For a Docker-backed API-development setup without memory semantics, see
[Local development](local-development.md).

## 1. Start a node

A node owns a data directory containing its identity, journal, and local
snapshot metadata.

```sh
export BARISTA_HYPEMAN_TOKEN_FILE=/path/to/hypeman-token

barista-node-agent \
  --data-dir /var/lib/barista \
  --listen 127.0.0.1:7070 \
  --runtime hypeman \
  --hypervisor cloud-hypervisor \
  --guest-bin /usr/local/lib/barista/barista-guest-agent
```

Use `firecracker` instead of `cloud-hypervisor` if that is the backend your
substrate is configured to run. The node prints `LISTENING 127.0.0.1:7070` once
it is ready to serve Contract A.

## 2. Check the node

```sh
barista doctor
```

`doctor` is strict. It exits non-zero if the substrate, guest channel, journal,
or memory-preserving pause capability is unavailable. Use `barista node info`
when you only want to inspect a deliberately degraded node.

## 3. Create an instance

Direct-node commands use a client-chosen ULID. The CLI generates one when you
omit `--instance-id`; this guide supplies one so later commands are easy to copy.

```sh
INSTANCE_ID=01ARZ3NDEKTSV4RRFFQ69G5FAV

barista create \
  --instance-id "$INSTANCE_ID" \
  --image busybox:latest \
  --digest sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616 \
  --mem-mib 512 \
  --ttl-seconds 900 \
  -- sleep 1d

barista start "$INSTANCE_ID"
```

The digest is required because memory captured against one root filesystem must
never be restored onto different image bytes.

## 4. Work inside it

```sh
barista exec "$INSTANCE_ID" -- sh -c 'echo hello from inside'
barista exec "$INSTANCE_ID" -- cat /proc/uptime
barista cp ./context.json "$INSTANCE_ID":/tmp/context.json
barista cp "$INSTANCE_ID":/tmp/context.json ./context.roundtrip.json
```

Exec and file transfer count as activity and reset the TTL.

## 5. Pause it

```sh
barista pause "$INSTANCE_ID" --require-memory
```

`--require-memory` makes this walkthrough fail instead of accepting a disk-only
result. Once paused, the sandbox process, CPU allocation, and host RAM are gone;
the local snapshot and journal record remain.

Inspect the result:

```sh
barista get "$INSTANCE_ID"
barista snapshots --instance "$INSTANCE_ID"
```

## 6. Resume it

```sh
barista resume "$INSTANCE_ID" --require-memory
barista exec "$INSTANCE_ID" -- cat /proc/uptime
```

The uptime continues from before the pause. A cold boot would reset it, so this
checks the semantic guarantee rather than only trusting a state label.

## 7. Clean up

```sh
barista destroy "$INSTANCE_ID"
```

Use `--keep-snapshots` only when you deliberately want retained snapshots to
outlive the instance.

## Next steps

- [Sessions](concepts/sessions.md) — direct instance ids and fleet names.
- [Sleep and wake](concepts/sleep-and-wake.md) — TTL and scheduled wake.
- [Snapshots](concepts/snapshots.md) — kinds, restore keys, and retained points.
- [CLI reference](cli.md) — the complete implemented command surface.
- [Known issues](platform/known-issues.md) — host and runtime limitations.
