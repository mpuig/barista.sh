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

## 6. `network.egress` is schema-validated and unenforced — and unknown fields are accepted

**Severity: high, and silent by construction.** Measured 2026-08-08 on
hypeman-api 0.16.1 against the pinned contract 0.3.0 (`nap-014` task 4.2). A VM
created with `network.egress.enabled: true` and
`enforcement.mode: http_https_only` opens a direct TCP connection to
`1.1.1.1:443` exactly like an unmediated twin. Repeated with the *stronger*
`mode: all`, which the contract describes as rejecting direct non-mediated TCP
egress: both 443 **and** 53 stayed open.

Barista was removed from the picture before this was filed — the instance was
created straight at the substrate API, so the mapping is not the fault.
`GET /instances/{id}` never echoes an `egress` object back, and the daemon's
"allocated network" log line mentions no egress handling.

The second half is what makes the first undetectable: **the API returns `201` for
a request carrying an invented field** (`network.totally_not_a_real_field`). A
client therefore cannot distinguish "the policy was applied" from "the policy was
discarded" by any response it receives. An accepted create is no evidence at all.

This is the worst degradation shape a sandbox platform has: the caller believes
untrusted code cannot reach the internet, and it can. Barista's node reports
`egress_control: false` and *refuses* mediated specs rather than passing them
through, and its acceptance test is written as a tripwire asserting today's
behaviour — so the day this is fixed upstream, that test fails and says so.

---

## 7. No vsock transport for a third-party guest agent

**Severity: design gap, not a defect.** `vsock` does not occur anywhere in
`openapi.yaml` at 0.3.0, and `Instance.network` carries `enabled`, `name`, `ip`
and `mac` — no vsock field, port or CID. So a guest agent that is not hypeman's
own can be reached only over the instance's IP on the shared `default` network.

hypeman itself runs a vsock channel for its own agent (`docs/adr-001-substrate-evaluation.md`
§2 — bidirectional-streaming `Exec` over vsock, disabled with
`--skip-guest-agent`), so the transport exists in the implementation and is
simply not exposed.

The consequence for anything building on hypeman: since `network.name` is always
`"default"`, one instance's agent port is reachable by every sibling VM on the
host, and the only available defence is in-band authentication. Barista has built
per-instance mutual TLS for exactly this (`barista-021`). An exposed vsock
endpoint would retire that mechanism entirely — a channel with no network
identity to spoof needs no certificate to pin.

---

## 8. The build mirror rejects images pinned by multi-arch index digest

**Severity: blocking for any digest-pinned base image.** Measured on API `0.3.0`
(linux/amd64, GitHub-hosted runner), first observed 2026-08-11 on the acceptance
workflow's first bring-up.

### Symptom

`hypeman build` answers `build failed: build failed` — no cause in the API
response (finding §5's shape again). The journal has two errors, and the one the
API reports is the *second*:

```json
{"level":"WARN","msg":"failed to mirror base image",
 "image":"library/python@sha256:9b4929a7…",
 "error":"push to local registry: PUT …/v2/library/python/manifests/sha256:9b4929a7…:
 unexpected status code 400 Bad Request: digest mismatch:
 expected sha256:9b4929a7…, got sha256:1e58d36e…"}
{"level":"ERROR","msg":"build failed",
 "error":"create builder instance: image is required"}
```

### Cause

The Dockerfile pins its base by the **multi-arch index** digest
(`python:3.13-alpine@sha256:9b4929a7…`), which is the digest `docker pull`
prints and the only one that is platform-neutral. hypeman's mirror resolves the
reference — obtaining the **platform manifest** (`sha256:1e58d36e…` for
linux/amd64) — and then pushes that manifest to its local registry under the
*index* digest. The registry correctly refuses content whose digest does not
match its name. The mirror failure is only a `WARN`; the build then proceeds to
create a builder instance with an empty image and fails with the unrelated
`image is required`.

### Consequence and workaround

Any `FROM image@sha256:…` with an index digest — which is what supply-chain
pinning produces — cannot build. The workaround is to strip the digest for
hypeman builds and keep the tag (the acceptance workflow does this with a `sed`,
named as a workaround for this finding). Pinning by the *platform* manifest
digest would satisfy the mirror but breaks every other platform, so it is not a
fix for a Dockerfile that developers on arm64 and CI on amd64 share.

---

## 9. A Linux release install cannot build images: the builder image is never prepared

**Severity: blocking for `hypeman build` on Linux release installs.** Measured on
API `0.3.0` installed by the official script on ubuntu-latest; code read at
`eed540f`. Found on the acceptance workflow's bring-up, 2026-08-11 — the error
survived finding §8's fix, so the two were initially one opaque failure.

### Symptom

With the base image mirrored successfully, `hypeman build` still fails:

```json
{"msg":"creating instance name=builder-… image=\"\" vcpus=4"}
{"level":"ERROR","msg":"build failed","error":"create builder instance: image is required"}
```

### Cause

Builder VMs boot an image that must exist before the first build. With
`build.builder_image` unset (the default), `ensureBuilderImage` at startup
builds the binary's **embedded** builder Dockerfile with Docker — and on
`v0.3.0` that `docker build` uses the **service's cwd as the build context**
("context is cwd = repo root in development"). The installer's systemd unit
starts the service at `/` with `ProtectSystem=strict`, so the Dockerfile's
`COPY go.mod …` directives find nothing and the docker socket is not even
connectable from inside the sandboxed unit. Preparation fails with a WARN —
and `v0.3.0` sets its ready flag in a `defer`, **even on failure**, so a
submitted build is not refused but proceeds to create a builder instance with
an empty image ref.

Current `main` (`eed540f`) is halfway to a fix — it falls back to a local
Docker image `hypeman/builder:latest` that "the installer builds … before
loading the service" — but the installer only does that in its **darwin**
branch, so a Linux release install still has neither path.

### Workaround

Give the `v0.3.0` service what its embedded build expects: a source checkout
of the same tag as cwd, and the docker socket as a writable path.

```ini
# /etc/systemd/system/hypeman.service.d/builder-context.conf
[Service]
WorkingDirectory=/opt/hypeman-src   # git clone --branch v0.3.0
ReadWritePaths=/var/run/docker.sock # ProtectSystem=strict blocks connect()
```

The acceptance workflow does exactly this and then waits for the journal's
"builder image ready" before proceeding, because the ready flag cannot be
trusted (above). Upstream fix would be publishing a pinnable builder image for
`build.builder_image`, porting the installer's darwin builder step to Linux,
and not marking a failed preparation ready.

---

## 10. Default registry config breaks every push: BuildKit told HTTPS, registry serves HTTP

**Severity: blocking for `hypeman build` on a default install.** Measured on API
`0.3.0`, ubuntu-latest, the layer under §9: with the builder image finally
prepared, the scenario image *builds* and then fails its final step:

```
ERROR: failed to push 10.100.0.1:4973/builds/…:
  Head "https://10.100.0.1:4973/v2/…": http: server gave HTTP response to HTTPS client
```

The built-in registry rides the API's own listener, which serves plain HTTP.
But `registry.insecure` defaults to `false` — and that flag is what the build
manager hands the builder VM, where it decides BuildKit's scheme. The example
config has no `registry:` section at all, so a default install ships the
contradiction: an HTTP registry that instructs its only client to speak HTTPS.
(The API-side *mirror* pushes over HTTP regardless, which is why §8's mirroring
worked while the builder's push failed — two clients of the same registry with
two TLS opinions.)

Workaround: state the truth in `/etc/hypeman/config.yaml`:

```yaml
registry:
  insecure: true
```

Upstream fix: default `registry.insecure` to match whether the API listener
actually has TLS, or refuse to start a registry whose advertised scheme it
knows to be wrong.

---

## 11. `hypeman build --image-name` produces a name that never becomes ready

**Severity: blocking for the named handle; a working handle exists.** Measured
on API `0.3.0`, ubuntu-latest, the layer under §10: with mirror, builder image
and registry scheme all fixed, the build itself succeeds — and the *named*
image stays `pending` forever.

### Symptom

```json
{"msg":"build succeeded","id":"hpraalgz…","digest":"sha256:2ef3ade…"}
{"msg":"re-tagged build image","from":"builds/hpraalgz…","to":"docker.io/library/barista-scenario:latest"}
{"level":"WARN","msg":"re-tagged image conversion timed out",
 "image_name":"barista-scenario","error":"get image: image not found"}
```

`GET /images` then reports the named image `pending` indefinitely, and any
instance created from it is refused `image_not_ready`.

### Cause (as far as the journal shows)

After a build, the manager re-tags `builds/{id}` to the requested name via
`ImportLocalImage` and waits for the re-tagged ref to become ready — and that
wait fails with `image not found`, a name-normalization mismatch between the
ref the import registers and the ref the wait looks up. The conversion behind
the name never runs; only a `WARN` records it, and the build still reports
`ready` (its own KERNEL-863 fix waits for `builds/{id}` — which does convert —
not for the name).

### Workaround

Use the handle that works: `builds/{build-id}` is converted and ready before
the CLI returns, under the same digest the build prints. The acceptance
workflow extracts the build id from `Build started:` and creates instances
from `builds/<id>@<digest>`, ignoring the requested name entirely.

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
