# ADR-001 substrate evaluation — `hypeman` as Contract B

> Status: **RATIFIED 2026-08-06** (task 4.5). The §6 recommendation was accepted
> by the human and is recorded as **BRD ADR-001 v2, §13.7**. Change:
> `nap-004-runtime-substrate-spike`.
>
> Ratified knowingly with task 3.4 still open: every restore-performance number
> below is arm64/`vz`. The firecracker/UFFD path is unmeasured, so no claim about
> production restore latency rests on this document.
>
> Subject: `kernel/hypeman` @ `07d2c6a` (2026-08-05), MIT, Go.
> Host for all measurements: Apple Silicon (aarch64) macOS, `vz` backend.

## 1. Gates

### 1.2 Does the Virtualization.framework backend snapshot? — **Yes, arm64 only**

`lib/hypervisor/vz/client.go:87`:

```go
SupportsSnapshot: runtime.GOARCH == "arm64",
```

Registered capabilities, read from each backend's `capabilities()`:

| Flag | `vz` | `firecracker` | `cloud-hypervisor` | `qemu` |
|---|---|---|---|---|
| `SupportsSnapshot` | **arm64 only** | yes | yes | yes |
| `SupportsPause`, `SupportsVsock` | yes | yes | yes | yes |
| `SupportsSnapshotBaseReuse` | **no** | **yes** | no | no |
| `SupportsHotplugMemory` | no | no | yes | no |
| `SupportsDiskIOLimit` | no | yes | yes | yes |
| `SupportsGracefulVMMShutdown` | yes | no | yes | yes |

### 1.3 Can the Barista guest agent be injected? — **Yes**

- `hypeman run --entrypoint` (repeatable) overrides the image entrypoint, so
  `barista-guest-agent serve` can wrap the workload without touching the image.
- `--env/-e` delivers the per-instance token (spec §7 bootstrap).
- `lib/system/README.md`: init is PID 1, chroots to the container rootfs, starts
  its own agent, then launches the entrypoint **as a child**, forwarding
  shutdown signals to it. So on this substrate Barista's agent is *not* PID 1 and
  the SIGTERM-forwarding and zombie-reaping caveats from nap-003's `fake` path
  become hypeman's problem, not ours.
- hypeman ships its own guest agent (`lib/guest`, gRPC `GuestService` with
  bidirectional-streaming `Exec`, over vsock; `--skip-guest-agent` disables it
  at the cost of `exec`/`stat`). Its `Exec` stream is a byte channel that can
  carry gRPC — so **nap-003's `bridge` mode transfers unchanged**, with a
  different `open_bridge()`. No new transport design is needed.
- Overlap to decide, not a blocker: their agent covers exec/shutdown/metrics;
  Contract C adds `ready_cmd`, snapshot hooks, TTL activity and restore duties.
  Coexisting is cheap; forking their agent to add Contract C is not.

### 1.4 Node-scoped sandbox labels? — **Yes**

`--tag/--metadata/-l KEY=VALUE` (repeatable) carries `barista.node_id`, satisfying
the scoped zero-orphan sweep required by `node-agent-api`.

### 1.1 Install on this host — **works, with two undocumented prerequisites**

Both are fatal and neither is mentioned in the install output:

1. **`caddy`** — `lib/ingress/binaries_darwin.go` only looks for it on `PATH`
   (not embedded on macOS), and ingress init is unconditional, so the API
   crash-loops every ~10s until `brew install caddy`. Barista does not use ingress;
   its gateway is Phase 5. Hard dependency for a feature we do not want.
2. **`e2fsprogs`** — on Darwin the rootfs is converted to **ext4** (the VZ
   kernel has no erofs support) via `mkfs.ext4`, looked up on `PATH` or in
   Homebrew's keg-only path. Missing it fails the image with a bare status of
   `failed`; the reason appears nowhere in logs or API and had to be read out of
   `lib/images/disk.go`. **Diagnosability is poor.**

Non-fatal: hypeman's *own build* feature fails here (`docker build … failed to
xattr .file: permission denied`). It affects `lib/builds` only, not pull/run.
This was **not** a corporate-proxy (Zscaler) issue — the registry resolved a
manifest digest successfully; both real failures were missing local binaries.

Image conversion overhead: `busybox:latest` becomes a 701 MB *apparent* ext4
disk, but only ~15 MB allocated — the disks are sparse, so apparent and
allocated sizes must always be reported separately.

## 2. Measurements

`vz`, aarch64, `busybox:latest`. Guest memory dirtied into `/dev/shm` (tmpfs =
real guest memory pages, so snapshots are not sparse). `/dev/shm` is capped at
about half of guest RAM, which bounds the dirty sizes below. Probes:
`work/t7-probe.sh`, `work/idle-cost.sh`.

| Dirty memory | Content | Snapshot | Standby | **Restore** |
|---|---|---:|---:|---:|
| ~0 (untouched) | — | ~92 MB | 0.47 s | **0.67 s** |
| 1.5 GiB | repetitive text | 92 MB | — | — |
| 1.5 GiB | 1 GiB text + 0.5 GiB random | 608 MB | 1.12 s | **1.97 s** |
| 1.9 GiB | all `/dev/urandom` | 2.0 GB | 3.69 s | **29.21 s** |

### 3.1 T7 shape — **semantics pass**

`/proc/uptime` continued across a 60 s pause (338.80 s → 338.96 s) instead of
resetting, so memory was genuinely restored and the guest genuinely suspended —
Barista's T3 assertion, satisfied on a developer's Mac. While in `Standby` the
instance disappears from `hypeman ps` and appears only under `ps -a`, matching
Barista's `PAUSED` = "holds zero sandbox resources".

### 3.2 Restore latency — **tracks snapshot bytes, i.e. memory entropy**

**Correction to an earlier draft of this annex.** The first measurement filled
guest memory from `/dev/urandom` and produced 29.21 s, which was written up as a
~44× degradation and an R-SNAP-4 blocker. That was an artifact of adversarial
test data: random bytes are incompressible, so they produce the largest possible
snapshot. Re-measured with realistic content — a mix of repetitive text and
random bytes, standing in for code, JSON and session context alongside genuinely
random heap and binary data — the same 1.5 GiB of live memory restores in
**1.97 s** from a 608 MB snapshot.

Snapshot size is essentially `92 MB baseline + incompressible bytes`: 1.5 GiB of
repetitive text compresses to 92 MB (~17:1), the mixed case lands at 608 MB
(~2.6:1, and almost exactly additive — 512 MB of random plus the text's 92 MB),
and pure random stays ~1:1. **Restore cost follows snapshot bytes, not guest
memory size.**

Two honest caveats on the 29.21 s outlier: the host was under heavy memory
pressure during that run (`vm_stat` showed the compressor holding ~1.8 GiB more
than at rest), so it may reflect host swap as much as snapshot size. It should
be re-measured under controlled conditions before being cited at all.

So a ~2 s resume for a realistic multi-GiB session **plausibly satisfies
R-SNAP-4** — it should comfortably beat cold-starting a Python or Node agent
runtime — though it is not the "milliseconds" of the vendor's blog, and `vz`
still lacks the base-reuse and userfaultfd lazy-paging machinery that would make
restore size-independent. That remains the point of task 3.4.

### 3.3 Idle cost of a paused session — **compute is free; disk is sub-linear**

The value proposition ("hibernate at ~zero cost") decomposes into three costs,
which must be kept separate:

| Cost | While `Standby` | Evidence |
|---|---|---|
| **CPU** | **zero** | VMM process count goes 1 → 0; no process remains for the instance |
| **RAM** | **zero on the host** | there is no process to hold it |
| **Disk** | **the real cost** | see below |

Per paused session, measured with 1.5 GiB of live memory:

| Component | Allocated | Apparent | Charged per session? |
|---|---:|---:|---|
| `snapshots/…/machine-state.vzm` | 608 MB | 608 MB | **yes** — not sparse |
| `overlay.raw` (writable layer) | 151 MB | 10 GiB | **yes**, but sparse; grows with writes |
| `images/…/rootfs.ext4` (base) | 15 MB | 701 MB | **no** — shared between instances |

So a realistic paused session costs roughly **0.4 bytes of disk per byte of live
memory**, plus ~150 MB of overlay — and nothing at all in CPU or RAM. The
worst case (incompressible memory) is ~1:1.

A caveat on the RAM claim: Virtualization.framework does not attribute guest
memory to the VMM process's RSS (the `vz-shim` shows ~17 MB while hosting a
4 GiB guest), so no trustworthy *running-state* host-RAM figure could be taken
here. The zero-while-paused claim rests on the process being gone entirely,
which is sound; the running-state figure needs a different instrument.

`--snapshot-compression-enabled` (zstd, with a delay) made **no measurable
difference** in a controlled A/B at 1.5 GiB, in either direction. Since
compression is evidently happening regardless, either the `.vzm` writer always
compresses or the flags are a no-op on `vz` — consistent with `--auto-standby`
being documented Linux-only. **Unresolved**; do not rely on those flags.

Implication for Barista: hibernation really is ~free in compute, and disk is
sub-linear in memory for realistic content. Disk is still the cost that scales
with the number of paused sessions — 1,000 paused sessions at 608 MB is ~600 GB
of local disk — which is precisely what motivates **R-SNAP-2's object-store
tier**. The two-tier design is validated, not undermined.

### 2.1 Contract B verb mapping — **one verb has no equivalent**

| Barista verb / trait method | hypeman | Fit |
|---|---|---|
| `create(spec, guest)` | `POST /instances` | **wrinkle** — it boots. See below |
| `start(h)` | `POST /instances/{id}/start` | clean |
| `stop(h, grace_seconds)` | `POST /instances/{id}/stop` | clean — init forwards the signal to the entrypoint, then falls back to hypervisor shutdown and a force-kill after the stop timeout |
| `destroy(h)` | `DELETE /instances/{id}` | clean; idempotency still to confirm |
| `list_labeled()` | `GET /instances` + tag filter | clean (`-l barista.node_id=…`) |
| `remove_orphan(id)` | `DELETE /instances/{id}` | clean |
| `guest_channel()` | exec stream over vsock | clean — nap-003's `bridge` transfers |
| **`Pause`** | `POST /instances/{id}/standby` | clean, verified |
| **`Resume`** | `POST /instances/{id}/restore` | clean, verified (~2 s realistic) |
| **`Checkpoint`** (live; instance keeps running) | **no equivalent** | **gap** — see below |
| `ListSnapshots` / `DeleteSnapshot` | `GET /snapshots`, `DELETE /snapshots/{snapshotId}` | clean |
| — (beyond v1alpha1) | `POST /instances/{id}/fork`, `POST /snapshots/{id}/fork` | bonus |

**The gap: no live checkpoint.** `POST /instances/{id}/snapshots` accepts a
`Running` source, but `lib/instances/snapshot.go:98–135` implements it as
`standbyInstance()` → `copySnapshotPayload()` → `restoreInstance()`. The guest is
suspended for the whole copy. Barista's `Checkpoint` is specified as a *live*
snapshot with the instance still running (spec §2.5, B31; E2B's transient
`Snapshotting`; runsc's `--leave-running`), so on a hypeman backend:

- `RuntimeCapabilities.live_checkpoint` must be **`false`**, and
  `CheckpointInstance` must fail with `CAPABILITY_MISSING` — which is exactly the
  honest-degradation machinery nap-002 already built;
- **acceptance test T2 is not achievable** ("Checkpoint while a counter process
  runs; counter never stops"). Given the measured standby+restore cost, the
  counter would stop for seconds.

This is the strongest single argument for the **dual-tier** outcome rather than
hypeman-only: `runsc` (or firecracker per B31/B38) remains the only path to a
live checkpoint, so if any consumer needs T2, hypeman cannot be the sole backend.

**The wrinkle: `create` boots.** There is a `StateCreated` ("VMM created but not
booted") but it is annotated *CH native* — Cloud Hypervisor's two-phase
configure-then-boot — not a general create-without-boot, and `Manager.Create`
calls `StartVM` unconditionally. Two mappings, both acceptable:

1. Defer the hypeman call to Barista's `start`, so Barista's `CREATED` is registry-only.
   Preserves Barista's state machine and T1's exact sequence, but `CREATED` then
   asserts less than it does today (no sandbox has been materialized).
2. `POST /instances` then immediately `stop`, mapping `CREATED` onto hypeman's
   `Stopped` ("no VMM, no snapshot"). Semantically truer, costs a boot cycle.

Option 1 is preferable: nap-003 already treats `CREATED` as a journal state, and
paying a boot+shutdown per create would be a real latency cost on the hot path.

**Settles the two-agents question from §1.3:** snapshotting a running instance
calls `ensureGuestAgentReadyForForkPhase`, so hypeman's own guest agent is
load-bearing for `Pause`. `--skip-guest-agent` is therefore not an option — Barista's
Contract C agent coexists with theirs rather than replacing it.

### 2.3 Snapshot keying and restore compatibility — **a shared responsibility**

| Barista key (spec §3.3) | hypeman equivalent | Enforced on restore? |
|---|---|---|
| `template_hash` | `StoredMetadata.ResolvedImage` — digest-pinned `repo@sha256:…`, documented as existing so an instance "can't drift to a different image/arch across restarts" | **Yes.** `restore.go:62–76` resolves it and fails fast if the image is gone, with a regression test (`restore_image_missing_test.go`) |
| `runtime_bundle_ref` | `StoredMetadata.HypervisorVersion` + `KernelVersion`, on the *instance* | **Partially.** `restore.go:443` boots with the pinned `stored.HypervisorVersion`, and `GetBinaryPath` errors if that version is unavailable. But the versions live on the instance, not the snapshot, there is no guest-agent version, and no explicit compatibility comparison happens |
| `cpu_class` | **nothing** | **No.** There is no CPUID or feature comparison anywhere in `lib/` |

`Snapshot` itself records only `Id`, `Name`, `Kind`, `Tags`,
`SourceInstanceID`, `SourceName`, **`SourceHypervisor`** (type, not version),
`CreatedAt`, `SizeBytes` and compression fields.

Three conclusions, and none of them is a defect:

1. **Image identity: hypeman is already as strong as our spec requires.** Digest
   pinning with an explicit anti-drift intent plus a tested fast-fail is exactly
   what `template_hash` and `SNAPSHOT_INVALIDATED` exist for. Barista should *map*
   `template_hash` onto the resolved digest rather than re-implement it.
2. **`cpu_class`'s absence is a boundary, not a gap.** hypeman's snapshots are
   node-local and restored on the same host, where the CPU cannot differ. CPU
   class only becomes load-bearing when a snapshot moves *between* hosts — i.e.
   R-SNAP-2's object-store tier and cross-node migration, which hypeman puts
   explicitly out of scope ("cross-host scheduling, failover and regional
   placement are handled outside"). That is Barista's Phase 2, exactly where our own
   spec already puts it.
3. **The guest-agent version is unambiguously Barista's job.** Contract C's binary is
   *our* component: a Barista upgrade could otherwise change the in-guest agent
   underneath a restored snapshot. No substrate can key that for us.

So the keying split falls almost exactly on the existing Contract A/B seam. Barista
must own: `cpu_class` (only for the remote tier), the guest-agent component of
`runtime_bundle_ref`, and the **machine-readable reasons** — hypeman surfaces
generic errors and 404s, whereas spec §8 requires `CPU_CLASS_MISMATCH`,
`BUNDLE_MISMATCH` and `SNAPSHOT_INVALIDATED`. The cold-boot fallback (B42) stays
Barista's too, since it is a policy decision.

### 2.4 Fork — **works on `vz`**

A running 4 GB instance holding ~2 GB of dirty memory forked into a second
*Running* instance. Evidence for spec §10 OQ4 (N-times restore) and B39/B10,
suggesting the v1alpha2 deferral of fork may be unnecessary.

## 3. Build vs adopt — sizing

Substrate that a from-scratch Barista implementation would have to reproduce
(excluding `lib/instances`, which is lifecycle orchestration Barista already owns in
a better-specified, journaled form):

| Area | Code | Tests |
|---|---:|---:|
| `lib/hypervisor` (4 backends) | 6,505 | 1,914 |
| `lib/system` (initrd, init, OCI boot) | 3,621 | 989 |
| `lib/images` | 3,045 | 1,887 |
| `lib/network` | 2,745 | 487 |
| `lib/devices` | 2,547 | 2,324 |
| `lib/uffdpager` + `lib/uffdgraduate` | 2,194 | 641 |
| `lib/guestmemory` | 1,560 | 440 |
| `cmd/vz-shim` | 1,020 | 81 |
| `lib/volumes` | 1,012 | 832 |
| `lib/snapshot` + `lib/forkvm` | 1,043 | 706 |
| **Total** | **≈ 25,300** | **≈ 10,300** |

Barista today is **6,098** hand-written lines (4,341 code + 1,757 tests). Cloning
the substrate is therefore ~6× the entire project to date, for zero
differentiation. The measurements above sharpen the point: the hard part is the
paging, compression and base-reuse machinery that decides whether a resume takes
2 s or 30 s, and §3.2 shows how easily a wrong measurement of it misleads.
Code ports; operational evidence does not.

## 4. Policy overlap — adopt mechanisms, own policy

`lib/autostandby` and `lib/scheduledsnapshots` overlap Barista's R-SNAP-3, but
should **not** be adopted:

- `--auto-standby-enabled` is documented **Linux-only**, so it is absent exactly
  where the dev loop runs.
- It keys on **inbound TCP activity**. Barista's TTL resets on *guest/session*
  activity — exec, file ops (B33, nap-003). A session doing long local work with
  no inbound traffic would be standby'd by their signal and kept alive by ours.

Use `standby`/`restore` as mechanisms; keep the decision in Barista's reconciler,
where nap-003 already put it.

## 5. Dependency shape — **control plane, not data plane**

What Barista would depend on: one long-lived Go daemon (`hypeman-api`) speaking
OpenAPI 3.1 / JSON over localhost (default `:4973`), bearer-token auth, API
version `0.3.0`.

**The measurement that matters.** `vz-shim` runs as a child of `hypeman-api` but
in its own process group. Killing the API with `SIGKILL`:

- the VM **survived**, reparented to PID 1;
- the guest ran **continuously** — uptime 45.93 s → 64.50 s straight through the
  kill, so no restart and no interruption;
- launchd restarted the API, which **re-adopted** the instance as `Running`, and
  `exec` worked again against the same guest.

So the daemon's death costs *management availability*, never running sessions.
That mirrors Barista's own crash-safety property (`kill -9` the Node Agent, sandboxes
survive, recovery reconciles) and it is the single most important fact about this
dependency.

**Costs, honestly:**

1. **No Rust SDK.** Stainless targets Go and TypeScript only, so Barista needs an
   OpenAPI 3.1 → Rust client (`progenitor`, or hand-rolled) — a second codegen
   toolchain beside buf/prost.
2. **A second daemon per node** to ship, configure and supervise, with its own
   data dir and service unit. Verified under launchd only; systemd untested.
3. **Two records of the same instance** — hypeman's metadata plus Barista's journal,
   so divergence is possible. This is the real design work. But Barista already built
   the reconciliation for exactly this shape: the node-scoped `list_labeled`
   sweep from nap-003 extends to it rather than being new.
4. **Control-plane availability must be surfaced, not hidden.** While the daemon
   is down every Barista mutation fails, and Barista should report that as an explicit
   degradation rather than looking broken. `/health` and `/resources` endpoints
   exist, so a truthful signal is cheap — and `/resources` may also serve
   `GetNodeInfo`'s inventory.
5. **Pre-1.0 API (`0.3.0`)** — breaking changes are likely; pin and track.
6. **A bearer token for a local daemon** — one more secret in the Node Agent's
   configuration.

**A reframe worth stating**, because "a Go daemon from a Rust codebase" sounds
worse than it is: an out-of-process HTTP dependency is *looser* coupling than
linking a library. It cannot corrupt Barista's address space, it can be versioned,
restarted and replaced independently, and Barista already expects to shell out to
runtime binaries (`runsc` would too). The genuinely new thing is a **shared,
stateful** daemon rather than per-instance processes — which is why item 3 is the
actual work, and the language boundary is not.

## 6. Recommendation — **RATIFIED (task 4.5, 2026-08-06)**

Adopt hypeman as a **Contract B backend** (not wholesale, not exclusively),
keep Contract A/C, the journaled ops model and session policy as Barista code, and
retain the `Runtime` trait seam so a `runsc` backend stays possible for a
shared-kernel tier. Do **not** clone the substrate.

Gates 1.1–1.4 pass, T7 semantics pass on a laptop, fork works, idle cost is free
in compute and sub-linear in disk, and a realistic 1.5 GiB session resumes in
~2 s. On the evidence gathered, nothing disqualifies hypeman.

Remaining work before ratification — none of it currently looks
recommendation-flipping:

1. **Task 3.4** — restore latency on firecracker/Linux with UFFD at 1–4 GiB.
   Now a question of *how much better* production is than 2 s, rather than
   whether the approach is viable at all.
2. Re-measure the 29.21 s outlier under controlled host-memory conditions, or
   drop it.
3. ~~Dependency shape~~ — **resolved, see §5.** The daemon is control plane
   only: killing it does not disturb running sessions and it re-adopts them on
   restart. Residual cost is an OpenAPI→Rust client, a second daemon per node,
   and reconciling two records of one instance.
4. No shared-kernel tier for the voice-agent runtime density (§11.3); arm64-only evidence.

**One finding does sharpen the recommendation toward dual-tier.** §2.1 shows
hypeman has no live checkpoint, so `live_checkpoint: false` and **T2 is
unachievable** on it. Pause/Resume — the north star — is unaffected. But if any
consumer needs a snapshot without pausing, `runsc` must stay in the plan rather
than being demoted, which argues for adopting hypeman *alongside* a `runsc`
backend rather than instead of one.

Correction to BRD §9.10: describing hypeman as "two days old" is wrong in a way
that matters — it is two days *public*, at 344 merged PRs, v0.1.0 since June
2026, running KERNEL's production fleet. The dependency risk is ordinary, and
MIT means the worst case is a fork.
