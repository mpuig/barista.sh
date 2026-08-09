# The linux/arm64 release embeds x86-64 guest binaries — no guest can boot

**Versions:** hypeman-api 0.17.0 (linux/arm64). Not present on linux/amd64 or
macOS/arm64.

## Symptom

Every instance panics immediately after the kernel hands off to init:

```
Run /init as init process
/init.bin: line 11: syntax error: unterminated quoted string
Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000200
```

The shell error is a red herring: `/init` is a `/bin/sh` wrapper ending in
`exec /init.bin "$@"`, and `init.bin` is an **x86-64 ELF on an aarch64 kernel**.
`execve` returns `ENOEXEC`, busybox `sh` falls back to interpreting the ELF as a
script, and "line 11" is a property of the binary's bytes.

Reproduces with `hypeman run busybox` alone, identically on both Linux
hypervisors (cloud-hypervisor and firecracker) — which is the tell that the
payload, not the platform, is at fault.

## Cause

`lib/system/initrd.go` embeds both guest binaries from non-arch-qualified paths:

```go
//go:embed guest_agent/guest-agent   // lib/system/guest_agent_binary.go
//go:embed init/init                 // lib/system/init_binary.go
```

Whatever the release pipeline leaves at those paths ships inside every host
build; for linux/arm64 that is currently the amd64 binaries. Verified by
extracting the initrd: `/init.bin` and `/usr/local/bin/guest-agent` are both
`ELF 64-bit LSB executable, x86-64` in the 0.17.0 linux/arm64 build, and both
aarch64 in the macOS 0.16.1 build. The two fail in sequence: fixing only
`init.bin` reaches `fork/exec /opt/hypeman/guest-agent: exec format error`.

Other embedded assets are already handled per-arch (`caddy`, `firecracker` land
under `system/binaries/<name>/<version>/aarch64/`); only the two `go:embed`ed
guest binaries miss it.

## Suggested fix

Arch-qualify the embeds the way the downloaded binaries already are —
`//go:embed guest_agent/guest-agent_$GOARCH` selected by build tag, or embed
both and pick at initrd-build time. Cheap regression test: assert the ELF
`e_machine` of both binaries matches `runtime.GOARCH` before writing the
initrd — it would turn this kernel panic into a startup error naming its cause.

## Workaround we are running

Graft the aarch64 binaries from the macOS 0.16.1 initrd into the Linux 0.17.0
initrd (extract cpio, replace both files, repack `newc`). Works — the agent
reaches `HYPEMAN-AGENT-READY` — but pairs 0.17.0 api with 0.16.1 guest
binaries, a version skew nobody should have to carry.
