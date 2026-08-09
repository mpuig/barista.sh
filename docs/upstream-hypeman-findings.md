# Upstream findings — `hypeman`

Defects found in `hypeman` while building the nap-005 backend, recorded here so
they can be reported upstream and so Barista's own test results stay interpretable:
three of them required a local workaround, and a reader six months from now needs
to know which measurements were taken on a patched substrate.

Versions: macOS `0.16.1` (arm64), Linux `0.17.0` (arm64), API `0.3.0`.

---

## 1. The linux/arm64 release embeds x86-64 guest binaries — guests cannot boot

**Severity: blocking.** No instance can start on linux/arm64.

### Symptom

The guest kernel boots and then panics immediately:

```
Run /init as init process
/init.bin: line 11: syntax error: unterminated quoted string
Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000200
CPU: 1 UID: 0 PID: 1 Comm: busybox
```

The shell-syntax error is a red herring worth explaining, because it is what makes
this look like a quoting bug in generated config. `/init` is a `/bin/sh` wrapper
ending in `exec /init.bin "$@"`. `init.bin` is an **x86-64** ELF on an **aarch64**
kernel, so `execve` returns `ENOEXEC`; busybox `sh` then falls back to interpreting
the file as a shell script, and line 11 of the ELF's bytes contains an unbalanced
quote. The reported error is a property of the binary's *bytes*, not of any script.

### Cause

`lib/system/initrd.go` builds the initrd from two embedded binaries, and both
embed directives name a single, non-arch-qualified path:

```go
// lib/system/guest_agent_binary.go
//go:embed guest_agent/guest-agent

// lib/system/init_binary.go
//go:embed init/init
```

Whatever the build placed at those paths is embedded into every host build. The
release pipeline evidently populates them with amd64 binaries, so the linux/arm64
`hypeman-api` carries an x86-64 init and an x86-64 guest-agent. Verified directly:

| artifact | initrd built by macOS 0.16.1 | initrd built by Linux 0.17.0 |
|---|---|---|
| `/init.bin` | ELF aarch64 ✓ | **ELF x86-64** ✗ |
| `/usr/local/bin/guest-agent` | ELF aarch64 ✓ | **ELF x86-64** ✗ |

The two are separate failures in sequence: fixing only `init.bin` gets the guest
to `chroot` and then fails at
`failed to start guest-agent: fork/exec /opt/hypeman/guest-agent: exec format error`.

Not a nested-virtualisation problem, though it presents as one: it reproduces on
`hypeman run busybox` with no other software involved, and identically on both
Linux hypervisors (`cloud-hypervisor` and `firecracker`) — which is itself the
tell, since two independent VMMs failing the same way points at the payload.

Other embedded assets are handled correctly per-arch (`caddy` and `firecracker`
land under `system/binaries/<name>/<version>/aarch64/`), so this is specific to
the two `go:embed`ed guest binaries.

### Suggested fix

Arch-qualify the embeds the way the downloaded binaries already are — e.g.
`//go:embed guest_agent/guest-agent_$GOARCH` selected by build tag, or embed both
and choose at initrd-build time. A cheap regression test: assert the ELF
`e_machine` of both binaries matches `runtime.GOARCH` before writing the initrd,
which would have turned this kernel panic into a startup error naming its cause.

### Workaround used by Barista

The correct aarch64 binaries were taken from the macOS 0.16.1 initrd and grafted
into the Linux 0.17.0 initrd at
`/var/lib/hypeman/system/initrd/aarch64/latest/initrd` (extract cpio, replace
`init.bin` and `usr/local/bin/guest-agent`, repack as `newc`). hypeman does not
rebuild the initrd when its `.hash` sidecar is unchanged, so the patch survives a
daemon restart.

**Consequence for Barista's measurements**, and the reason this file exists: every
Linux number Barista records was taken against a substrate patched this way, pairing
hypeman-api 0.17.0 with 0.16.1 guest binaries. The vsock protocol between them
proved compatible in practice (the agent reaches `HYPEMAN-AGENT-READY` and the
entrypoint runs), but it is a version skew, and any surprising result on Linux
should suspect it first. On linux/**amd64** the shipped binaries are the right
arch, so a normal deployment does not need this and does not carry the skew.

---

## 2. Guest network is unreachable from the host on macOS (upstream #358)

**Severity: blocking for host↔guest transport on macOS.** Open upstream.

Guests are addressed on `10.100.0.0/16`, but on macOS the `vz` bridge is
`192.168.64.1/24` and no host interface carries the guest subnet, so
`route get 10.100.x.y` resolves via the physical LAN and packets leave the
machine. Every host→guest path fails the same way, including the `/ingresses`
Caddy reverse proxy, which returns **502** with route, listener and DNS all
verified healthy.

Linux is unaffected and the contrast is the proof:

| | macOS / `vz` | Linux |
|---|---|---|
| host interface on `10.100/16` | none | `vmbr0: 10.100.0.1/16` |
| route to a guest address | via the physical LAN | `dev vmbr0 src 10.100.0.1` |

So the `/ingresses` transport design is sound; it is the macOS platform binding
that is broken.

---

## 3. Ingress DNS collides with `systemd-resolved` on `:5353`

**Severity: minor, but silent.** On Ubuntu 24.04, hypeman's ingress DNS binds
`:5353`, which `systemd-resolved` already holds. Barista's Lima config moves it to
`5354` (`.tools/nap-linux.yaml`).

---

## 4. A missing `mkfs.erofs` fails images with the cause only in the journal

**Severity: minor, poor diagnostics.** Without `erofs-utils` installed, every
image reaches `status: failed` and the API reports nothing actionable — the real
cause appears only in the daemon's journal. Barista's node preflight now checks for
`mkfs.erofs` by name, alongside the existing `caddy` and `mkfs.ext4` checks.

---

## 5. `POST /instances` answers a bare `500 internal_error` for a bad hypervisor

**Severity: minor, poor diagnostics.** Asking for `vz` on Linux fails with an
unexplained `500`; the real message — `no VM starter for hypervisor type: vz` —
is only in the journal. Relevant to anyone driving the API programmatically,
which is the whole of Barista's use.

---

## Substrate state on the `nap-linux` dev VM

Not a defect, but recorded here for the same reason the rest of this file exists:
a measurement is only interpretable if you know what the substrate was doing.

`nap-005` task 5.5 set `hypervisor.firecracker_snapshot_memory_backend: uffd` in
the VM's `/etc/hypeman/config.yaml` to measure the lazy-restore path. **Reverted
on 2026-08-08**; the VM is back on the stock `file` backend and
`hypeman-uffd@0.1.6.service` is stopped. The uffd variant is kept at
`/etc/hypeman/config.yaml.uffd-nap005` if the sweep needs repeating.

Only the `uffd` rows of the dirty-memory sweep (`docs/BRD.md` §6) were taken with
that setting live; every other Linux number on this VM is on the `file` default.

Reverted rather than left on because the setting bought 5–15% on resume — partly
inside run-to-run noise — while one of the six UFFD runs killed the firecracker
VMM (`fc.sock: connection refused`) with no cause established. A non-default
pager left active would make every later failure on this VM start its diagnosis
with "was it the pager?", which is exactly the interpretability tax this file
exists to avoid.
