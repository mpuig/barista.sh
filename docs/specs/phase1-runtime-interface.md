# Phase 1 — Runtime Interface Specification

> Status: **Draft v0.10** — v0.10: §9's T4 row records that it is **not
> satisfied**. Phase 1 closed on "T1–T12 except T2 and T11", and T4 is the one
> row that closure was wrong about: no change ever claimed it, so a
> runtime-level test stood in for a gRPC-level one, and at the gRPC boundary the
> default pause is refused rather than degraded. Found while auditing the claim,
> and independently by `barista-026-pause-degradation-parity`, which now claims
> the row. v0.9 (nap-011/OQ10): `TemplateRef` carries exactly
> one artifact kind — the `oneof` collapses to a plain `OciImageRef`, `RootfsRef`
> is deleted with tag and name `reserved`, and the digest becomes required
> (`INVALID_SPEC` when empty; the tag is a label, never identity). CONVERT
> annotations retired throughout. v0.8: T9 delivered as specified (§9, nap-010) and §10's
> open sub-questions are consolidated against ADR-001 v2: items 1–4 are resolved
> (transport is plain TCP on the rank-1 substrate; one stream per exec shipped;
> `cpu_class` ships as a flags hash; N-times restore is proven — T9 restores the
> same bytes twice), items 5–6 are deferred with the rank-2 `runsc` tier whose
> internals they describe. v0.7: T7's runtime column corrected to the rank-1
> substrate (`hypeman`) and the T7 session clarified as *driven through* `Exec`
> but hosted as the instance's workload; first NFR-1 measurement recorded in the
> BRD. v0.6 (ADR-001 v2 / constitution v1.3.0): **T2 and T11 deferred** with the
> rank-2 `runsc` tier; T9 must draw inside the `POST_RESTORE` hook. v0.5:
> T7/T11 workload revised: the driving acceptance case
> is an **ACP agent session**, not a REPL — it is
> where all four consumers converge, and it makes `post_restore_cmd` load-bearing
> rather than theoretical (§7, B26). T11's timed delta now runs against a stubbed
> model backend so it measures the sandbox, not API latency, and keeps a separate
> ptrace probe. v0.4: verification evidence added (N-times
> restore, B14/overlayfs, `/proc/gvisor/spec_environ`), entropy-reseed mechanism
> marked runtime-specific `[VERIFY]`. v0.3: abstraction level revised to OCI per **BRD ADR-001**
> (`runsc` as first real runtime); `TemplateRef` becomes a `oneof`;
> `hardware_isolation` added to capabilities. Earlier forks resolved in v0.2:
> separate `Checkpoint`/`Pause` verbs · SQLite (WAL) operation journal ·
> fork-on-resume deferred to v1alpha2. Scope: Phase 1 (Node Agent + pluggable
> runtime, no Control Plane). Companion to [`../BRD.md`](../BRD.md) — references
> (B*, FR-*, Risk *, §*) point there.
>
> Driving acceptance case: *an agent session runs inside a Barista instance,
> pauses while idle, and resumes with its in-memory context intact* (§11.1).

---

## 1. Purpose & scope

Defines the three contracts that make the runtime pluggable and the platform
crash-safe from day one:

| Contract | Boundary | Consumers |
|---|---|---|
| **A — Node Agent API** | gRPC over TCP/UDS | Phase 1: `barista` CLI · Phase 2+: Control Plane (Python) |
| **B — `Runtime` trait** | Rust trait, in-process | Implementations: `runsc` (gVisor — primary, ADR-001), `firecracker` (hardware-isolation tier), `fake` (Docker — tooling only) |
| **C — Guest Agent API** | gRPC over a per-runtime transport: unix socket (`runsc`) · vsock (`firecracker`) · docker exec socket (`fake`) | Node Agent only — never exposed directly |

**In scope:** instance lifecycle, snapshots (local tier only), exec/files,
readiness, TTL, events, operations model, capability discovery.

**Out of scope (deferred):** object-store snapshot tier (R-SNAP-2 remote),
fork/clone (B39 — API leaves room), scheduler, manifests, gateway/port
forwarding, kata runtime, policy engine (OQ8).

## 2. Foundational decisions

1. **Schema-first**: protobuf packages `barista.node.v1alpha1` and
   `barista.guest.v1alpha1` are the single source of truth. Python and Rust
   code is generated; hand-written duplicate types are forbidden (OQ4).
2. **Interface pitched at the OCI-bundle level of abstraction**
   (`[REVISED v0.3 — ADR-001]`; was: the Firecracker level): a workload = image
   reference + runtime bundle + resources + commands. The rule Risk 3 was
   protecting still holds — nothing in the contract may assume Docker
   *networking*, Docker *cgroups*, or the Docker daemon — but the *artifact* is
   an OCI image, which every runtime consumes natively (`[REVISED v0.9]` the
   CONVERT stage and its `RootfsRef` are retired — nap-011/OQ10; the rank-1
   substrate does its own initrd + overlay, ADR-001 v2). Pitching this at the Firecracker level would
   force every runtime through a rootfs pipeline that only one of them needs.
3. **Every mutation is an async `Operation`**, journaled as a durable FSM on
   node-local storage before execution begins (B15, flyd pattern). The Node
   Agent can be `kill -9`ed at any point and recover deterministically.
4. **Capabilities are declared, not assumed** (§5). Degraded runtimes stay
   API-compatible.
5. **Verbs** (B17 + B31/B32): `Create · Start · Stop · Pause · Resume ·
   Checkpoint · Destroy`. `Checkpoint` = live snapshot, instance keeps running
   (E2B `Snapshotting` transient). `Pause` = snapshot + release the VM.

## 3. Instance model

### 3.1 InstanceSpec (immutable after create)

```protobuf
message InstanceSpec {
  string instance_id = 1;            // client-chosen, unique per node (ULID)
  TemplateRef template = 2;          // digest-pinned OCI image + runtime bundle
  Resources resources = 3;           // vcpu, mem_mib, disk_mib
  Process process = 4;               // start_cmd, ready_cmd, env, workdir (B36)
  Hooks hooks = 5;                   // pre_snapshot_cmd, post_restore_cmd + timeouts (B5/B26)
  uint64 ttl_seconds = 6;            // 0 = no TTL; reset on guest activity (B33)
  TtlAction ttl_action = 7;          // PAUSE (default) | STOP | DESTROY
  map<string,string> labels = 8;
}

message TemplateRef {                // [REVISED v0.9 — nap-011/OQ10]
  OciImageRef oci = 1;               // the one artifact kind: every runtime consumes OCI
  reserved 2;                        // held RootfsRef, the retired CONVERT stage's output
  reserved "rootfs";
  string runtime_bundle_ref = 3;     // pinned; contents are runtime-specific (B35)
  string template_hash = 4;          // identity per BRD §12.2
  string arch = 5;                   // aarch64 | x86_64
}

message OciImageRef {
  string image = 1;                  // human-readable label — never identity
  string digest = 2;                 // sha256:… — the identity; required (INVALID_SPEC when empty)
}
```

**`runtime_bundle_ref` contents by runtime** — versioned as a unit, pinned per
build, and required to match exactly on resume (B35):

| Runtime | Bundle = | Template hash = |
|---|---|---|
| `runsc` | (runsc version, guest-agent version) + `[GPU]` NVIDIA driver branch (B43) | hash(OCI digest + bundle + resources + arch) |
| `firecracker` | (kernel vmlinux, Firecracker version, guest-agent version) | hash(OCI digest + bundle + resources + arch) |

Modal reports snapshots are sensitive to **container-runtime version** as well
as driver version (BRD §9.5) — the bundle is what encodes that, so
`BUNDLE_MISMATCH` (§8) is the same check for both runtimes.

### 3.2 State machine

```
                    ┌──────────────── Checkpoint ───────────────┐
                    ▼                                           │
CREATING → CREATED → STARTING → RUNNING ──────────→ CHECKPOINTING
                        ▲          │   │
                        │          │   └─ Pause →  PAUSING → PAUSED
                        │          │                            │
                        │        Stop → STOPPING → STOPPED      │
                        │                  │                    │
                        └──── Start ───────┘        Resume → RESUMING → RUNNING
any transitional state → FAILED (retryable per op journal)
any state → Destroy → DESTROYING → DESTROYED (terminal)
```

Rules:
- `RUNNING` carries a separate `ready: bool` (result of `ready_cmd` via guest
  agent) — readiness is not a state.
- `PAUSED` holds **zero VM resources**: only snapshot files + metadata remain
  on the node.
- `STOPPED` = clean shutdown, **memory lost**, disk preserved. `Start` from
  `STOPPED` is a cold boot.
- TTL expiry triggers `ttl_action` (default `PAUSE`; falls back to `STOP` if
  the runtime lacks `MEMORY_SNAPSHOT`).
- `FAILED` records the failing operation; `Destroy` is always legal.

### 3.3 Snapshot record

```protobuf
message Snapshot {
  string snapshot_id = 1;
  string instance_id = 2;
  SnapshotKind kind = 3;             // MEMORY_AND_DISK | DISK_ONLY (B32)
  string cpu_class = 4;              // restore-compat key (B27)
  string template_hash = 5;          // invalidation key (B29)
  string runtime_bundle_ref = 6;     // must match exactly on resume (B35)
  Tier tier = 7;                     // LOCAL (v1); OBJECT_STORE reserved
  uint64 size_bytes = 8;
  google.protobuf.Timestamp created_at = 9;
}
```

Resume preconditions (checked by the Node Agent, enforced later by the
scheduler): same `cpu_class`, same `runtime_bundle_ref`, template not
invalidated. Violations → `FAILED_PRECONDITION` with a machine-readable reason.

## 4. Contract A — Node Agent service

```protobuf
service NodeAgent {
  // Identity & capabilities
  rpc GetNodeInfo(GetNodeInfoRequest) returns (NodeInfo);
  //   NodeInfo: node_id, arch, cpu_class, runtimes + capabilities (§5),
  //   resources (total/allocatable), agent version.

  // Lifecycle — all return Operation (async, idempotent)
  rpc CreateInstance(CreateInstanceRequest) returns (Operation);   // spec + idempotency_key + require_hardware_isolation (§5)
  rpc StartInstance(InstanceRef) returns (Operation);
  rpc StopInstance(StopRequest) returns (Operation);               // grace_seconds, then kill
  rpc PauseInstance(PauseRequest) returns (Operation);             // keep_memory: bool (default true)
  rpc ResumeInstance(ResumeRequest) returns (Operation);           // by instance_id (latest snapshot) or snapshot_id
  rpc CheckpointInstance(CheckpointRequest) returns (Operation);   // live; instance stays RUNNING
  rpc DestroyInstance(DestroyRequest) returns (Operation);         // keep_snapshots: bool

  // Introspection
  rpc GetInstance(InstanceRef) returns (Instance);
  rpc ListInstances(ListInstancesRequest) returns (ListInstancesResponse);
  rpc ListSnapshots(ListSnapshotsRequest) returns (ListSnapshotsResponse);
  rpc DeleteSnapshot(SnapshotRef) returns (Operation);

  // Operations & events
  rpc GetOperation(OperationRef) returns (Operation);
  rpc WatchEvents(WatchRequest) returns (stream Event);            // state changes, op progress, ttl warnings

  // Guest passthrough (Phase 1 convenience; the gateway owns this later, B25)
  rpc Exec(stream ExecFrame) returns (stream ExecFrame);           // interactive, stdio + resize + exit code
  rpc ReadFile(ReadFileRequest) returns (stream FileChunk);
  rpc WriteFile(stream WriteFileRequest) returns (WriteFileResponse);
}
```

### 4.1 Operations model (B15)

- Every mutating RPC requires `idempotency_key`; retries return the original
  `Operation`.
- An `Operation` = `{op_id, kind, instance_id, state: QUEUED|RUNNING|DONE|FAILED,
  steps[], error?}` journaled to node-local storage (v1: SQLite, WAL mode)
  **before** side effects start.
- On Node Agent restart: journal is replayed; each op either resumes from its
  last durable step or is marked `FAILED` with cleanup executed. **Invariant:
  no orphan VMs, no half-created instances invisible to the API** (Risk 5
  groundwork).
- One in-flight mutating op per instance; conflicting calls →
  `FAILED_PRECONDITION { reason: CONCURRENT_OPERATION }`. `Destroy` may cancel.

## 5. Capabilities & degraded modes

```protobuf
message RuntimeCapabilities {
  bool memory_snapshot = 1;      // runsc: true · firecracker: true · fake: false
  bool disk_snapshot = 2;        // all runtimes: true
  bool live_checkpoint = 3;      // runsc: true (--leave-running) · firecracker: true (B31/B38) · fake: false
  bool guest_agent = 4;          // transport per §1; fake bridges via exec socket
  bool hardware_isolation = 5;   // [NEW v0.3] firecracker: true · runsc: false · fake: false
  bool lazy_restore = 6;         // runsc: --background · firecracker: mmap (B9/B37)
  bool cow_fork = 7;             // firecracker: true (B39) · runsc: [VERIFY] ADR-001 §13.4
}
```

Semantics under degradation — **same API, explicit downgrade** (never silent):
- `Pause` without `memory_snapshot` → `DISK_ONLY` snapshot; `Resume` cold-boots
  (E2B `keepMemory:false`, B32). The returned `Snapshot.kind` tells the truth.
- `Checkpoint` without `live_checkpoint` → `FAILED_PRECONDITION`.
- Callers that require exact resume set `require_memory: true` on
  `PauseRequest` to fail fast instead of degrading.
- **`hardware_isolation` is not a degradation — it is a user-visible property**
  (`[NEW v0.3 — ADR-001 §13.5.2]`). A caller placing untrusted code sets
  `require_hardware_isolation: true` on `CreateInstanceRequest`; a node whose
  runtime cannot honour it fails with `CAPABILITY_MISSING` rather than silently
  placing the workload on a shared kernel. Phase 4 surfaces this as an isolation
  tier in the `Session` manifest, alongside `SnapshotKind` (B32).

## 6. Contract B — `Runtime` trait (Rust, internal)

```rust
#[async_trait]
pub trait Runtime: Send + Sync {
    fn capabilities(&self) -> RuntimeCapabilities;

    async fn create(&self, spec: &InstanceSpec) -> Result<Handle>;   // materialize the sandbox; the substrate owns rootfs/overlay (ADR-001 v2)
    async fn start(&self, h: &Handle) -> Result<()>;                 // boot; guest agent must dial back within start_timeout
    async fn stop(&self, h: &Handle, grace: Duration) -> Result<()>;
    async fn checkpoint(&self, h: &Handle, opts: &SnapOpts) -> Result<SnapshotMeta>; // impl guidance: MAP_SHARED continuous flush (B38)
    async fn pause(&self, h: &Handle, opts: &SnapOpts) -> Result<SnapshotMeta>;      // checkpoint + release resources
    async fn resume(&self, snap: &SnapshotMeta) -> Result<Handle>;
    async fn destroy(&self, h: &Handle) -> Result<()>;
    async fn guest(&self, h: &Handle) -> Result<GuestChannel>;       // unix socket (runsc) | vsock (firecracker) | exec-bridge (fake)
}
```

Implementation notes:
- **runsc** (primary, ADR-001): `create` = OCI bundle from the image digest,
  writable layer via the runtime's own overlay; `checkpoint` = `runsc checkpoint
  --image-path=<unique dir>` (`--leave-running` for the live variant);
  `resume` = `runsc restore --background` so the workload starts once kernel
  state is loaded and remaining pages fault in on demand, prioritized
  (B9/B37 equivalent). **Every image-path must be unique** — the snapshot store
  layout must not reuse directories. `--background` requires
  `--compression=none`, so the local tier trades disk for latency; the
  object-store tier (deferred) may prefer the inverse. No `MAP_SHARED`
  equivalent: expect pause cost to scale with the pages file (100 MiB–10 GiB in
  Modal's fleet, BRD §9.5), **not** the 30–100 ms of B38.
- **firecracker** (hardware-isolation tier): memory file mmap'd `MAP_SHARED`
  from creation (B38) so `checkpoint`/`pause` cost 30–100ms, not 1s/GB;
  hugepages per template config (B36); balloon before snapshot optional (B19).
- **fake** (Docker): `create` = container create from the *same OCI image* the
  rootfs was built from (BRD §12); guest agent injected as entrypoint wrapper;
  `pause` = stop + `docker commit`-equivalent disk state. Purpose: CP/API/tooling
  development on macOS — **never** a semantics reference for snapshots.

## 7. Contract C — Guest Agent (per-runtime transport)

```protobuf
service GuestAgent {
  rpc Health(HealthRequest) returns (HealthStatus);       // liveness + ready_cmd result + activity timestamps (feeds TTL, B33)
  rpc Exec(stream ExecFrame) returns (stream ExecFrame);  // PTY + pipe modes
  rpc ReadFile(ReadFileRequest) returns (stream FileChunk);
  rpc WriteFile(stream WriteFileRequest) returns (WriteFileResponse);
  rpc StatPath(StatRequest) returns (StatResponse);
  rpc RunHook(HookRequest) returns (HookResult);          // PRE_SNAPSHOT | POST_RESTORE, bounded by timeout
}
```

`[ADDED v0.6]` The duties have their own contract surface —
`GuestAgent.RunRestoreDuties(RestoreDutiesRequest)` — because `RunHook` runs the
*workload's* commands and cannot carry host-supplied entropy or host time. The
request's entropy field is **required**: a reseed with nothing to mix cannot
de-duplicate two resumes, so the agent refuses it rather than reporting success.
The response reports `entropy_credited` and `clock_stepped` separately from
`degraded`, so a duty that could not run says so (measured: a Docker-backed
sandbox drops `CAP_SYS_ADMIN` and `CAP_SYS_TIME`, so it can mix without crediting
and cannot step the clock at all).

**Agent duties on restore, in order (automatic, before `POST_RESTORE` hook):**
1. **Reseed entropy** (differentiator — Modal punts on this, §9.5).
   `[MEASURED v0.6 — nap-005 task 1.4]` The rank-1 substrate does nothing here: it
   configures no virtio-rng device and its guest init does not touch the RNG on
   restore. Mechanism: the host sends fresh bytes over Contract C on resume and the
   guest agent mixes them via **`RNDADDENTROPY`**, which credits entropy;
   `RNDRESEEDCRNG` alone is insufficient because it reseeds *from* the snapshotted
   pool. `runsc` remains `[VERIFY]` — `/dev/random` is served by the sentry's
   userspace kernel, so in-guest `RNDADDENTROPY` may not affect sentry RNG state.

   **Do not read a passing T9 as proof of safety.** Two resumes of one snapshot were
   measured drawing *different* bytes with no reseed implemented at all, because
   Linux's CRNG mixes in interrupt timing and two guests diverge within seconds.
   T9 must therefore draw inside the `POST_RESTORE` hook (§9).
2. **Step the clock** (chrony/`clock_settime` from host-provided time).
3. Re-verify network reachability; emit `Restored` event with drift metrics.
4. Run `post_restore_cmd` (workload reconnects sockets — B26; provider sockets
   never survive restore, §11.3).

Pre-snapshot: agent runs `pre_snapshot_cmd` (quiesce: flush buffers, close what
can't survive) with timeout; on timeout the snapshot proceeds and the result is
recorded in `Snapshot` metadata.

Bootstrap: agent is PID-1-adjacent, dials the host, and authenticates with a
per-instance token. The guest never accepts inbound connections. Per runtime
(`[REVISED v0.3 — ADR-001 §13.5.3]`):

| Runtime | Injected at | Transport | Token via |
|---|---|---|---|
| `runsc` | container create (entrypoint wrapper over the unmodified OCI image) | unix socket bind-mounted into the bundle | env / mounted file |
| `firecracker` | ~~CONVERT (BRD §12.2)~~ `[REVISED v0.9]` the substrate's own initrd (ADR-001 v2) | vsock, fixed port | kernel cmdline / MMDS |
| `fake` | entrypoint wrapper | docker exec socket | env |

`[NEW v0.3]` gVisor additionally exposes **application-driven checkpointing**
via `/proc/gvisor/checkpoint` (enabled by OCI annotation), which Firecracker has
no equivalent of. A workload can request its own snapshot at a
semantically-safe point — strictly better than a `pre_snapshot_cmd` timeout
race. Treat it as an optional extension to `RunHook`, never as the only path:
the platform must still be able to snapshot uncooperative workloads.
`[NEW v0.4]` gVisor also exposes **`/proc/gvisor/spec_environ`** — the workload
can read restore-time environment — a natural channel for the guest agent to
publish fresh values (new tokens, host time, session metadata) to
`post_restore_cmd` consumers.

## 8. Error model

Canonical gRPC codes + machine-readable `reason` in details:
`INVALID_SPEC · TEMPLATE_NOT_FOUND · BUNDLE_MISMATCH · CPU_CLASS_MISMATCH ·
CAPABILITY_MISSING · CONCURRENT_OPERATION · GUEST_UNREACHABLE · HOOK_TIMEOUT ·
RESOURCES_EXHAUSTED · SNAPSHOT_INVALIDATED`.

**Restore is never a correctness dependency** (`[NEW v0.3]` — B42, Modal): when
`Resume` fails for any snapshot-related reason (`BUNDLE_MISMATCH`,
`CPU_CLASS_MISMATCH`, `SNAPSHOT_INVALIDATED`, corrupt image), the Node Agent
**falls back to a cold boot from the template** and reports the degradation on
the `Operation` and as an event — it does not fail the instance. Callers that
genuinely require exact resume opt out with `require_memory: true` (§5) and get
the error instead. This keeps a bad snapshot from taking a session down.

## 9. Acceptance tests (Phase 1 DoD)

`[REVISED v0.6 — ADR-001 v2]` **T2 and T11 are deferred** with the rank-2 `runsc`
tier; Phase 1 claims the rest (Constitution §I, amended v1.3.0). The snapshot
tests now run on `hypeman`, which does memory pause/resume on any of its
backends — including Apple Silicon `vz`, so they run on a developer's laptop.

`[REVISED v0.3 — ADR-001 §13.5.8]` The snapshot tests move off Firecracker onto
`runsc`, which is what makes this DoD reachable: they now run on any Linux CI
runner instead of requiring nested virt or a bare-metal box.

| # | Test | Runtimes |
|---|---|---|
| T1 | Full lifecycle create→start→ready→stop→start→destroy | runsc + fake |
| ~~T2~~ | ~~`Checkpoint` while a counter process runs; counter never stops; snapshot restorable~~ **`[DEFERRED v0.6 — ADR-001 v2]`** rank-1 `hypeman` has no live checkpoint (it pauses, copies, resumes), so `Checkpoint` fails with `CAPABILITY_MISSING` there. Arrives with the rank-2 tier when a consumer needs a snapshot without pausing | ~~runsc~~ (rank-2 tier) |
| T3 | `Pause`/`Resume` with memory: in-memory counter continues; `/proc/uptime` proves no reboot | hypeman |
| T4 | `Pause`/`Resume` degraded (`DISK_ONLY`): disk state survives, process cold-restarts; `Snapshot.kind` honest. **`[NOT SATISFIED — RECORDED v0.10]`** and it is the one row Phase 1 closed over. Half of it holds: `a_pause_stops_the_container_keeps_its_disk_and_reports_disk_only` (`crates/barista-node-agent/tests/fake_runtime.rs`) proves `FakeRuntime::pause` stops the container, keeps the disk and reports `DISK_ONLY` — boot log line 1 surviving is the disk, line 2 appearing is the cold restart. But it calls the runtime **directly**, one layer below the gate, and these tests are gRPC-level by design (constitution III). At the gRPC boundary `PauseInstance` refuses: `keep_memory` defaults to `true`, and `service.rs` answers `FAILED_PRECONDITION`/`CAPABILITY_MISSING` when a runtime cannot honour it — so a default pause on `fake` never reaches the degraded path this row describes. No change ever claimed T4, which is how a passing runtime-level test came to stand in for an acceptance test that does not pass. Claimed and resolved by `barista-026-pause-degradation-parity` | fake |
| T5 | `kill -9` Node Agent mid-`Create` → restart → op resolves deterministically; zero orphan sandboxes/containers | all |
| T6 | TTL expiry → auto-`Pause` (fake: auto-`Stop` fallback); activity via guest agent resets TTL | all |
| T7 | **agent-session scenario**: ACP agent session, pause 60s, resume — session continues with its in-memory conversation context; `post_restore_cmd` reconnects the provider socket (B26). `[REVISED v0.7]` Runtime column was `runsc (Lima, no nested virt)`, which predates ADR-001 v2 — the rank-1 substrate is `hypeman` and `runsc` is a deferred rank-2 tier. Also: the session is driven **through** `Exec` but is not *hosted* by it. `Exec` spawns a new process per call and a pause severs the stream, so an exec-hosted session is the one thing a pause cannot preserve; the session is the instance's workload and each `Exec` is a client of it (nap-006 task 3.2) | hypeman |
| T8 | Resume with mismatched `cpu_class` or `runtime_bundle_ref` → cold-boot fallback + degradation event; with `require_memory: true` → `FAILED_PRECONDITION`, no partial boot | hypeman |
| T9 | Two resumes from one snapshot produce **different** post-reseed random values in guest, **drawn inside the `POST_RESTORE` hook** — `[REVISED v0.6]` drawn via a later `Exec` the test passes with no reseed implemented (measured, task 1.4), because two live guests diverge within seconds. `[DELIVERED v0.8 — nap-010]` literally as written: one **explicit** substrate snapshot, the same bytes restored twice (`t9_the_same_bytes_restored_twice_diverge`), the draw inside the hook, and the same-bytes premise itself asserted (the draw file must hold exactly one line per life). Running it found `RNDADDENTROPY` alone insufficient — the CRNG's ChaCha key and reseed timer restore byte-identical, so draws repeated until the duty also forced `RNDRESEEDCRNG`; the weaker successive-restore variant stays alongside and could never have seen this | hypeman |
| T10 | Idempotency: same `idempotency_key` replayed 3× → one instance, same `Operation` returned | all |
| ~~T11~~ | **`[DEFERRED v0.6 — ADR-001 v2]`** ~~Compatibility gate~~ — this was ADR-001's ratification gate; the ADR was instead ratified on substrate evidence (`docs/adr-001-substrate-evaluation.md`), so T11 now gates only whether the **rank-2 `runsc` tier** is viable for agent sessions. Protocol when it runs: a real ACP agent session as a standard ACP session — agent subprocess over stdio JSON-RPC, own MCP servers, tool subprocesses, package installs — unmodified under `runsc`, recording every syscall-surface failure, plus a separate ptrace/debugger probe and a timed `runsc`-vs-`runc` delta against a **stubbed model backend** | ~~runsc~~ (rank-2 tier) |
| T12 | `[NEW]` `require_hardware_isolation: true` against a node whose runtime lacks it → `CAPABILITY_MISSING`, instance never created. `[REVISED v0.7]` said "a `runsc`-only node"; the Phase 1 runtime without hardware isolation is `fake`. Paired with the **positive** case on `hypeman`, because until a runtime with the capability existed, "fails closed" and "fails always" were the same green | fake (negative) + hypeman (positive) |

## 10. Open sub-questions

1. ~~vsock framing: tonic custom connector vs raw HTTP/2-over-vsock — spike needed.~~
   **Demoted by ADR-001**: only blocks the `firecracker` tier now, since `runsc`
   uses a unix socket (§7). **`[RESOLVED v0.8 — ADR-001 v2]`** dissolved: the
   rank-1 guest channel is a plain TCP dial to the instance's address (nap-005
   task 2.3, verified on Linux), and vsock is the substrate's internal transport,
   not Barista's — owning its framing would violate §13.7.
2. ~~Exec multiplexing: one stream per exec (v1, simple) vs muxed channel.~~
   **`[RESOLVED v0.8]`** one stream per exec shipped (nap-003 §7, exercised by
   T6/T7 and the CLI); revisit only if a consumer measures the per-stream cost.
3. ~~`cpu_class` derivation: CPUID flags hash vs explicit allowlist (Modal needs ~6 classes, B27 — start with flags hash, observe cardinality).~~
   **`[RESOLVED v0.8]`** the flags hash shipped (`node_info.rs`: SHA-256 of
   `/proc/cpuinfo` flags, first 8 bytes). "Observe cardinality" stands as a
   watch item for the cross-host tier, where the class is load-bearing (task 3.5
   enforces it only there).
4. `[NEW v0.3]` ~~**Can one `runsc` checkpoint image be restored N times?**~~
   Blocks fork-on-resume (B39, v1alpha2) and golden-template cloning (B10).
   Docs require unique image-paths per checkpoint and say Docker cannot restore
   into a new container; Modal restores a snapshot repeatedly, so the mechanism
   likely exists below the Docker integration. Spike before designing the fork API.
   `[EVIDENCE v0.4]` gVisor docs state restore *always* targets a new container
   ("restore is a command which parallels start": `runsc create <new id>` →
   `runsc restore --image-path`), i.e. the restriction is Docker's wrapper, not
   the primitive — the spike should now assume N-times works and measure
   concurrent-restore behaviour instead. **`[ANSWERED v0.8 — nap-010]`** on the
   rank-1 substrate, by test rather than spike: T9-as-specified restores **one
   explicit snapshot twice** and asserts both the same-bytes premise and the
   post-reseed divergence (§9 T9); `fork` exists upstream as the endpoint B39
   would consume. The `runsc`-specific concurrent-restore question travels with
   the rank-2 tier.
5. `[NEW v0.3]` ~~**Writable-layer CoW for `runsc`**~~: `cp --reflink` of an ext4
   file (BRD §12.2) has no direct analogue for an OCI overlay. Decide between
   overlayfs upper-dir snapshots, btrfs/XFS subvolume clones, or a
   snapshotter-style layer store. `[EVIDENCE v0.4]` Modal's production answer is
   overlayfs upper-dir over a FUSE lazy-loading lower (B14, verified) — default
   to overlayfs upper-dir snapshots unless the spike disproves it.
   **`[DEFERRED v0.8 — ADR-001 v2]`** retired as Barista's problem on rank 1 (the
   substrate owns initrd + per-guest overlay, §13.7); returns only with the
   rank-2 tier, Modal's answer still the default when it does.
6. `[NEW v0.3]` ~~**`--background` vs `--compression`**~~: background restore
   requires uncompressed images, trading local disk for time-to-first-instruction.
   Local tier likely uncompressed, object-store tier compressed — confirm the
   crossover with real pages-file sizes. **`[DEFERRED v0.8]`** the flags are
   `runsc`'s and go with its tier; the *general* crossover question belongs to
   the Phase 2 object-store tier, where BRD §6's sweep is the first input
   (snapshot compression is opt-in and off by default on the rank-1 substrate).

### Resolved

- ~~Journal store~~ → **SQLite, WAL mode** (§4.1).
- ~~Unified `snapshot(release: bool)` vs separate verbs~~ → **separate
  `Checkpoint` / `Pause`** (§2.5).
- ~~Fork-on-resume in v1?~~ → **deferred to v1alpha2**; `ResumeRequest.snapshot_id`
  keeps the API shape open (B39). Revisit when the agent platform needs agent-exploration forks.
- ~~`ExecStart.user_activity` presence~~ → **default-true, server-side** (nap-003).
  A proto3 `bool` has no presence, so a probe cannot express `false`
  distinguishably from unset: every passthrough call resets the TTL and the flag
  is forwarded to the guest for its own activity clock. Revisit with
  `optional bool` only if a real probe caller needs to opt out.
