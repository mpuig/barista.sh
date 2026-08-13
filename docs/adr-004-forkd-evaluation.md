# ADR-004 forkd evaluation — no substrate change

> Status: **PROPOSED — desk review only (2026-08-13).** Nothing was installed
> and nothing was measured: every claim about forkd below is read from its
> README and docs, not exercised. That is enough to answer the routing question
> this ADR exists for; it is not enough to rank forkd's published numbers
> against ours.
>
> Subject: `deeplethe/forkd` ~v0.5 (alpha), Apache-2.0, Rust. A single-node
> microVM sandbox runtime built on a **vendored fork of Firecracker**
> (`deeplethe/firecracker:forkd-v0.4-mem-backend-shared-v1.12`; their
> `MAP_SHARED` memory-backend proposal is pending upstream).

## 1. The question

forkd's headline — fork 100 children from a warm parent snapshot in ~101 ms at
~0.12 MiB resident per child, and live-BRANCH a *running* VM with ~56 ms p50
source pause — is the strongest public demonstration of the CoW/fan-out end of
the snapshot design space Barista also lives in. Does any of that justify
replacing `hypeman` under Contract B, or adding forkd beside it?

## 2. What forkd is

A parent VM boots once with warm state (imports, JIT, model weights), pauses to
a Firecracker snapshot (`memory.bin` + vmstate); each child is a separate
Firecracker process that `MAP_PRIVATE`-mmaps the parent image, so the kernel
does page-level copy-on-write. On top of that:

- **Live BRANCH** (v0.4): UFFD write-protect captures dirty pages out of band,
  so a running source is paused ~56 ms p50 / 64 ms p90 (1.5 GiB source) and
  with `wait: false` the caller returns in ~70 ms. Requires Linux ≥ 5.7 and
  `vm.unprivileged_userfaultfd=1` or `CAP_SYS_PTRACE`.
- **Snapshot chains** (v0.5): layered diff snapshots (`parent_tag` +
  content-hash edges, e.g. base → +numpy → +pandas) assembled transparently at
  spawn.
- REST + bearer auth, Python/TS SDKs (E2B-compatible), an MCP server,
  Prometheus metrics, per-child netns, cgroup v2 memory limits, a packed-
  snapshot hub (`.tar.zst` push/pull).
- Explicitly out of scope, by their own list: multi-node scheduling, egress
  policy, cpu/io/pids quotas. No hosted offering; x86_64 Linux + KVM only.

## 3. Gates (ADR-001 §1, applied at desk level)

| Gate | `hypeman` | forkd |
|---|---|---|
| OCI image materialization (spec: sessions are digest-pinned OCI) | yes | **none** — the distribution unit is a packed snapshot, not an image |
| Guest-agent injection (spec §7: entrypoint wrap + token delivery) | `--entrypoint` / `--env` | no documented equivalent; parent images are hand-built |
| Memory snapshot + restore of a **named, durable instance** | yes (pause/resume + named snapshots, nap-015) | snapshots exist, but the designed flow is parent → ephemeral children, not durable identity |
| Node-scoped sandbox labels (zero-orphan sweep, barista-034) | `--tag` | not documented |
| Contract B honesty surface (`SnapshotRef.kind`, `stop_status`, sweeps) | mapped | would all be glue we write against a REST API that does not speak it |
| Maturity | ratified (ADR-001, 2026-08-06) | alpha; on-disk formats and API shapes explicitly unstable |

Two gates fail outright (OCI materialization, labels/sweep), one is DIY (guest
injection), and the whole thing sits on a fork of Firecracker that upstream has
not accepted. ADR-001 rejected substrates for less.

## 4. Why the attractive parts do not force a substrate change

1. **hypeman already embeds Firecracker** (`--hypervisor firecracker`), and its
   own capability registry marks Firecracker as the only backend with
   `SupportsSnapshotBaseReuse` (ADR-001 §1.2) — the primitive underneath
   fork-from-base. The machinery forkd is built on is reachable inside the
   substrate we already run.
2. **Contract A already reserves the capability bits** — `cow_fork`,
   `lazy_restore`, `live_checkpoint` (`specs/phase1-runtime-interface.md`
   capability sketch; `cow_fork` = Firecracker B39). Fan-out was designed to
   arrive as a capability of the existing seam, not as a new substrate.
3. **The UFFD end of this space was probed and reverted**: nap-005 task 5.5
   ran `firecracker_snapshot_memory_backend: uffd` and got 5–15% on resume,
   partly inside run-to-run noise, with one of six runs killing the VMM
   (`upstream-hypeman-findings.md`, "Substrate state on the nap-linux dev VM";
   reverted 2026-08-08). The frontier forkd lives on is not yet stable even via
   stock Firecracker.

## 5. Decision

Keep `hypeman` as the sole production Contract B backend. Route fork-shaped
capability work through the embedded Firecracker hypervisor and the reserved
capability bits. Do not build a forkd `Runtime` backend now.

Revisit only if all three hold: (a) upstream Firecracker lands the
`MAP_SHARED` memory-backend patch, removing the fork-of-a-fork; (b) fan-out or
session branching becomes a product requirement on the control-plane side; and
(c) forkd grows OCI materialization, or we accept owning an image→snapshot
build pipeline ourselves.

## 6. Worth importing regardless of the decision

- **Snapshot chains.** Layered, content-hash-linked diff snapshots are the
  shape a "warm agent image" feature would take (base → +toolchain →
  +project); today the control plane installs tooling at first boot.
- **Live-BRANCH semantics.** Capture-without-stopping is the UX to hang on
  `live_checkpoint` whenever a backend can honour it: "branch a running
  session" rather than "pause, copy, resume".
- **Host preflight** (`forkd doctor`, 17 checks) validates a pattern Barista
  already has (`barista doctor`, hypeman preflight) — no action.
