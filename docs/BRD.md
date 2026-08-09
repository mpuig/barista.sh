# Barista — Business & Requirements Document (BRD)

> Status: **Draft v0.12** — §9.13 registers the **actor-runtime thesis** (the
> external working document this project is the narrow bet on, reviewed twice
> against primary sources): the session↔actor term map, "durable substrate"
> adopted as the collective noun for journal + snapshot keys + lease objects,
> and the thesis's own persistence×ownership axes placing Barista in the one
> unclaimed cell — process-snapshot persistence with no-control-plane
> ownership, measured where every neighbour is aspirational or partial.
> v0.11 — **ADR-002 ratified**: Phase 2's coordination is
> bucket CAS leases with ETag fencing (`docs/adr-002-coordination-evaluation.md`,
> measured by nap-012). Roadmap rows 2–3 rewritten — no Control Plane service,
> no scheduler service; the north-star milestone becomes *a manifest written to
> the bucket materialises on some node*; OQ9 closes. §2's component table is
> annotated rather than rewritten — its "Control Plane" naming survives as the
> name of a *role* (inventory, manifests) that ADR-002 distributes between the
> bucket and the nodes.
> v0.10 — §9.12 surveys **Cloudflare Durable Objects +
> Containers** against their own docs: the closest *product* neighbour, and the
> source of Barista's sharpest positioning line ("DO + Containers, self-hosted,
> where sleep loses neither memory nor disk" — their sleep loses both). Two
> patterns borrowed (B56 scheduled wake — the missing wake edge; B57
> programmable egress — the substrate already has the knob), two premises
> modified (waking is the platform's: request → alarm → verb; the session
> **name** is the public handle, ids are internal — which makes OQ9's lease
> table and the addressing table the same table), and one deprioritised: the
> object-store tier leaves the Phase 2 critical path (DO ran for years without
> migration; B45 locality + the local tier carry v1).
> v0.9 — §1 records the **design priorities** (low latency and
> simplicity of use) that had been driving decisions without being written down,
> and the rule that follows from them: tier complexity lives in capability
> discovery, never in the consumer's spec. Under those priorities the pause
> freeze — not the restore — is the latency target, which promotes B52 (a
> declared keep-awake lease, so a session that was never idle is never paused)
> above the rest of the borrowed patterns and demotes B46's policy surface.
> OQ10 and OQ11 are answered on the strength of those priorities: `RootfsRef`
> goes, and a session stays one workload. Same revision, second clause: the
> **open-question ledger is consolidated** — OQ3 (latency/size halves measured),
> OQ5 (convert stage retired with ADR-001 v2) and OQ7 (the runsc-first answer,
> superseded) are re-marked against the evidence; Risks 4/6/7/8 likewise; and
> the duplicated `[OPEN]` markers now point at single homes (scale targets →
> NFR-4, platform-state store → OQ4, policy vocabulary → OQ8).
> v0.8 — the 5.5 dirty-memory sweep changes two planning
> assumptions (§6): NFR-1 gains a **pause-freeze budget** — the measured latency
> risk is on the pause side (~1.2–1.7 s/GiB frozen), not restore — and the
> borrowed lazy-restore rationale (B9/B37) is marked as **not reproduced** on the
> rank-1 substrate, so Phase 2 must not inherit it unexamined. Both re-price the
> rank-2 tier's trigger (§13.7): live checkpoint stops being a nice-to-have as
> session memory grows.
> v0.7 — T7 is green and NFR-1 carries its first measurement
> (§6, median 368 ms). §9.2 is re-verified against Rivet's source: its engine
> crates are unusable as libraries, RivetKit is a layer *above* Barista, and its
> internal design docs specify the idle→sleep edge better than anything else
> surveyed — B51–B54. §9.1 is re-verified against `celld.dev`: it coordinates
> through an S3 bucket with no control plane and no consensus, which §1's tiers
> make the leading Phase 2 candidate (OQ9, sharpened). New open questions 10
> (`RootfsRef` and §12's convert stage) and 11 (one workload per session, or a
> pod shape?).
> v0.6 — §1 fixes the **deployment targets** as a binding
> constraint (three privilege tiers, from Hetzner bare metal to Fargate), which
> implies a fourth `process` runtime and makes the honest-capability model
> load-bearing rather than decorative. §9.11 adds **Agent Substrate**
> (`agent-substrate/substrate`, Apache-2.0) — the closest neighbour yet, which
> resolves §9.9's `[OPEN]` and restates the differentiator: not "portable vs
> GKE-only", but "runs where there is no cluster". B44–B50 are borrowed from it.
> v0.5 added `hypeman` (§9.10) as a candidate **substrate** rather than a
> competitor, resolved by measurement in `nap-004-runtime-substrate-spike`.
> Earlier revisions are tracked by inline `[REVISED v0.2]` / `[NEW v0.3]` /
> `[EVIDENCE v0.4]` markers. Items marked `[ASSUMPTION]` were inferred, not
> stated; items marked `[OPEN]` need a decision.

---

## 1. Vision

A compute platform that orchestrates lightweight isolated sandboxes — `[CORRECTED
v0.9]` an adopted hypervisor substrate (`hypeman`) by default, gVisor (`runsc`)
where a shared kernel or live checkpoint is wanted (**ADR-001 v2, §13.7**) — with an
**intent-based, declarative developer experience**. Developers declare *what* they
want (services, sessions, resources, lifecycle policies); the platform decides *how*
(placement, scaling, snapshotting, identity, networking).

**Key differentiator:** interruptible, resumable sessions — live memory state can be
snapshotted and restored so a workload resumes *exactly* where it left off, at a
fraction of the cost of keeping it running or re-initializing from scratch.

**Target (decided, v0.1):** session-centric compute — workloads that are named,
long-lived, single-writer sessions with expensive in-memory context. Three
personas share this core: **(a) AI-agent sandboxes** (beachhead), **(b) cloud dev
environments**, **(c) stateful interactive apps / long-lived agent sessions**.
General stateless PaaS is explicitly **out of scope** — it is the one segment
where memory-snapshot durability adds least value. The platform's core manifest
resource kind is the **`Session`**.

**Deployment targets (decided, `[NEW v0.6]`):** Barista must run on **AKS, EKS,
OpenShift, AWS Fargate, Azure Container Apps, DigitalOcean droplets, Hetzner
bare metal, and macOS/Apple Silicon**. This is a binding constraint, not an
aspiration — it is what separates Barista from every platform in §9 that assumes a
cluster it can install node-level components into (§9.11). The set splits into
three privilege tiers, and the honest-capability model (Constitution §I) is what
lets one API span them:

| Tier | Hosts | Isolation available | Best snapshot |
|---|---|---|---|
| **A — full host control** | Hetzner bare metal, droplets, macOS/Apple Silicon | `hypeman` (KVM · `vz`) | `MEMORY_AND_DISK` + hardware isolation |
| **B — cluster with node access** | AKS, EKS, OpenShift | `runsc` via a privileged DaemonSet; KVM only on bare-metal node pools. OpenShift needs a custom SCC — `restricted-v2` forbids it by default | `MEMORY_AND_DISK`, shared kernel |
| **C — serverless containers** | Fargate, Azure Container Apps | no privileged containers, no device access, no DaemonSet, no node to install onto | `DISK_ONLY` at best |

Three consequences:

1. **Tier C has no runtime today** and implies a fourth: **`process`** — no
   sandbox of Barista's own, isolation delegated to the host platform,
   `memory_snapshot: false` reported honestly rather than emulated. It is the
   `fake` runtime promoted to a production tier, which the pluggable `Runtime`
   trait already makes cheap.
2. **The Node Agent stays an ordinary container** with no cluster primitives,
   which permanently rules out a CRD/DaemonSet-shaped design (§9.11).
3. **A consumer learns its tier before it commits** — preflight and `barista doctor`
   report the tier this host grants and why, so degradation is discovered at
   deploy time rather than at the first `Pause`.

`[OPEN]` Which consumer requires Fargate, and whether the *session* must run
inside it or merely call one hosted elsewhere — §11 records neither, and the two
readings need different work. Tier C is unmeasured: whether `runsc` runs in a
Fargate task at all (it permits adding `SYS_PTRACE`, which is the only opening),
and where snapshots live when the task host is itself reclaimable. Both need a
spike before tier C is promised to anyone. If `runsc` is the only tier-C option,
T11 stops being a deferred rank-2 gate and becomes load-bearing for a live
consumer.

**Design priorities (decided, `[NEW v0.9]`): low latency and simplicity of
use.** Where either conflicts with capability or configurability, it wins. Three
consequences, which §6's measurements make concrete rather than aspirational:

1. **The latency target is the pause, not the restore.** The 5.5 sweep shows
   restore flat in working-set size (~370 ms to first workload response at both
   1 and 2 GiB) while pause scales at ~1.2–1.7 s per GiB of *frozen* session.
   Because TTL fires on the platform's initiative and Barista infers idleness from
   passthrough activity, the freeze lands exactly when a consumer is about to
   return — a session waiting on an LLM call makes no RPCs and looks idle — and
   that consumer then pays a restore on top of the freeze it just caused. The
   highest-leverage fix is therefore not a faster snapshot but **not pausing a
   session that was never idle**: B52's declared keep-awake lease, which under
   these priorities outranks the rest of B44–B55. It is also a single concept
   with a working default, which is the other half of the goal.
   B46's four-concept policy surface is demoted to match: take the mechanism,
   expose one knob with a sound default, and let a consumer's demand earn the
   rest.
2. **The complexity of the three tiers above lives in capability discovery,
   never in the consumer's spec.** One `InstanceSpec` on every target;
   preflight, `barista doctor` and `RuntimeCapabilities` report honestly what this
   host granted and why. This is the rule that stops each new tier from adding a
   field to the contract, and it is what keeps "eight platforms" a deployment
   fact rather than an API surface.
3. **Fewer concepts beats more capability at equal cost.** OQ10 (`RootfsRef`,
   which no runtime implements) and OQ11 (pod shape vs one workload per session)
   both point against the extra concept under this rule, and B55 (require the
   digest) is favoured precisely because it removes a way to be wrong rather
   than adding a way to be right.

---

## 2. System Architecture

### 2.1 Components

| Component | Responsibility |
|---|---|
| **Node Agent** | Runs on each host. Creates, stops, destroys instances. Reports node resources and instance status. Executes snapshot/restore operations. |
| **Runtime (pluggable)** | Abstraction behind the Node Agent. Implementations: `hypeman` (rank 1 — hardware isolation via KVM/`vz`, tier A), `runsc` (rank 2 — gVisor, shared-kernel, no KVM required, tier B), `process` (`[NEW v0.6]` — tier C: no sandbox of Barista's own, `DISK_ONLY`, isolation delegated to the host platform) and `fake` (Docker or local processes, tooling only — never snapshot semantics). The Control Plane ↔ Node Agent contract is runtime-agnostic. Runtime ranking and rationale: **ADR-001 v2 (§13.7)**; deployment tiers: **§1**. |
| **Control Plane** | Registers Node Agents, dispatches orders, tracks cluster inventory. Source of truth for *platform state*. `[ANNOTATED v0.11 — ADR-002]` This survives as a **role**, not a service: ownership and name resolution live in the bucket's lease objects, inventory is a prefix listing, orders become the nodes' pull loop. A read-model service may reappear when fan-out measurably hurts — rebuildable from the bucket, never authoritative. |
| **Scheduler** | Chooses placement for new/restored instances based on node resources. |
| **Reconciler** | Event-driven state machine. Continuously converges actual state toward desired state. Described as "the heart of the system." |
| **Manifest engine** | Parses declarative manifests; translates intent into desired state. |
| **Network layer** | Service discovery + ingress gateway. HTTP and WebSocket exposure. |
| **Identity & secrets** | Per-service identity with associated permissions. No plaintext secrets. |
| **Snapshot subsystem** | Two-tier memory snapshots (see §3). |

### 2.2 State model

Three distinct state classes, each managed differently:

1. **Ephemeral state** — live memory of an instance. Preserved only via memory
   snapshots.
2. **Persistent state** — databases, files, volumes. Survives instance lifecycle
   independently of snapshots.
3. **Platform state** — Control Plane metadata: instance→node mapping, identity,
   attached volumes, desired/actual status.

`[OPEN]` Storage backend for platform state (etcd, Postgres, SQLite+Raft, …) and
for volumes/persistent state is undecided. `[CONSOLIDATED v0.9]` This is OQ4's
remaining half — answered there or not at all. Phase 1's *node* journal chose
SQLite WAL (spec §4.1), which is a data point, not the answer.

---

## 3. Snapshot & Session Subsystem

### 3.1 Requirements

- **R-SNAP-1** — An interactive session can be paused and later resumed with its
  full memory state intact.
- **R-SNAP-2** — Two snapshot classes:
  - **Local (hot):** stored on the same node; fast; used for short pauses.
  - **Remote (durable):** stored in an object store; slower; survives node loss;
    enables cross-node session migration. `[DEPRIORITISED v0.10]` Off the
    Phase 2 critical path, demand-driven: DO sustained a massive product for
    years with entities pinned to their creation site and no migration (§9.12),
    B45's locality pin plus the local tier cover the three internal consumers,
    and the 5.5 sweep already retired the lazy-restore rationale this tier
    borrowed (B9/B37, §6). It returns when a consumer loses a node and it
    hurts — R-SNAP-1's promise stands, its schedule moves.
- **R-SNAP-3** — Snapshot lifecycle is **policy-driven and declarative**. Example
  intent: *"this session may sleep 20 minutes before being snapshotted."* The
  platform — not the developer — decides when to pause, snapshot, tier, and destroy.
- **R-SNAP-4** — Restore latency must beat cold initialization for the target
  workloads. ~~`[OPEN]` No quantitative SLO defined (see §7, Risk 2).~~
  `[MEASURED v0.8–v0.9]` NFR-1 now carries draft SLOs *and* first measurements
  (§6: restore ~370 ms flat in working-set size; pause 1.2–1.7 s/GiB with its
  own draft budget). What remains open is the >1 GiB pause budget, recorded at
  NFR-1 deliberately.

### 3.2 Proposed policy tiers (to be specified)

`running → paused (in-memory) → local snapshot → object-store snapshot → destroyed`

`[OPEN]` Policy vocabulary, where policies live (manifest? API?), and default tiers.
`[CONSOLIDATED v0.9]` This is OQ8 — one question, answered once, and v0.9's
priorities already bound it: B52's lease is promoted, B46's four-concept surface
is demoted.

### 3.3 Known hard problems (not yet addressed — see §7)

- ~~Docker/process fake runtime cannot do memory snapshots (CRIU is experimental) —
  the differentiating feature is untestable in the dev runtime.~~
  **`[LARGELY RESOLVED v0.2 — ADR-001]`** The premise holds for `runc`+CRIU
  (CRIU must reconstruct state through the *host* kernel's `/proc`), but gVisor
  sidesteps it: the userspace kernel owns all guest state and serialises itself
  (§9.5). A `runsc` dev runtime on any Linux VM does real memory snapshots, so
  the differentiator is exercisable in development and CI.
- Firecracker snapshot-restore caveats: guest clock drift, duplicated entropy/RNG
  state (security issue if a snapshot is restored more than once), MAC/IP identity,
  in-flight TCP connections, open file descriptors to external resources.
- Snapshot consistency with attached persistent volumes.

---

## 4. Delivery Roadmap

| Phase | Scope | Definition of Done |
|---|---|---|
| **1** | Node Agent — instance lifecycle | ~~Create, stop, destroy a test instance on demand.~~ **`[CLOSED 2026-08-08]`** DoD was the spec's acceptance tests **T1–T12 except T2/T11** (deferred with the rank-2 tier, constitution v1.3.0), including the north-star agent-session scenario (T7: five runs, same digest across a 60 s pause). Spec: [`specs/phase1-runtime-interface.md`](specs/phase1-runtime-interface.md). |
| **2** | Fleet coordination `[REWRITTEN v0.11 — ADR-002]` | Sessions addressed by **name** fleet-wide: CAS ownership leases with epoch/ETag fencing in an object-store bucket (B1), name→owner resolution from the same object, node **pull** loop (B6), inventory as prefix listing, per-node event fan-out. No Control Plane service, no consensus. DoD: two nodes, one bucket, a contended name — exactly-one-owner holds under kill -9, and a session resumes on its owning node. Single node keeps working with **no bucket at all**. **`[BUILT AND VERIFIED 2026-08-08 — nap-017]`** The DoD above is met, tested against MinIO, and the inherited first task is closed: the matrix ran against a real Cloudflare R2 bucket and R2 honours both conditional-write primitives cleanly, with the fencing property holding under ±3 s clock skew (ADR-002 §3.1). AWS S3 and Azure Blob stay documented-and-unmeasured, but the design no longer rests on one vendor. **One constraint became load-bearing on the way:** measured from a laptop against R2's public endpoint, the wake path costs ~390 ms p50 — longer than the restore it precedes — so "nodes and bucket share a region" is a deployment requirement rather than a preference (ADR-002 §3.3). |
| **3** | Placement & cross-node reconciliation `[REWRITTEN v0.11 — ADR-002]` | "First node with fit" placement (B16) with scalar load (B20) and B45's locality pin (a node-local pause is a hard filter, and under pull it is physics); reconciliation of orphaned ownership after node loss. **← end of technical MVP.** No scheduler *service* — placement is a rule nodes apply when acquiring, not a component that assigns. |
| **4** | Declarative manifests | Devs describe what they want; the system realizes it. **← functional platform** |
| **5** | Networking | Service discovery + gateway; cleanly expose an HTTP or WebSocket service. `[REFRAMED v0.10]` The gateway is not "networking", it is **the session interface**: waking is the platform's job, triggered in frequency order by request (B7/B44), alarm (B56), then the explicit verb as operator fallback — addressing a session by name is what materialises and wakes it (§9.12). |
| **6** | Security & identity | Every service has an identity and permissions; no plaintext secrets. |
| **7+** | Snapshots & sessions · autoscaling · declarative resources (e.g. databases) · developer experience | Not yet broken down. |

**North-star milestone:** ~~a manifest creates an instance on a node through the
Control Plane~~ `[RESTATED v0.11 — ADR-002]` **a manifest written to the bucket
materialises on some node** — same validation of the base architecture, minus
the component the architecture no longer has.

### 4.1 Local development strategy

- Firecracker requires KVM ⇒ Linux only; no native macOS support.
- **Dev path:** implement the Node Agent against the `fake` runtime (Docker or
  local processes) on the Mac; keep the Control Plane contract identical; swap in
  Firecracker later.
- **Firecracker path on Mac:** lightweight Linux VM. `[SUGGESTION]` Lima/UTM with
  an ARM64 Linux guest — Firecracker supports aarch64 and KVM works inside Apple
  Silicon Linux VMs. Alternatively a small bare-metal cloud box for integration
  tests (most cloud VMs lack nested virtualization).
- **`[REVISED — ADR-001, §13]`** The gVisor (`runsc`) path removes the nested-virt
  requirement entirely: its default `systrap` platform needs no virtualization
  extensions, so a plain Lima guest runs the *real* runtime with *real* memory
  checkpoint/restore. This makes the differentiator testable on the dev machine
  and in ordinary CI, which is the primary mitigation for Risks 1 and 3 and
  shrinks the `fake` runtime's remaining purpose to near-zero.

---

## 5. Functional Requirements (consolidated)

- **FR-1** Instance lifecycle: create / stop / destroy on demand (P1).
- **FR-2** Node registration, heartbeats, resource inventory (P2).
- **FR-3** Remote instance creation via Control Plane on an arbitrary node (P2).
- **FR-4** Event-driven reconciliation to desired state (P3).
- **FR-5** Declarative manifests as the sole developer interface (P4).
- **FR-6** Service discovery + gateway for HTTP/WebSocket (P5).
- **FR-7** Workload identity, permissions, secret management (P6).
- **FR-8** Two-tier memory snapshot / restore with declarative policies (P7+).
- **FR-9** Autoscaling (P7+).
- **FR-10** Declarative managed resources, e.g. databases (P7+).
- **FR-11** Runtime pluggability: `fake` ↔ `runsc` ↔ `firecracker` behind one
  interface (P1). Isolation grade is a *declared capability*, not an
  implementation detail (§13.5).

## 6. Non-Functional Requirements

- **NFR-1** Restore-from-snapshot faster than cold init for target workloads.
  Draft SLOs (proposal, to be benchmarked): local-tier wake p50 < 500 ms–1 s
  (dev envs / interactive apps); object-store resume < 5–10 s (agents, migration);
  pause must be non-blocking for the workload (B31/B38).
  `[AMENDED v0.8]` **Pause-freeze budget added**: on a substrate without live
  checkpoint, pause freezes the session for a duration that scales with dirty
  memory (measured below: ~1.2–1.7 s/GiB). Draft SLO: pause-freeze p50 **< 2 s
  for sessions ≤ 1 GiB dirty**; above that the budget is `[OPEN]` — deliberately,
  because the honest answers are "live checkpoint" (the rank-2 tier, T2) or
  "accept the freeze", and that is a product trade-off, not a number to invent.
  The "non-blocking" clause of the first paragraph is **explicitly deferred with
  T2** (constitution v1.3.0) rather than half-claimed.

  **First measurement `[NEW v0.7 — nap-006 task 4.3]`.** T7's own scenario, five
  consecutive runs: **361.1, 362.9, 368.0, 427.4, 443.4 ms** (median **368 ms**).
  Inside the draft local-tier p50 budget, with the caveats that matter more than
  the number:

  - **What is being timed**: `Resume` submitted → the *session answers a
    JSON-RPC call*. Not "the instance reports RUNNING", which happens earlier and
    is not what a consumer waits for — an instance whose workload has not been
    scheduled yet is not a resumed session.
  - **Conditions**: 512 MiB guest, `cloud-hypervisor`, Ubuntu 24.04 aarch64 under
    Lima nested virtualisation on an M4 Max, hypeman 0.17.0 with the arm64 init
    graft (`docs/upstream-hypeman-findings.md` §1). Nested virt and a small guest
    both flatter this number.
  **Dirty-memory sweep `[NEW v0.7 — nap-005 task 5.5]`**, `firecracker`, same
  host, via `scenario/measure_restore.py`. Medians in ms; `resume op` is the
  `Resume` operation reaching DONE, `1st response` is when the *workload* answers.

  | backend | dirty | pause | resume op | 1st response |
  |---|---|---|---|---|
  | `file` (eager, the default) | 1 GiB | ~1070 | ~285 | ~379 |
  | `file` | 2 GiB | 3287 | 295 | 392 |
  | `uffd` (lazy) | 1 GiB | 1830 | 272 | 366 |
  | `uffd` | 2 GiB | ~3169 | ~247 | ~345 |

  Three findings, and the second is the one that changes planning:

  1. **Resume does not scale with the working set.** Doubling dirty memory from
     1 to 2 GiB moves resume by single-digit percent on both backends — it is
     dominated by fixed overhead at this scale, not by page count. T7's 368 ms on
     a 512 MiB guest and 379 ms here on a 1 GiB one are the same number.
  2. **Pause does scale, at roughly 1.2–1.7 s per GiB, and nothing budgeted for
     it.** NFR-1 is written entirely about restore. On the rank-1 substrate a pause
     is stop-copy-resume (no live checkpoint, constitution v1.3.0), so this is time
     the *session is frozen* — a 4 GiB agent would be unavailable for ~5 s every
     time it idles out. **That, not restore, is the latency risk for T7's
     consumer**, and NFR-1 currently has no target for it.
  3. **UFFD's benefit here is small — 5–15% on resume — and partly inside
     run-to-run noise.** It does not reproduce a dramatic lazy-restore win because
     the `file` backend is *already* in the 200–300 ms band B9/B37 describe;
     firecracker appears to fault pages in on demand either way, and the backends
     differ in who services the faults. UFFD should matter more at larger working
     sets or under host memory pressure — neither of which this host can produce.

  **UFFD is opt-in and off by default**
  (`hypervisor.firecracker_snapshot_memory_backend`, default `"file"`), which is
  worth knowing before quoting any lazy-restore number: a stock node is not on
  that path.

  **Consequences for planning `[NEW v0.8]`** — ratified after the sweep:

  - **Phase 2 must not inherit B9/B37 unexamined.** The lazy-restore rationale
    ("mmap + fault-on-demand is the latency differentiator") did not reproduce
    here: the eager `file` backend is already inside the 200–300 ms band, and
    UFFD moves resume 5–15%, partly inside noise. If the object-store tier's
    design leans on lazy restore, it must first show a measurement where lazy
    restore wins — larger working sets or memory pressure — not cite B9/B37.
  - **The rank-2 tier's trigger is re-priced.** §13.7 defers `runsc` / live
    checkpoint "until a consumer needs a snapshot without pausing". The sweep
    puts a number on what waiting costs: every idle-out freezes a session
    ~1.2–1.7 s per GiB of dirty memory. For agent sessions trending toward 4 GiB
    that is ~5 s per pause, on the platform's initiative (TTL), not the user's.
    The deferral stands, but its condition is now measurable rather than
    hypothetical — revisit when a real consumer's session-size distribution is
    known.

  **Honest gaps in this sweep.** 4 GiB was not reached — the host is a 7.7 GB
  nested-virt VM. One of the six UFFD runs failed with the firecracker VMM dying
  (`fc.sock: connection refused`) with no OOM in the journal and the host at
  586 MB used; cause not established, reported rather than retried away, so the
  2 GiB UFFD figures are two samples not three. The `file` 1 GiB row is two
  observed samples for the same reason (output truncation, not a failure).
  - **The "pause is non-blocking" half of NFR-1 is not met by the rank-1
    substrate and is not claimed**: `hypeman` has no live checkpoint, so a pause
    is pause-copy-resume (constitution v1.3.0 defers T2).
- **NFR-2** Cost optimization via automatic pause/snapshot/destroy tiers.
- **NFR-3** Developers declare intent only; no exposure to placement/runtime mechanics.
- **NFR-4** `[OPEN]` Scale targets (nodes, instances/node, snapshot sizes), availability targets, multi-tenancy/isolation model — all undefined. `[CONSOLIDATED v0.9]` The **single home** for these: OQ3's unmeasured remainder and OQ2's multi-tenancy question resolve here, not in parallel.
- **NFR-5** `[MISSING FROM PLAN]` Observability (logs, metrics, tracing) — not in any phase; needed by P3 at the latest to debug the reconciler.
- **NFR-6** `[NEW v0.6]` Portability across §1's deployment tiers is functional,
  not a packaging detail: one Node Agent binary, no cluster primitives, no
  privilege required to *start* (only to reach tier A or B), and the tier a host
  grants reported through `RuntimeCapabilities` before a consumer depends on it.

## 7. Risks & Critical Review

1. **Differentiator validated last.** Snapshots/sessions — the stated
   differentiator — sit in Phase 7+, and the fake runtime can't exercise them.
   *Mitigation:* put `snapshot()`/`restore()` in the runtime interface from Phase 1
   (fake impl may be stop+state-file or CRIU); run a Firecracker
   snapshot/restore spike **in parallel with Phases 1–3** on a Linux box.
   **`[LARGELY RESOLVED v0.2 — ADR-001, pending T11]`** `runsc` does real memory
   snapshots in dev and CI (§3.3, §4.1); the spike requirement drops to the
   `firecracker` tier only.
2. **"Restore beats cold start" is not universally true.** Firecracker cold-boots
   in ~125 ms; restoring a multi-GB snapshot from an object store can take seconds
   to minutes. The claim holds only when warm state is expensive to rebuild.
   *Mitigation:* define per-tier latency budgets and measure early.
3. **Fake-runtime divergence.** Docker silently provides images, networking, and
   cgroups that Firecracker does not. *Mitigation:* define the runtime contract at
   the Firecracker level of abstraction (kernel+rootfs+vsock/tap), not Docker's.
   **`[REVISED v0.3 — ADR-001]`** Contract level moved to the OCI bundle (spec
   §2.2); the rule survives as "no Docker networking/cgroups/daemon assumptions".
   With `runsc` as dev runtime, `fake` shrinks to tooling-only and the divergence
   surface with it.
4. ~~**Rootfs/image pipeline undefined.**~~ How workloads are packaged (OCI images →
   rootfs? raw block devices? firecracker-containerd?) is unaddressed and affects
   Phases 1, 4, and 7. **`[RESOLVED v0.9]`** answered by OQ5 (OCI universal),
   then the pipeline itself retired with ADR-001 v2 — the substrate consumes
   OCI natively and owns conversion (§13.7).
5. **Reconciler DoD is vague** ("reacts to events"). Specify failure scenarios it
   must handle: node-agent crash, control-plane partition, orphaned instances,
   double-scheduling.
6. ~~**Build-vs-reuse not examined.**~~ firecracker-containerd, Cloud Hypervisor,
   Kata, or building session-resume on top of existing orchestrators were not
   considered. ~~Decision depends on whether this is a product or a learning project. `[OPEN]`~~
   **`[RESOLVED v1.2.0 of the constitution — ADR-001 v2]`** examined by
   measurement (`nap-004` spike) and decided: adopt `hypeman`, own the session
   layer; reimplementing substrate (~35,600 lines for zero differentiation) is
   an explicit non-goal. See also §9 — celld demonstrates a control-plane-free
   alternative to Phases 2–3, which is OQ9's live question, not this one's.
7. ~~**Security of restored snapshots.**~~ Restoring one snapshot twice duplicates RNG
   state and key material — must be a first-class design constraint of FR-8.
   **`[RESOLVED v0.9 — nap-010]`** It is one: restore duties (reseed → clock →
   net → hook) run on every resume, and T9-as-specified exercises the exact
   attack — same bytes restored twice, draws inside the `POST_RESTORE` hook.
   Running it found `RNDADDENTROPY` alone insufficient (the CRNG's key and
   reseed timer restore byte-identical); the duty now forces `RNDRESEEDCRNG`,
   and the test would catch a regression. Residual: neither hypervisor reseeds
   on restore by itself (upstream-issue candidate), so the duty is
   load-bearing.
8. ~~**K8s mode may not support memory snapshots initially.**~~ **`[RESOLVED
   v0.2 — ADR-001]`, resolution deferred `[v0.9 — ADR-001 v2]`.** The risk was an artefact of assuming Firecracker.
   Firecracker needs `/dev/kvm` (privileged DaemonSet + nested-virt node pools,
   e.g. AKS Dsv3+), and Kata offers no guaranteed memory-snapshot support — but
   **gVisor's `systrap` platform requires no virtualization extensions at all**
   and does full memory checkpoint/restore (§9.5, Modal in production
   multi-tenant). K8s mode therefore ships the *full* tier on stock node pools
   via a `runsc` RuntimeClass; the degraded filesystem-only tier (B32) remains
   available as a fallback, not as the K8s ceiling. *Residual risk:* the
   privileged-DaemonSet requirement for the Node Agent itself (host paths,
   snapshot storage) is unchanged, and isolation grade drops from hardware to
   shared-kernel (§13.4). **`[AMENDED v0.9]`** The `runsc`-RuntimeClass
   resolution now belongs to the deferred rank-2 tier (constitution v1.3.0),
   and T11 gates that tier, not the ADR. Until it lands, K8s ships the full
   memory tier only on node pools with `/dev/kvm` (hypeman as privileged
   DaemonSet — §1 tier B) and the honest `DISK_ONLY` tier everywhere else.

## 8. Open Questions (need answers before Phase 2 ends)

1. ~~Who is the user and what is the workload?~~ **Answered (v0.1):**
   session-centric compute for (a) AI-agent sandboxes — beachhead, (b) dev
   environments, (c) stateful interactive apps; stateless PaaS out of scope.
   Consequences: `Session` as core manifest kind; B5/B7/B11/B12/B25/B26/B31–B34/
   B37–B39 promoted to mandatory; egress control + RNG reseed promoted (untrusted
   agent code); autoscaling demoted. Divergence risk: three access interfaces
   (exec/files API vs SSH/IDE vs HTTP/WS) — sequence them, don't build all three
   at once.
2. ~~Product or learning project? Single-tenant or multi-tenant?~~ **Partially
   answered (v0.1):** internal platform first — three concrete in-house consumers
   (§11). De-risks §10.5 ("market gap or technical gap"): the market is initially
   internal. Multi-tenancy still open (the voice-agent product is multi-tenant enterprise).
3. Quantitative targets: resume latency, snapshot size, nodes, instances/node.
   **Partially measured (v0.9):** resume latency and snapshot size have real
   numbers (§6 NFR-1: ~370 ms flat at 1–2 GiB dirty; §13.7 spike: a 1.5 GiB
   session snapshots to 608 MB) plus the pause-freeze budget v0.8 added. The
   unmeasured remainder — nodes, instances/node, availability — is **NFR-4's**,
   which is the single home for scale targets; this entry no longer duplicates it.
4. ~~Platform-state store? Implementation language/stack?~~ **Stack answered
   (v0.1): Python + Rust.** Rust = data plane: Node Agent (B15/B38), runtime
   integrations, in-guest agent (static musl binary, envd-equivalent), gateway
   wake path. Python = control plane: FastAPI API (REST + MCP + CLI, deployctl
   pattern), manifest engine, scheduler/reconciler v1 (migrate to Rust only if
   proven insufficient). **Mandatory mitigation:** schema-first contracts
   (protobuf/gRPC or OpenAPI codegen) between CP ↔ Node Agent ↔ in-guest agent —
   no hand-duplicated types. Platform-state *store* still open (B8 progression
   suggests SQLite/FS → Postgres).
5. ~~Workload packaging format and rootfs build pipeline?~~ **Answered (v0.1),
   then mostly retired (v0.9 — ADR-001 v2):** OCI as the universal input
   artifact stands — it is what made adopting `hypeman` cheap (§13.7) — but the
   4-stage pipeline's *convert* stage and its sub-decisions (ext4 vs erofs,
   artifact store layout, builder placement) are the substrate's problems now,
   not Barista's (§13.7 "retired as Barista's problems"). What survives as ours:
   digest-pinning discipline (B29) and `RootfsRef`'s removal (OQ10).
6. ~~Why not Kubernetes / what exactly is being rejected from it?~~ **Answered
   (v0.1): K8s is a supported deployment target, not the foundation.** Dual-mode
   packaging: standalone binary (the agent platform, local dev, bare metal) + Helm chart with
   Node Agent as privileged DaemonSet (the voice-agent product, the preview-env platform). Not CRD/operator-native:
   session/snapshot lifecycle has no matching kube primitive (KubeVirt is
   VM-centric, no session semantics), and CRD-native would force K8s onto every
   consumer. See Risk 8 for the K8s snapshot-capability tier.
7. ~~Where does production run — bare metal (KVM required) or nested-virt cloud?~~
   ~~**Answered (v0.2 — ADR-001, pending ratification via T11): the question dissolves.**~~
   **Re-answered (v0.9 — §1 + ADR-001 v2):** the v0.2 answer rode the
   runsc-first ranking and did not survive it — rank-1 `hypeman` *does* need
   `/dev/kvm`, and T11 no longer ratifies anything ADR-wide (constitution
   v1.2.0). The durable answer is §1's deployment tiers: **tier B** (anything
   with KVM — bare metal, nested-virt cloud, K8s node pools with `/dev/kvm`)
   is the rank-1 home and runs the full memory tier today; **tiers A and C**
   (Fargate, ACA, bare processes) get the honest degraded tier until the
   rank-2 `runsc` / `process` runtimes arrive. *Still open,* unchanged:
   whether any consumer's tenancy model requires the hardware-isolation
   guarantee specifically (§13.4).
8. Snapshot policy vocabulary and its place in the manifest schema?
9. ~~Could object-store CAS leases (celld-style, §9) replace the Control Plane +
   scheduler for v1, at least for ownership and failover?~~ **Answered
   (v0.11 — ADR-002, ratified 2026-08-08): yes, and not only for v1.**
   Measured by nap-012: the fencing property holds under ±3 s clock skew, the
   wake path fits the latency allowance same-region, contention fails clean,
   and the owned protocol is ~150 critical lines. Roadmap rows 2–3 rewritten;
   the residue (cloud-backend measurement; per-node event fan-out as v1) is
   Phase 2's first work. See `docs/adr-002-coordination-evaluation.md`.
   **`[SHARPENED v0.7]`** This stopped being speculative. §1 fixes tiers A and C
   — Fargate, Azure Container Apps, a lone droplet, a laptop — where there is no
   cluster to host a control plane and no node to install one onto. celld
   (§9.1) runs this class of system with *no* control plane and *no* consensus
   service, coordinating through the bucket alone; §9.11 needs Kubernetes plus a
   sharded Redis/Valkey. The question is no longer "could it?" but "what does
   Barista lose by defaulting to it?" — the honest answer being placement quality
   (B16 is already minimal) and inventory queries. B1 and B3 are the concrete
   mechanisms. `[AMENDED v0.10]` §9.12's premise change collapses two problems
   into one: if the session **name** is the public handle (ids internal), then
   the name→owner table the bucket lease custodies *is* the addressing table —
   coordination and discovery are the same CAS object, which is also exactly
   how a gateway resolves a session (B7/B44).
10. ~~What happens to `RootfsRef` and to §12's *convert* stage?~~ **Answered
    (v0.9): drop the variant.** OQ5 settled OCI as the universal input artifact
    and hypeman consumes OCI natively (§9.10), so no runtime accepts the rootfs
    variant — `runtime.rs:236` and `fake.rs:107` both return
    `TEMPLATE_NOT_FOUND`, which is itself dishonest: the template exists, Barista
    simply cannot consume it. Rather than make the lie polite by reporting
    `CAPABILITY_MISSING`, the variant goes: it was the output of §12's *convert*
    stage, which existed to satisfy a firecracker rootfs pipeline that ADR-001 v2
    removed. §1's "fewer concepts beats more capability at equal cost" decides
    the rest. **Ratified 2026-08-08**; `v1alpha1` breaks formally but no client
    breaks, since nothing could ever have used it. *Consequence:* §12's convert
    stage needs re-examining — if nothing converts, the pipeline is
    build → warm → distribute. Implementation is a change proposal, not an edit
    (workflow + §V).

11. ~~One workload per session, or a pod shape?~~ **Answered (v0.9): one
    workload per session.** `InstanceSpec` keeps its single `Process`; §9.11's
    `containers: []Container` over shared volumes is not adopted. Nothing in
    T1–T12 asks for it, §IV says take the smaller design, and §1's simplicity
    priority settles the rest. **Ratified 2026-08-08.** This is a decision *not*
    to build, so it costs nothing to revisit: the voice-agent runtime (provider sockets, VAD/ONNX
    models) and an MCP server beside an agent are the shapes that will ask
    first. Reopen it then — and note the real question at that point is not "do
    we support sidecars" but **whether a sidecar is a second process inside one
    session or a second session**, which memory snapshots make a consequential
    choice rather than a packaging one.

12. **How does a session carry an *enterprise* identity — one an IdP issues and
    an administrator can revoke — and what does a memory snapshot do to it?**
    `[NEW v0.13]` Raised while reviewing barista-021, which is **not** this: that
    change authenticates the channel *inward* (who may talk to the guest agent,
    against an on-path sibling), with a per-instance anchor Barista mints and
    destroys. FR-7 and Phase 6 are the *outward* question — who the workload **is**
    when it calls Graph, a database, or an internal API, against an authority
    Barista does not control.

    Recorded now because three constraints already exist and should shape Phase 6
    from the start rather than be discovered inside it:

    - **The two cannot share a delivery path.** barista-021 rides the per-instance
      volume, which the substrate can create and delete but **not update** (no
      write-contents operation; the mount is `readonly`). That is correct for a
      credential minted at create and valid for the instance's life. An
      access token lives about an hour. A mechanism that structurally cannot
      rotate must not carry a credential that must.
    - **A memory snapshot captures whatever the workload holds.** A session paused
      on Monday and resumed on Friday comes back with its token live, past the
      life the issuer intended, with nothing noticing. This project has already
      been bitten once by restored state that should not have been reused — T9's
      RNG reseed. It is the strongest argument for **credential brokering**
      (nap-014 design decision 4): what is never inside the VM is never inside the
      snapshot, and the key the agent could exfiltrate is a placeholder. Its
      prerequisite is a mediated egress path that actually enforces, which
      §3.1's measurement says this substrate does not.
    - **barista-021 is an input, not a rival.** A per-instance identity is a
      natural attestation of *who is asking* when a session exchanges for a
      short-lived scoped token, with the session **name** — already the fleet-wide
      public handle (§9.12) — as the principal. The instance identity proves the
      machine; the IdP grants the permissions.

    Open: whether the beachhead consumers need this before Phase 6, since an
    agent that cannot act as anybody is an agent that cannot do much.

## 9. Related Work

### 9.1 celld (denoland/celld) — self-hosted distributed Durable Objects

Same conceptual space (hibernating, resumable, stateful compute on a replaceable
node fleet), opposite architectural bets:

| Axis | celld | This project |
|---|---|---|
| Isolation unit | V8 isolate — JS/TS only, written against the Workers/DO API | `hypeman` VM (rank 1) · `runsc` (rank 2) · `process` (tier C) — **any OCI image, no SDK**. `[CORRECTED v0.7]` The axis that separates the two is SDK lock-in, not packaging: both consume OCI (ADR-001 v2 §13.7) |
| Resume semantics | Restore per-cell SQLite DB; live memory discarded by design | Restore full memory snapshot; exact process resume |
| Coordination | No control plane, no consensus — S3 compare-and-swap ownership leases with epochs/fencing | Control Plane + scheduler + reconciler |
| Scheduling | Pull-based: nodes acquire unowned cells; LRU pressure shedding via watermarks | Push-based placement |
| State size | KB–MB (SQLite, continuously replicated) | Potentially GBs (memory snapshots, tiered) |
| Dev contract | Wrangler/Workers compatibility | Own declarative manifests |

**Takeaways:**
- Differentiation vs. celld: language-agnostic workloads + preservation of
  ephemeral memory state (in-flight computation, loaded models, REPL/session
  context). If the target workload is small-state JS apps, celld already covers it —
  reinforces Open Question 1.
- celld's bucket-lease ownership (CAS + epoch fencing) is a proven, consensus-free
  answer to double-scheduling and failover; evaluate adopting the pattern even if a
  Control Plane is kept for inventory/manifests.
- Continuous state replication is viable for SQLite-sized state but not for GB
  memory snapshots — validates the two-tier snapshot design rather than replacing it.

**`[VERIFIED v0.7 — celld.dev/docs]`** Apache-2.0, JS/TS, early. Now published
as a product rather than a repository, and three things sharpen:

- **It needs no infrastructure at all beyond a bucket.** *"Runs server-side
  JavaScript on your machines and keeps all shared data in an S3-compatible
  bucket that you own"*; the bucket **is** the coordinator — no consensus
  service, no fixed membership list, nodes discover each other through it, and
  *"one atomic write to the bucket gives a node the ownership of a cell"*.
  Tested against Cloudflare R2, standard AWS credential chain. It is the most
  deployment-portable of the three surveyed — more than Barista — and it buys that
  by discarding memory and constraining the workload to one language.
- **Durability is stronger than Barista's, in the one dimension Barista cannot match.**
  Per-cell SQLite replicated to S3 at **RPO=0** — a write is not acknowledged
  until it is in the bucket. That is unreachable for GB-scale memory snapshots,
  which is not a gap but a confirmation of where §2.2's line between persistent
  and ephemeral state belongs.
- **It has the complete idle→wake loop, and the reason matters.** States are
  active → idle → hibernated → inactive, and *"memory holds nothing across these
  transitions"*. celld is the only one of the three with both edges built,
  because discarding memory makes falling asleep cheap. Substrate has only the
  wake edge, Barista only the sleep edge (§9.11): the moment a system decides to
  keep memory, both edges get expensive. One detail worth stealing directly — a
  hibernating cell *"stays on its node"* while it holds WebSocket connections,
  which is B45's locality rule and B54's held-connection rule reached
  independently, by a system with a completely different durability model.

**Consequence for OQ9.** Now that §1 fixes tiers A and C — Fargate, Azure
Container Apps, a lone droplet, a laptop — celld's coordination model is a
better fit for Barista's Phase 2 than §9.11's. Substrate is closer in *premise* but
demands a Kubernetes control plane plus Redis/Valkey with sharding; celld proves
this class of system runs with neither. B1 and B3 were logged when Phase 2 was
still abstract; they are now the default candidate rather than an interesting
pattern.

### 9.2 Rivet (rivet.dev) — actors + agent sandboxes

Commercial platform ("infrastructure for the agentic era"), YC/a16z-backed. Three
surfaces: **RivetKit actors** (TS-first, Rust beta — durable state, realtime,
built-in hibernation), **Rivet Engine** (open-source Rust control plane: actor
lifecycle, message routing, APIs; storage backends FS → Postgres → FoundationDB),
and **agentOS/Sandbox + Rivet Cloud** (managed agent sandboxes: filesystem, shell,
tools).

| Axis | Rivet Actors | This project |
|---|---|---|
| Durability model | Serializable `c.state` / per-actor SQLite, auto-persisted; memory discarded on sleep, `createVars` re-runs on wake | Full memory snapshot; exact process resume |
| Coordination | Central Rivet Engine + pluggable state store | Control Plane + scheduler + reconciler (similar) |
| Runners | Pull-based: backends connect to engine and execute actor code | Push-based scheduler → Node Agents |
| Workloads | Code written against RivetKit SDK | Any Linux binary, no SDK |
| Sleep policy | Idle timer + `can_sleep` predicate + grace window; `onSleep`/`onWake`/`onMigrate` hooks | Declarative policy (e.g. "may sleep 20 min") |

**Takeaways:**
- Third data point (after Cloudflare DO and celld) that the industry default is
  *programming-model durability* — state forced through a storage API, ephemeral
  memory discarded on hibernate. This project's memory-snapshot bet is the
  differentiator; its true siblings are sandbox platforms (Modal memory snapshots,
  CodeSandbox/E2B-style microVM cloning), not actor frameworks.
- Rivet validates the control-plane architecture of Phases 2–3 and offers a
  pragmatic storage-backend progression (FS → Postgres → FDB) relevant to Open
  Question 4.
- Rivet's actor lifecycle state machine (`Ready → Started → SleepGrace →
  SleepFinalize → Terminated`) is strong prior art for the R-SNAP-3 policy
  vocabulary and §3.2 tiering.
- Competitive positioning: if the target workload is AI agents, the wedge vs.
  Rivet is **no SDK lock-in + exact memory resume for arbitrary runtimes**.

**`[VERIFIED v0.7 — read from source]`** Apache-2.0, Rust, actively developed.
Three corrections and additions to the entry above:

- **The engine is not reusable as libraries.** Every candidate crate
  (`gasoline` — their durable workflow engine, `guard-core` — the wake-on-request
  proxy, `universaldb` — B8's storage progression made concrete) is
  `publish = false` and written against `rivet-config` / `rivet-runtime` /
  `rivet-pools`; `gasoline` additionally requires ClickHouse. Reuse means
  vendoring a platform, not adding a dependency. They remain valuable as **Rust
  reference implementations** of things Barista has yet to build — `guard-core`
  especially, being a wake-on-request proxy that does not drag in Envoy, which
  matters for §1's tier A hosts.
- **RivetKit is a real published library** (`rivetkit` on npm, Apache-2.0, with
  Rust/Python/Swift/React/Next.js packages) but sits *above* Barista: it is the actor
  programming model a consumer writes against, not infrastructure Barista would
  depend on. `[OPEN]` The interesting seam is `engine-runner-protocol` — a
  documented, multi-backend contract between the SDK and whatever executes it.
  Barista presenting itself as a RivetKit runner would inherit an application layer
  (SDKs, MCP hub, agentOS) for free. Not a Phase 1 decision, and only worth
  taking if a consumer asks for a *programming model* rather than a runtime;
  §11.4's app-layer consumer is the natural owner of that question, not Barista.
- **The camps are complementary, not merely opposed.** Rivet is §9.8's camp 1 —
  state serialized, memory discarded. On Barista, memory survives, so RivetKit's
  persistence layer would become an optimization rather than a correctness
  requirement. "Your actors, but they keep their RAM" is a sharper story than
  either project tells alone.
- **Their internal design docs are the real find.**
  `docs-internal/engine/sleep-sequence.md` and `HIBERNATING_WS.md` specify the
  idle→sleep edge — the half of the loop Barista owns and §9.11 lacks — far more
  rigorously than the public docs B5 was drawn from. B51–B54 come from there.

### 9.3 Borrowable patterns (consolidated)

> Sources: celld/Rivet from primary docs. Modal, E2B, CodeSandbox originally from
> memory, since **verified against primary sources** — see §9.5–9.7 for detail and
> corrections. B14 verified v0.2 (Modal mem-snapshots blog: OverlayFS upper +
> FUSE lazy-loading lower). Fly/Cloudflare entries still unverified.

| # | Pattern | From | Apply to |
|---|---|---|---|
| B1 | CAS ownership leases + epoch fencing in object store (exactly-one-owner, no consensus) | celld | Snapshot/instance ownership; double-scheduling risk §7.5 |
| B2 | Object store as source of truth; nodes rebuildable/cattle | celld | Platform-state design (§2.2) |
| B3 | Watermark-based pressure shedding of LRU idle workloads (never active ones) | celld | Node Agent memory pressure → snapshot-and-release |
| B4 | `diagnose` fleet CLI + deterministic fault-injection simulation of the protocol | celld | Phase 3 reconciler testing & ops |
| B5 | Sleep state machine (`SleepGrace`/`SleepFinalize`) + `can_sleep` predicate + idle timer + grace window + `onSleep`/`onWake`/`onMigrate` hooks. **`[AMENDED v0.7]`** Captured from Rivet's public docs and thinner than the implementation — see B51–B54, read from `docs-internal/engine/sleep-sequence.md` | Rivet | R-SNAP-3 policy vocabulary; in-guest quiesce hooks mitigate §3.3 consistency issues |
| B6 | Pull-based runners: agents dial the control plane | Rivet | Phase 2 topology (no inbound to nodes) |
| B7 | Wake-on-request: gateway holds connection while session restores | Rivet, Fly, Cloudflare DO | Phase 5 gateway × Phase 7 sessions integration |
| B8 | Control-plane store progression FS/SQLite → Postgres → (FDB) behind one interface | Rivet | Open Question 4 |
| B9 | Lazy restore: mmap memory snapshot, fault pages on demand | CodeSandbox | Restore-latency SLO (Risk 2, NFR-1). `[MEASURED v0.8]` Did **not** reproduce as a differentiator on the rank-1 substrate — see §6 NFR-1 sweep |
| B10 | Golden template snapshots + copy-on-write cloning; hot snapshots kept in page cache | CodeSandbox | Provisioning fast-path; local snapshot tier |
| B11 | Dockerfile/OCI → ext4 rootfs build pipeline (same artifact feeds fake + Firecracker runtimes) | E2B | Open Question 5; Risk 4 |
| B12 | In-guest agent daemon over vsock (exec, files, ports) | E2B | Phase 1 runtime contract |
| B13 | CPU-feature masking / schedule restores by CPU class | Modal | FR-8 constraint + scheduler input |
| B14 | Content-addressed lazily-loaded image filesystem (**verified v0.2**: OverlayFS, FUSE lazy lower — also evidence for spec §10.5 writable-layer choice) | Modal | Later-phase storage layout |
| B15 | Durable per-host operation FSMs journaled to local disk (survive agent crashes) | Fly (flyd) | Node Agent design, Phase 1 |
| B16 | Minimal placement first ("first node with fit"); avoid over-building the scheduler | Fly | Phase 3 scope control |
| B17 | Runtime verb set: `create/start/stop/suspend/resume/destroy` with documented suspend constraints | Fly Machines | Runtime interface spec (FR-11) |
| B18 | Named, globally-unique, single-writer session addressing; hibernatable WebSockets at the edge | Cloudflare DO | Session identity model; Phase 5 gateway |
| B19 | virtio-balloon before snapshot; diff snapshots for incremental object-store tiering | Firecracker | Snapshot size/cost (R-SNAP-2, NFR-2) |

`[NEW v0.6]` From Agent Substrate (§9.11) — read from its source, not its docs:

| # | Pattern | From | Apply to |
|---|---|---|---|
| B44 | Request parking: a bounded parking lot with explicit shedding, a per-**flight** (not per-request) budget, `singleflight` dedup so N concurrent wakes cost one control-plane RPC, and reserved fast-path headroom so a saturated lot cannot starve already-running sessions | Agent Substrate | Phase 5 gateway — the worked form of B7 |
| B45 | Pause and Suspend as distinct verbs with locality semantics: a node-local pause pins the next resume back to that node, as a **hard** placement filter rather than a preference | Agent Substrate | Phase 3 scheduler input; sharpens B16 |
| B46 | Snapshot **scope** as declared per-trigger policy (`onPause`/`onCommit`, with `onCommit ⊆ onPause`) over an explicit durable-data surface, plus golden-as-resume-source: shared golden memory combined with this session's data, so an idle session costs one small blob instead of a memory image | Agent Substrate | R-SNAP-3 policy tiers (§3.2); the input side of `SnapshotKind`; extends B10 |
| B47 | Substrate-upgrade snapshot invalidation as a lifecycle event (not only a restore-time error), and template immutability — a new version, never an edit — so goldens stay coherent with the spec that produced them | Agent Substrate | B29/B35 keying; fleet upgrade story |
| B48 | Per-session identity delivered as a **file, not an environment variable**: anything in the environment is frozen in the golden's checkpointed memory and comes back identical for every forked session. Use the readiness probe to decide when a golden is warm enough to capture, instead of a fixed settle delay | Agent Substrate | v1alpha2 fork/golden (B10/B39); `Process.ready_cmd` (B36) |
| B49 | Classify errors as retryable vs terminal at the contract, so the CLI, gateway and consumer SDKs do not each re-derive the list from status codes | Agent Substrate | `ErrorReason` (Contract A) |
| B50 | Session-scoped observability from the start — every log, metric and trace tagged with session and host, suspended sessions inspectable — and three published north-star numbers rather than none | Agent Substrate | NFR-4, NFR-5 |
| B55 | **Digest-pinned images enforced at validation**, not merely preferred: an image reference carrying no digest is rejected outright, because a tag can be repointed at different bytes while the template identity stays stable | Agent Substrate | `OciImageRef` — `snapshot_key.rs:29` already names the hazard ("a tag can be repointed at different bytes tomorrow") and then hashes the tag anyway when the digest is empty, which makes B29 invalidation fail silently. B47's rule seen from the other side. **`[LANDED v0.9 — nap-011]`** validation rejects the unpinned digest at `CreateInstance`, the hash no longer forgives it, and the tag is a label the identity ignores |

`[NEW v0.7]` From Rivet's internal design docs (§9.2), which are materially
richer than the public docs B5–B8 were drawn from:

| # | Pattern | From | Apply to |
|---|---|---|---|
| B51 | **Two** sleep predicates, not one: `can_arm_sleep_timer()` (may we start counting down?) and `can_finalize_sleep()` (may we actually stop?), gating different transitions over different counter sets | Rivet | TTL semantics (B33, T6); amends B5 |
| B52 | Keep-awake as **scoped leases that cannot leak**, never a flag — a promise-scoped counter decrements on settle, whereas their deprecated `setPreventSleep` boolean "had to be paired by hand; forgetting to clear it wedged the actor awake". Two variants: blocks-idle-and-finalize vs blocks-finalize-only (best-effort flush). User and framework holds counted separately, for diagnostics only | Rivet | Guest Agent contract — the vsock equivalent of `keepAwake`; Barista's TTL is host-side and activity-*inferred*, this is workload-*declared* |
| B53 | Grace window with a **soft** abort signal ("shutdown has started, please wrap up" — never force-stops) behind a **hard** deadline; on timeout, log every non-drained counter *by name*. Sanity caps (a bound nobody should hit) distinguished from deadlines (normal control flow) | Rivet | §7 quiesce policy — their `can_finalize_sleep()` is a veto made safe by the deadline, where nap-005 chose "the hook is a chance, not a veto"; a decision to take deliberately |
| B54 | Hibernating connections: the **gateway owns the client socket across a pause** — the runtime closes its side with `hibernate = true`, the client's connection stays open and idle, the next client message wakes the session. Opt-in by capability flag; a keepalive marks the held request live; and the wake path **tells the workload which connections are still held** | Rivet | Phase 5 gateway (extends B7/B18); restore duties (nap-005 §4.2) currently omit the reattach list — the difference between an invisible pause and a dropped user |

### 9.4 LiveKit Agents server (docs.livekit.io/agents/server)

Domain-specific job orchestrator for realtime (voice/media) AI agents. **No
durability**: no state persistence or snapshots — on crash, LiveKit detects the
dead agent (~15s) and dispatches a *fresh* one into the same room; the app must
rebuild context. Architecture: workers (agent servers) dial in and register,
stand by, accept dispatched jobs, and run **one process per job** for isolation.

**Takeaways (borrowables continue numbering):**

| # | Pattern | Apply to |
|---|---|---|
| B20 | Scalar self-reported load (0–1 `load_fnc`) + `load_threshold` instead of a full resource model | Phase 2–3 simplification; pairs with B16 |
| B21 | Offer/accept dispatch with worker veto (`requestFunc`) | Placement middle ground between push scheduler and celld leases; lets nodes veto on CPU class (B13) or snapshot locality (B10) |
| B22 | Prewarm hook (`setup_fnc`) as a first-class option | DX surface for golden-template snapshots (B10) |
| B23 | Graceful drain with `drain_timeout` as the deploy primitive | Node Agent upgrades — Barista upgrade: drain = snapshot-and-migrate, not wait |
| B24 | ~15s liveness detection + automatic replacement bound to the same session identity | Concretizes Phase 3 reconciler DoD (Risk 5); Barista restores memory instead of restarting with amnesia |
| B25 | Stable rendezvous (room) outlives compute; clients stay connected across agent replacement | Third confirmation of B7/B18: session identity lives in the gateway, not the instance |

**Positioning:** more potential consumer than competitor — stateless realtime
agents that lose context on crash are the gap memory-resume fills. Boundary
condition for Open Question 1: live media cannot wait for GB-scale restore;
memory-resume targets paused/idle sessions, not active realtime streams.

### 9.5 Modal (docs.modal.com + engineering blog — Memory Snapshots) — verified

CPU Memory Snapshots (GA) + GPU Memory Snapshots (driver-level
checkpoint/restore). 3–10× faster cold starts for init-heavy functions. The most
mature *policy layer* around memory snapshots.

> **`[CORRECTION v0.2]` Modal runs on gVisor (`runsc`), not Firecracker.**
> Stated in their docs ("Modal runs containers using the sandboxed gVisor
> container runtime") and explained in *Memory snapshots: Checkpoint/restore for
> sub-second startup*: CRIU targets `runc` and cooperates with the **host**
> kernel via `/proc`; Modal chose `runsc` **for security**, and because gVisor
> implements the kernel in userspace, checkpoint/restore is built *into* the
> guest kernel itself (`kernel.go` plus ~18 subsystems with `save_restore.go`).
> Owning the kernel is what makes C/R tractable — the inverse of the CRIU
> problem described in §3.3. This invalidates "Firecracker is the unanimous
> choice of the snapshot camp" (§10) and is the central evidence for ADR-001
> (§13).
>
> Measured: `import torch` 5s → **1.05s** p50 (0.69s p0); Stable Diffusion
> inference 13s → **3.5s**. GPU (CUDA checkpoint API, driver 570+, via
> `cuCheckpointProcessLock/Checkpoint/Restore/Unlock`): Parakeet 20s → **2s**,
> ViT 8.5s → 2.25s, vLLM/Qwen2.5 45s → **5s**. Bottleneck is the pages file
> (100 MiB–10 GiB). Snapshots are host-specific (an AWS `g6.12xlarge` lacking
> `pclmulqdq` cannot restore a snapshot taken elsewhere) and sensitive to NVIDIA
> driver *and container runtime* version.

| # | Pattern | Apply to |
|---|---|---|
| B26 | Two-phase init hooks: `@enter(snap=True)` pre-snapshot / `@enter(snap=False)` post-restore (e.g. weights→CPU RAM pre-snap, →GPU post-restore) | Completes B5: policy spec needs both pre-snapshot *and* post-restore hooks |
| B27 | Snapshots keyed by worker class (CPU flags / GPU type); ~6 variants for CPU coverage, 2–3 per GPU type, auto-recaptured | Hard confirmation of B13; scheduler must match restore→CPU class or budget N captures |
| B28 | "Don't snapshot rebuildable state": discard caches (e.g. KV cache) pre-snapshot, rebuild post-restore | Snapshot-size policy; platform could automate |
| B29 | Snapshot invalidation tied to deploy versioning; platform auto-recaptures on code/config/runtime changes | Missing from BRD — add to FR-8 |
| B30 | Snapshots skip CPU-bound init (imports, JIT) but not storage-bound loading (weights) — can even hurt | Sharpens Risk 2; needs fast image/volume storage alongside |
| B42 | **Restore failure falls back automatically to normal cold start** — snapshots are an optimization, never a correctness dependency | Node Agent `Resume` error path (spec §8); reconciler must treat `SNAPSHOT_INVALIDATED` as degraded-but-served, not failed |
| B43 | GPU C/R sequence: `cuCheckpointProcessLock` → `Checkpoint` (vRAM+CUDA objects→host RAM) → full memory snapshot → `Restore` → `Unlock`; driver 570+ | the voice-agent runtime GPU tier (§11.3); requires matching GPU hardware + driver across hosts |
| — | RNG duplication documented as *user's* problem: snapshot state is "reused over and over again — expectations of entropy may be violated" (they punt, verbatim) | Differentiation opportunity confirmed: in-guest agent re-seeds entropy post-restore (§3.3, spec T9) |

### 9.6 E2B (docs + open-source `e2b-dev/infra`, Go, Terraform/Nomad) — verified

Components: `api`, `orchestrator` (per-node, Firecracker), `client-proxy`, `envd`
(in-guest daemon — confirms B12), `template-manager`, `db`. The cleanest *API
surface* reference.

| # | Pattern | Apply to |
|---|---|---|
| B31 | Public state machine: `Running / Paused / Snapshotting / Killed`; `Snapshotting` is transient and auto-returns to `Running` (live checkpoint without stopping) | §3.2 tiering states; API design |
| B32 | Tiering as one API option: `pause()` = memory+FS; `keepMemory:false` = filesystem-only snapshot, cold-boot resume | R-SNAP-2 exposed as user-facing choice |
| B33 | TTL lifecycle: default 5-min timeout, reset on `connect()` — GC by lease expiry, not explicit kill | Phase 1 default lifecycle |
| B34 | Auto-resume on request (wake on activity) | Fourth confirmation of B7 |
| B35 | Builds = UUIDs with `-from-build` incremental layers; kernel + Firecracker version pinned per build | Reproducible snapshots; Open Question 5 |
| B36 | Node mechanics: NBD for rootfs delivery; 2MB hugepages by default; `start-cmd` + `ready-cmd` readiness contract | Phase 1 runtime contract & node setup |

### 9.7 CodeSandbox (blog: "How we clone a running VM in 2 seconds") — verified

The *performance playbook*, with numbers. Firecracker `create_snapshot` →
`snapshot.snap` (machine config) + `memory.snap` (full memory size).

| # | Pattern | Apply to |
|---|---|---|
| B37 | Lazy mmap restore: resume ~200–300ms; most VMs fault in <1GB even when guest "uses" 3–4GB | Confirms B9 with data; NFR-1 budget. `[MEASURED v0.8]` The band is real but the *eager* backend already sits in it (§6) — the number does not certify laziness |
| B38 | **`MAP_SHARED` continuous dirty-page flush**: kernel lazily syncs memory changes to the snapshot file → save drops from 1s/GB (8–12s) to **30–100ms** | Makes the local tier an always-warm, continuously-synced file; pause nearly free (R-SNAP-2) |
| B39 | CoW cloning of memory+disk (~50ms copy): pause 16ms + save 100ms + copy 800ms + boot 400ms = fork <2s, workload-independent | B10 verified; fork-as-provisioning |
| B40 | Measured business case: fresh start 132.2s → deps preinstalled 48.4s → build cache 22.2s → memory snapshot **0.6s** | Risk 2 evidence; snapshots win only when init is expensive |
| B41 | Product patterns: environment-per-branch via cloning; wake-on-webhook services hibernating after ~5 min (~300ms wake penalty) | Target use-case candidates (Open Question 1) |

### 9.8 Synthesis

- Two camps: **programming-model durability** (Cloudflare DO, celld, Rivet —
  serialize state, discard memory) vs **memory-snapshot durability** (Modal, E2B,
  CodeSandbox — all three built on checkpoint/restore, all three commercially
  alive). This project sits in camp 2; camp 2's existence proves feasibility, and
  its members are sandbox/function platforms — none offers a general
  *orchestrator* with declarative session policies, which is the remaining gap.
- Convergent patterns confirmed ≥3 times independently: wake-on-request (B7:
  Rivet, Fly, CF, LiveKit, E2B), agents dial control plane (Rivet, LiveKit, E2B),
  in-guest daemon (E2B envd, LiveKit process model, Modal hooks), pre/post
  snapshot hooks (Modal, Rivet), session identity above compute (B25).
- The technical recipe for competitive snapshots is public: mmap lazy restore
  (B37) + `MAP_SHARED` continuous flush (B38) + CoW clones (B39) + hugepages &
  NBD (B36) + worker-class keying (B27) + balloon/diff snapshots (B19).
- **`[CORRECTION v0.2]` Camp 2 is not runtime-homogeneous.** Modal runs gVisor
  (§9.5); E2B and CodeSandbox run Firecracker. The recipe therefore splits: the
  *policy layer* (B26–B30, B42, worker-class keying B27) is Modal's and is
  gVisor-native; the *mechanical* fast path (B38 `MAP_SHARED` flush, B39 CoW
  fork, B36 NBD/hugepages) is Firecracker-specific and does **not** transfer to
  gVisor. Lazy restore (B9/B37) exists on both — gVisor's equivalent is
  `runsc restore --background`, which starts the application once kernel state is
  loaded and faults in remaining pages on demand, prioritizing any page an
  application thread blocks on. See ADR-001 (§13).

### 9.9 GKE Agent Sandbox + Pod Snapshots (2025) — the gVisor-path competitor

Google's productisation of exactly this idea on gVisor: isolated per-agent
sandboxes with **Pod Snapshots** — full checkpoint/restore of running pods —
claiming sub-second restore and ~90% improvement over cold start, GKE-exclusive.

**Why it matters to ADR-001:** choosing `runsc` relocates the competitive frame.
Against Firecracker the neighbours are Modal/E2B/CodeSandbox (function and
sandbox platforms, no general orchestrator — the §9.8 gap). Against gVisor *on
K8s* the neighbour is Google shipping the same primitive natively. Two
consequences: (a) the pattern is validated by the largest possible reference
implementation; (b) the differentiator narrows to **declarative session policy +
portability**, since Pod Snapshots is GKE-only — which is precisely why it does
not help the voice-agent product on Azure (§11.3). ~~`[OPEN]` If Azure ships an equivalent, the
K8s-mode wedge is gone; standalone/bare-metal mode and the policy layer are what
survive.~~

**`[RESOLVED v0.6 — §9.11]`** The open question closed, and not the way it was
posed. Nobody needed to ship an Azure equivalent: Google open-sourced the same
primitive as **Agent Substrate**, Apache-2.0, on any Kubernetes. "Portable
because they are GKE-only" is dead as a wedge. What replaces it is narrower,
truer and load-bearing: Substrate — and every §9 platform of its shape — needs a
**cluster with privileged node-level components** it can install into, which
tiers A and C of §1 do not have. The surviving differentiator is *runs where
there is no cluster*, plus the honest capability model that lets one API span
all three tiers. See §9.11.

### 9.10 hypeman (`kernel/hypeman`, MIT, 2026-08-04) — `[NEW v0.5]` a substrate, not a rival

KERNEL open-sourced the single-host VM manager behind their agent-browser fleet
(>1M browsers/month; ~128 chromium VMs per host at 8× CPU oversubscription).
Unlike everything else in §9, it is **not a platform competing for the same
users** — it occupies exactly the Node Agent's scope and stops there:
"cross-host scheduling, failover, and regional placement are handled outside".
That makes it a candidate implementation of **Contract B**, and the first
serious challenge to ADR-001's *premise* rather than its conclusion.

**What it covers.** One uniform lifecycle API — create, boot, pause, snapshot,
restore, **fork**, shutdown — over four backends (Firecracker, Cloud Hypervisor,
QEMU on Linux; **Virtualization.framework on Apple Silicon**). Any OCI image runs
as a VM via a generic initrd that mounts the image rootfs read-only with a
per-guest writable overlay. VMs from one image share a single read-only disk;
forks hardlink the source's memory snapshot, so a fork costs no memory I/O and
siblings share page cache. Resource limits (cpu/mem/disk/IO/network/vGPU) with
oversubscription ratios. Docker-compatible CLI plus a JWT-authenticated remote
API.

**Why it matters to ADR-001.** ADR-001 ranked `runsc` first for two concrete
reasons, and hypeman removes both: no KVM requirement on Apple Silicon (so the
macOS dev loop survives), and **no rootfs CONVERT pipeline** — which retires
§12's pipeline and OQ5's writable-layer question as *our* problems. It also
supplies two things `runsc` cannot: hardware isolation (today T12's
`require_hardware_isolation` can only ever fail), and `fork`, which we deferred
to v1alpha2 (B39) and which subsumes OQ4's N-times-restore spike. Roughly
`nap-005`'s first two task groups are already built and operated at scale.

**What it does not cover — i.e. what stays Barista's.** No readiness probes, no
pre-snapshot/post-restore hooks, no restore-time duties (entropy reseed, clock
step), and no journal or crash-recovery model. That is **Contract C plus the
journaled operations model — nap-002 and nap-003, already delivered, and
runtime-agnostic by construction**. Session semantics (named, single-writer,
TTL-on-activity) and the Phase 2 control plane are untouched, the latter being a
shared gap rather than their advantage.

**Risks in adopting it.** Two days *public* at the time of writing — but at 344
merged PRs, v0.1.0 since June 2026, and running KERNEL's production fleet, so the
dependency risk is ordinary rather than reckless (MIT, therefore forkable). Its optimisation target is the inverse of
ours: a median VM lifetime of 4 minutes makes lifecycle a hot path, whereas Barista's
sessions are long-lived and long-paused — so their churn/fork work transfers less
than their standby-density work. And it is hypervisor-only: adopting it
exclusively removes any cheap shared-kernel tier, which bears on the voice-agent runtime's
per-call runtimes (§11.3).

`[RESOLVED v0.5 — measured]` **The `vz` backend does snapshot, on arm64 only**
(`SupportsSnapshot: runtime.GOARCH == "arm64"`), and a 60 s pause/resume on this
Mac restored guest memory in **0.67 s** with `/proc/uptime` continuing — Barista's T3
assertion, on a laptop. Guest-agent injection (`--entrypoint`, `--env`) and
node-scoped tags (`-l`) also check out, so nap-003's bridge transport transfers.

`[MEASURED v0.5]` **Restore cost tracks snapshot bytes — i.e. memory entropy —
not guest memory size.** A realistic 1.5 GiB session (text plus random data)
snapshots to 608 MB and restores in **1.97 s**; the same memory filled from
`/dev/urandom` snapshots to 2.0 GB and restored in 29.21 s. The scary number was
adversarial test data, not a property of the substrate. ~2 s plausibly satisfies
**R-SNAP-4** for an agent runtime. **Idle cost of a paused session:** zero CPU
and zero host RAM (the VMM process is gone), and roughly **0.4 bytes of disk per
byte of live memory** plus ~150 MB of sparse overlay — which validates
R-SNAP-2's object-store tier as the thing that scales with paused-session count.

~~`[OPEN]` How much *better* than 2 s the firecracker/UFFD path is, and where~~
`[MEASURED v0.8 — §6]` — answered: ~370 ms to first response, flat from 1 to
2 GiB dirty, and UFFD buys only 5–15% over the eager backend at this scale. Where
snapshot keying (`cpu_class`, `template_hash`, `runtime_bundle_ref`) lives —
hypeman appears not to enforce restore compatibility, which would make it Barista's
job. Evidence and the conditional recommendation:
[`adr-001-substrate-evaluation.md`](adr-001-substrate-evaluation.md).

### 9.11 Agent Substrate (`agent-substrate/substrate`, Apache-2.0) — `[NEW v0.6]` the closest neighbour, and the other half of the loop

Google's open-source successor to §9.9's GKE-exclusive Pod Snapshots: a
Kubernetes control plane for "a performant, high density runtime environment for
large scale agent deployments". Go, early ("not ready for production use, and
the APIs are almost guaranteed to change"), integrated with `google/ax`. Read
from source, not from its docs.

**It converges on Barista's premise almost line for line.** Agent-like workloads are
idle most of the time; the unit is a named, single-writer **Actor**
(`(atespace, name)`) that a Worker hosts one at a time; snapshots capture memory
plus filesystem; sandboxes are pluggable (gVisor rank 1, Kata/cloud-hypervisor
microVMs rank 2 — the inverse of ADR-001 v2's ranking); the node layer is an
agent plus an in-pod coordinator (`atelet` + `ateom` ≈ Node Agent + Guest
Agent); an immutable template produces a **Golden Snapshot** that new actors
restore from. Independent convergence on TTL-shaped policy, pre/post hooks,
disk-only as a declared tier, capability honesty and restore-compat keying
should be read as validation of those choices, not as borrowings owed.

**What it has that Barista does not.** A working cluster control plane: a scheduler
(random free worker, filtered by sandbox class and snapshot locality — B16 taken
further than Barista plans to), an Envoy/`ext_proc` router with wake-on-request and
request parking (B44), mTLS pod identity, an object-store snapshot tier, and
30×-oversubscription multiplexing of many actors onto few warm pods. Its north
stars are density: 1B actors per cluster, 1000 wakeups/s, 100 ms p95 activation.

**What Barista has that it does not.** The idle→suspend edge does not exist. The
`Control` service, the `AteomHerder` RPCs and the `Actor_Status` enum carry no
activity signal, no TTL and no reaper; `SuspendActor`/`PauseActor` are explicit
client calls, and their roadmap files idle GC under "ideas we are thinking
about". Barista owns that edge (`ttl_seconds` + `ttl_action`, reset on guest
activity — B33, T6) and lacks the wake edge. **The two projects have built
opposite halves of the same cycle** — a more accurate framing than "ahead" or
"behind", and the reason B44–B50 are worth taking without embarrassment.

**Why it is not a substitute.** Substrate cannot be deployed on §1's tier A or
tier C at all: `atelet` is a DaemonSet, workers execute `runsc` or a VMM inside
the pod, and the microvm class mounts KVM devices — none of which exists on
Fargate or Azure Container Apps, and none of which applies where there is no
cluster. This is the restated §9.9 wedge: not "portable vs GKE-only", but
**"requires a Kubernetes cluster with privileged node-level components" vs "a
standalone service that runs where there is no cluster"**. That distinction
follows from Substrate's architecture rather than from a vendor's distribution
choice, which makes it durable in a way §9.9's was not.

**Consequence for Barista's own plan.** Phases 2–6 — control plane, scheduler,
manifests, gateway, identity — now have a maintained reference implementation
with working code for all five. ADR-001 v2 refused to reimplement ~35,600 lines
of substrate for zero differentiation; the same test one layer up says build
what tiers A and C force Barista to own, and borrow the rest (B44–B50). `[OPEN]`
Whether a Barista session layer could run *on* Substrate where a cluster does exist
— attractive for the AKS/EKS/OpenShift tier, but it would be a replacement
rather than an adoption, because Substrate occupies Barista's own layer instead of
sitting beneath it as hypeman does. Not a decision for Phase 1; the evidence
that should inform it is T7's.

### 9.12 Cloudflare Durable Objects + Containers — `[NEW v0.10]` the closest product neighbour

**Verified against their docs** (developers.cloudflare.com `llms-full.txt`,
DO updated Jul 2026, Containers Jun 2026). DO is the upstream of half this
section's patterns — B18 came from it directly, celld (§9.1) reimplements its
model without Cloudflare, Rivet (§9.2) commercialises it — so it deserves its
own entry, and reading it whole yields more than reading its descendants.

**The DO vision, decomposed**: *"each Durable Object is a single-threaded,
globally-unique instance with its own persistent storage"* — addressed by name
(`getByName`), created by being addressed, evicted when idle, woken by the next
request **or by an alarm**, with WebSocket connections that survive hibernation.
Seven separable ideas; Barista already holds five (identity as primitive = B18;
wake-on-request = B7/B44; serialized single-writer ownership; hibernating
connections = B54; per-entity storage). The two it lacked are B56 and B57 below.

**Where Barista diverges by design, and the docs sharpen the wedge**: DO's
foundational clause is *memory is a cache, storage is truth* — in-memory state
is explicitly lost on eviction, shutdown hooks are deliberately not provided
(*"external software may rely too heavily on these unreliable hooks"*), and the
whole programming model exists to force state out of memory. **Containers
inherit the clause twice over**: *"All disk is ephemeral. When a Container
instance goes to sleep, the next time it is started, it will have a fresh
disk."* Sleep loses memory *and* disk; only the DO's SQLite survives. Hence the
positioning line, which is exact rather than rhetorical:

> **Barista = DO + Containers, self-hosted, where sleep loses neither memory nor
> disk — and no SDK: the per-session control entity is the platform's, not a
> class the user writes.**

**The architectural validation**: Cloudflare's own newest product structure has
each container *"managed by a Durable Object"* — a small single-writer control
entity per workload (routing, lifecycle, sleep timeouts, port readiness)
fronting a heavy isolated process. That is Barista's shape (journal row + ops +
lease ↔ hypeman VM), reached from the opposite direction. Independent
convergence, like celld's on B45/B54 (§9.1).

**Not to take**: the SDK contract (their control layer is user-written JS — the
symmetric opposite bet to §1's priorities); opaque placement (*created near
first request*, no control — conflicts with B45, where snapshot locality is
physics); the proprietary directory that enforces global uniqueness — which is
precisely the part celld replaced with bucket CAS, i.e. OQ9. **DO answers what
the abstraction should be; celld answers how to coordinate it without being
Cloudflare. The two compose.**

**Premises this review changed (ratified 2026-08-08)**: (1) *waking is the
platform's job* — request → alarm → explicit verb, in that frequency order;
`Resume` becomes internal machinery and the §4 Phase 5 row is reframed
accordingly. (2) *The session name is the public handle*, ids are internal —
see OQ9's amendment: the lease table and the addressing table become the same
table. Also one schedule change: the remote snapshot tier leaves the Phase 2
critical path (§3.1) — DO ran for years without migration and it was enough.
**Premises reaffirmed by the contrast**: no-SDK declarative; one workload per
session (1 DO ↔ 1 container — OQ11's answer, converged on independently);
hooks as best-effort chance, never veto (they provide none at all, for the
stated reason); `PAUSED` holds zero resources (their hibernation costs zero).

**Watch item**: DO + Containers is now the nearest product neighbour — nearer
than celld (JS-only) or Substrate (needs K8s + Redis). Their containers run in
microVMs, so memory snapshots are *technically* reachable for them; if
Cloudflare ships that, Barista's wedge narrows to self-hosted / portability /
no-SDK. Not imminent, but the one competitor move that should trigger a
positioning review.

| # | Pattern | From | Apply to |
|---|---|---|---|
| B56 | **Scheduled wake**: `setAlarm(timestamp)` / `schedule(when, callback, payload)` — the platform wakes an idle entity at a time with no inbound request; alarm handlers are idempotent by contract (*may fire more than once*), retried with a visible `retryCount`, and multiple named schedules multiplex one alarm | Cloudflare DO | The missing wake edge — TTL is the sleep edge, this is its mirror. A `WakeAt` on the session = a scheduled `Resume` driven by the reconciler that already manages TTL deadlines; payloads journaled like every other op. v1alpha2 verb; enables cron-agents ("check back at 9am") without an external poker |
| B57 | **Programmable egress per entity**: `outboundByHost` / `outbound` — intercept, block, mock or proxy the workload's outbound HTTP at the platform edge | Cloudflare Containers | The concrete form of the egress control OQ1 promoted to mandatory (untrusted agent code) in v0.1 and that has had no shape since. Declarative allowlist in `InstanceSpec`, enforcement delegated to the substrate — whose knob already exists (`network_egress_enabled` / `egress mode`), unexposed by Barista |

### 9.13 The actor-runtime thesis — `[NEW v0.12]` the mother document, and Barista's coordinates in it

An external working document (*Arquitectura de runtimes distribuïts per a agents
i actors*, working version 2026-08-08, maintained outside this repo) states the
general hypothesis this project is the narrow bet on: logical identity →
materialisation → runtime → backend, with primitives as the cross-cutting
contract and a **durable substrate** as the transversal layer of truth. It was
reviewed twice against this BRD; the second version reads the same primary
sources §9.1/§9.11/§9.12 verified and converges on the same conclusions the
project reached by measurement. Alignment findings, both directions:

**Barista is the thesis's vocabulary, arrived at wordlessly.** The project adopted
the actor model's properties piecemeal — B18 (identity as primitive,
single-writer), B5/B52 (declared lifecycle), B7/B44/B56 (activation edges),
ADR-002 (leases + epochs as the identity directory) — without ever using the
word. Term map, for the day Phase 2 needs to choose names: *actor lògic* =
named session; *materialització* = instance; *substrate durable* = journal +
snapshot keys + coordination bucket; *bindings* = the one concept the thesis
has that Barista lacks (declared, authorized dependency injection — Phase 4
manifest material).

**"Durable substrate" is adopted as this BRD's collective noun** for what has
so far had no single name: the op journal, the snapshot records with their
restore-compatibility keys (B27/B29/B35), and the ADR-002 lease objects — the
layer that answers "what can this identity be rebuilt from, and who owns it",
as distinct from the backend, which answers "where does it run now".

**The thesis's own design axes place Barista in the empty cell.** Its §10.7
separates persistence × ownership as independent axes; plotting the surveyed
systems:

| system | persistence | ownership |
|---|---|---|
| Agent Substrate (§9.11) | process snapshot | control plane (K8s + store) |
| Durable Objects (§9.12) | application state (SQLite) | proprietary directory |
| celld (§9.1) | application state (SQLite) | CAS in a bucket |
| **Barista** | **process snapshot (memory + disk, measured)** | **CAS in a bucket (measured)** |

No surveyed system combines process-snapshot persistence with
no-control-plane ownership. That combination is Barista's position, and unlike the
thesis's other cells it is measured rather than aspirational (T7; ADR-002).
This is the sharpest one-table statement of the wedge the BRD has, and the
natural frame for the planned article series.

**Where the thesis still needs editing before external use** (recorded here so
the next revision knows): (1) its restore flow (§8.2.15) still hedges
"preferably do not assume process memory restores portably" — now internally
inconsistent with its own §5 (`substrate: process-snapshot` is a declarable
option) and §10.4 (the compatibility contract that makes it safe); the fix is
to scope the caution to the compatibility keys, which is what this project
does (B27/B29/B35). Memory is saved — that is the product. (2) Its example
catalogue includes the stateless scale-out class this project excludes by
constitution; the validated slice is the hibernable-session class, and webs
scale on Barista by decomposition into identities, not by replication of a name.
(3) Its `checkpoint` primitive lacks the freeze/no-freeze distinction this
project treats as an honesty boundary (`Pause` vs `Checkpoint`, constitution
v1.3.0, nap-015).

## 10. Verdict on the Initial Plan (post-research)

**Validated by the field:**
- Three-state model (§2.2) — E2B ships it as an API flag.
- Two-tier snapshots (R-SNAP-2) — universal among snapshot platforms.
- Policy-driven, intent-based lifecycle (R-SNAP-3) — industry direction; adopt the
  richer vocabulary (predicate + idle timer + grace window + pre/post hooks, B5/B26).
- "Restore beats cold start" — verified (0.6s vs 132s, B40) but **conditional**:
  only for init-heavy workloads (B30).
- Phase skeleton and north-star milestone — mirrors Rivet/E2B architecture.
- ~~Firecracker — unanimous choice of the snapshot camp.~~ **`[REFUTED v0.2]`**
  Modal — the most mature policy layer in camp 2 — runs **gVisor**, chosen
  deliberately over `runc`+CRIU for security (§9.5). The camp splits 1–2 by
  runtime, not 3–0. What *is* unanimous is checkpoint/restore of guest memory;
  the VMM is not the load-bearing choice it appeared to be. → ADR-001 (§13).

**Corrected:**
1. *"Docker→Firecracker migration will be fluid"* — *rejected.* No Docker
   equivalent exists for the real mechanics (mmap/`MAP_SHARED`, hugepages, NBD,
   vsock agent, CPU-class snapshot keying). Fake runtime stays, but the Phase 1
   contract must be defined at Firecracker's abstraction level with snapshot
   verbs and the in-guest agent included (Risks 1, 3). **`[REVISED v0.3 —
   ADR-001]`** Abstraction level is now the OCI bundle (spec §2.2); the
   conclusion stands — snapshot verbs + in-guest agent in the Phase 1 contract —
   but the artifact is an OCI image, not a rootfs.
2. *Snapshots as Phase 7 feature* — *rejected as design sequencing.* Snapshot
   requirements reach back into Phase 1 (memory/file layout), Phase 3 (CPU-class
   scheduling, B27), Phase 4 (deploy-versioned invalidation, B29), Phase 5
   (wake-on-request, B7). Implement late, design early.
3. *Snapshot performance as solved detail* — it is the core engineering problem;
   recipe now known and referenced (§9.8).
4. *Never discussed:* rootfs pipeline (B11/B35), in-guest agent (B12),
   observability (NFR-5), snapshot security/RNG (§3.3), gateway-owned session
   identity (B25).
5. *Never asked:* who it is for. The gap — general orchestrator + declarative
   session policies + memory-snapshot durability — is real and unoccupied
   (§9.8), but Open Question 1 decides whether it is a market gap or only a
   technical one.

## 11. Initial Consumers (decided, v0.1)

Three in-house consumers, mapping 1:1 onto the OQ1 personas. Recommended order:
**the agent platform → the preview-env platform → the voice-agent runtime**.

### 11.1 The agent platform — agent sessions — persona (a), first consumer

The agent platform's coding sessions currently run in-process in its worker
pool or on separate instances. As Barista Sessions: hibernate while waiting on
LLM/human input (exact memory resume), wake-on-request on events/webhooks (B7),
fork for exploration (B39). Expensive-to-rebuild in-memory REPL/agent context =
the strongest "restore beats cold start" case (B40). Internal, single-tenant,
relaxed SLOs → ideal Phase 1–3 validator.

### 11.2 The preview-env platform — developer preview environments — persona (b)

PR preview environments (`*-pr-<n>-*.run.preview.example`) = the verified
environment-per-branch pattern (B41): previews hibernate, wake on first request,
cost ~zero while idle. Positioning: Barista is an **alternative execution
backend** for previews, not a replacement for the GitHub Actions/ACR/Argo CD
pipeline.

### 11.3 The voice-agent runtime — dedicated agent runtime — persona (c), design constraint

The ART→voice-agent runtime proposal needs: per-call/per-session isolation, warm pools hiding
voice cold start (bootstrap + VAD/ONNX models + provider sockets), a WS gateway
mapping `stream_id`→container with drain-on-scale-in, a hardening seam
(gVisor/Kata + a per-worker workload identity), and an isolation tier for custom
Python nodes (per-session/tenant, scale-to-zero).

Barista mapping: golden voice-runtime snapshot + CoW clone per call (<1s, B10/B39)
instead of idle warm pools; microVM isolation; session gateway (B25); drain =
snapshot-and-migrate (B23); workload identity (Phase 6).

**Constraints from this consumer:**
- LiveKit boundary (§9.4) applies fully: live calls cannot pause/resume — value
  is fast isolated provisioning and scale-to-zero *between* calls, not mid-call
  hibernation.
- Provider sockets do not survive restore → post-restore reconnect hooks (B26)
  are mandatory.
- The voice-agent runtime Lane C stateless fleet (RPS autoscale) is the excluded PaaS quadrant —
  out of scope; only its isolation tier is in scope.
- ~~Enterprise Azure/K8s deployment may force a third runtime implementation
  (`kata`/`runsc`) behind FR-11.~~ **`[SUPERSEDED v0.2 — ADR-001]`** `runsc` is
  now the *first* runtime; the voice-agent product's Azure/K8s constraint is served by default
  (stock AKS node pools, no KVM), and the voice-agent runtime doc's own "gVisor/Kata hardening
  seam" is satisfied natively.

### 11.4 The app-layer consumer — potential fourth consumer, persona (a)

The app-layer consumer is the adjacent *application* layer: agent conversation loops, capability
resolution (MCP/native tools, resources, prompt templates), LLM gateway
(Bifrost), governance. No isolation or compute substrate of its own — like
LiveKit (§9.4), a consumer, not a competitor. Zero coupling today.

Fit evidence — the app-layer consumer's contract maps onto Barista's without changes:
- The app-layer consumer **MCP tool** = "containerized MCP server (`image_ref`, `endpoint`,
  `tenancy_mode`)" → `TemplateRef.oci` + isolation tier
  (`require_hardware_isolation`, T12) + scale-to-zero with wake-on-request
  (B7/B34) for idle MCP servers.
- The app-layer consumer **Agent Runtime** executes loops in-process — the same pre-voice-agent-runtime pattern as
  the voice-agent product ART (§11.3); per-session isolation or hibernating long-lived agent
  sessions would use Barista as the execution backend.
- Same org ecosystem (deep-ai-core) — integration would be organic.

Status: not committed; recorded so the Phase 2 Control Plane API keeps the app-layer consumer's
shape in view.

## 12. Packaging & Image Pipeline (OQ5 resolution)

**Input format: OCI images built from Dockerfiles — no new format.** All three
initial consumers (§11) already produce OCI images; the preview-env platform's GH Actions→ACR
pipeline feeds Barista directly. The fake (Docker) runtime runs the *same image*,
guaranteeing artifact parity (B11, closes Risk 3).

### 12.1 Pipeline stages

> **`[REVISED v0.9 — nap-011]`** The pipeline is **build → warm → distribute**.
> The former stage 2, CONVERT (pull + flatten → inject agent → `mkfs.ext4` →
> content-addressed `rootfs.ext4`), is gone and should not be restored from the
> ADR that originally justified it: it existed to feed a firecracker rootfs
> pipeline that ADR-001 v2 replaced with a substrate consuming OCI natively
> (initrd + per-guest overlay, its problem not ours), and its contract-side
> output — `RootfsRef` — was removed with OQ10 after shipping in `v1alpha1`
> without a single consumer: both runtimes refused it. The agent now reaches
> the guest as a content-addressed volume at create (nap-005 task 2.0), not as
> a CONVERT-time injection.

1. **BUILD** (existing, outside Barista): Dockerfile → buildkit → OCI image in
   registry (ACR/GHCR), **pinned by digest** — an unpinned reference is
   `INVALID_SPEC` at create (B55).
2. **WARM** (optional, key for sessions): boot template on builder → `start-cmd` →
   wait `ready-cmd` (B36) → capture **golden snapshot** (B10/B22).
3. **DISTRIBUTE**: object store → per-node cache keyed by digest. v1 full
   download; v2 NBD/lazy delivery (B36).

### 12.2 Design rules

- **Template identity** = hash(OCI digest + runtime bundle + resource config +
  arch). Golden snapshots keyed by (template, CPU class) (B27); any hash change
  invalidates and triggers recapture (B29, Modal pattern).
- **Runtime bundle versioned as a unit and pinned per build** — a snapshot is
  only restorable with the exact bundle that created it (reproducibility).
- **Per-instance writable layer**: ~~v1 `cp --reflink` (XFS/btrfs) CoW copy of
  `rootfs.ext4`~~ `[REVISED v0.9]` the substrate's (initrd + per-guest overlay,
  ADR-001 v2 §13.7) — retired as Barista's problem with the CONVERT stage.
- **In-guest agent** ~~injected at CONVERT, invisible to the developer~~
  `[REVISED v0.9]` delivered as a content-addressed volume at create (nap-005
  task 2.0), invisible to the developer; in fake mode the same binary wraps the
  container entrypoint — identical exec/files API across runtimes.
- **`start-cmd` + `ready-cmd` live in the Session manifest**, not the Dockerfile;
  golden snapshot is taken only after ready.
- **Multi-arch day 1**: aarch64 (Lima dev) + x86_64 (prod); arch is part of the
  template hash.

---

## 13. ADR-001 — Runtime selection: adopt a multi-hypervisor substrate; gVisor for the live-checkpoint tier

> Status: **Decided — ratified 2026-08-06** (amended v2; see §13.7). Ratified on
> the evidence in
> [`specs/../adr-001-substrate-evaluation.md`](adr-001-substrate-evaluation.md),
> **not** via T11, which now gates only the `runsc` tier. Ratified knowingly with
> one gap open: all restore-performance evidence is arm64/`vz`; the
> firecracker/UFFD path (spike task 3.4) is unmeasured.
>
> §13.1–13.6 below record the v1 reasoning as written. It is superseded where
> §13.7 says so, and vindicated where §13.7 says that. Sources verified against
> gvisor.dev (checkpoint/restore, platforms, performance, production) and Modal's
> docs + engineering blog.

### 13.1 Context

The BRD assumed Firecracker as *the* production runtime (§1, §2.1), with `fake`
(Docker) for macOS development and `kata`/`runsc` as a third implementation that
the voice-agent product's Azure/K8s deployment might one day force (old Risk 8, old OQ7). Three
expensive consequences followed from that assumption:

1. The Phase 1 contract is pitched at the Firecracker level of abstraction —
   kernel + `rootfs.ext4` + vsock (spec §2.2, Risk 3).
2. The differentiator is untestable in the dev runtime (Risk 1), because the only
   substitute for VM snapshots is CRIU.
3. K8s deployment — where two of three initial consumers live — is a degraded
   tier (old Risk 8).

Two findings invalidate the assumption's basis:

- **gVisor does real memory checkpoint/restore**, and its default `systrap`
  platform needs no virtualization extensions whatsoever.
- **Modal — the most mature memory-snapshot policy layer in the industry
  (§9.5) — runs gVisor, not Firecracker**, and chose it *for security* over
  `runc`+CRIU. Every pattern the BRD borrowed from Modal (B26–B30, B42, B43,
  worker-class keying B27) is therefore a *gVisor* pattern already proven at
  multi-tenant public-cloud scale.

### 13.2 Decision (recommended)

> `[SUPERSEDED v2 — §13.7]` The ranking below is retained for the record. The
> ratified ranking is in §13.7: `hypeman` is rank 1, `runsc` drops to rank 2 for
> live checkpoint and shared-kernel density, and `fake` stays rank 3.

**Do not swap — re-rank.** Build `runsc` as the first *real* runtime; keep
`firecracker` as a demand-driven hardware-isolation tier; retire `fake` to
tooling-only status.

| Rank | Runtime | Role | Justifies |
|---|---|---|---|
| 1 | `runsc` (gVisor) | Production + dev + CI. Memory snapshots, no KVM. | All three consumers (§11) on the infrastructure they already have |
| 2 | `firecracker` | Hardware-isolation tier, bare metal / KVM nodes | Untrusted multi-tenant workloads if §13.4 demands it |
| 3 | `fake` (Docker) | API/CP/tooling development only — never snapshot semantics | Shrinks to near-zero once (1) runs in Lima |

**The load-bearing corollary:** the Phase 1 contract must be pitched at the
**OCI-bundle** level, not the Firecracker level. This inverts spec §2.2 and is
the only part of this ADR that is expensive to defer — it is free today and
costs a Phase-4 migration later.

### 13.3 What this buys

| Gain | Evidence | Retires |
|---|---|---|
| No KVM / nested virt anywhere in production | `systrap` platform | Risk 8, OQ7 |
| Real memory snapshots on a Mac (plain Lima) and in ordinary CI | §4.1 revised | Risk 1, Risk 3 |
| OCI images as the native input — CONVERT stage (§12.1 step 2) mostly disappears | gVisor consumes OCI bundles directly | Most of Risk 4 |
| The voice-agent runtime becomes servable, not constraining | stock AKS node pools | §11.3 blocker |
| GPU workloads possible at all (nvproxy + CUDA C/R, driver 570+) | B43, §9.5 | the voice-agent product GPU tier |
| Proven policy layer transfers unchanged | Modal = gVisor (§9.5) | "unverified recipe" |

### 13.4 What this costs

- **Isolation drops from hardware to shared-kernel.** gVisor is a genuine
  boundary — Google runs Cloud Run, App Engine and Cloud Functions on it, Modal
  runs untrusted multi-tenant user code on it — but a sentry escape or a
  reachable host-kernel bug is not contained the way a KVM exit is. Note that
  Modal's tenants are separated by a cloud VM boundary *as well*, since they run
  on rented instances; gVisor is their intra-node boundary, not their only one.
  **Consequence:** isolation grade becomes a user-visible property (spec §5),
  and OQ2's multi-tenancy answer now has a runtime consequence.
- **Compatibility narrows.** gVisor implements a subset of the Linux syscall
  surface. This contradicts the §9.1 "any Linux workload" wedge against celld,
  and it bites hardest exactly where persona (b) lives: dev environments running
  `strace`, debuggers, `docker build`, systemd, exotic filesystems. **This is
  the single item that should gate ratification** — validate against a real coding session on
  the agent platform before flipping this ADR to Decided.
- **Performance tax on syscall-heavy, concurrent-VFS and network-bound work**;
  CPU-bound is near-native. The agent platform's sessions (process spawn, package installs,
  file I/O) sit in the taxed quadrant.
- **B38 and B39 do not transfer.** "Pause costs 30–100 ms" (`MAP_SHARED`
  continuous dirty-page flush) and "CoW fork in ~50 ms" are Firecracker
  mechanics. gVisor gives async prioritized page restore (`--background`), not
  continuous flush or block-level cloning. Golden-snapshot-plus-clone — the
  answer to the voice-agent product's warm pools — needs new engineering here. Modal's own
  bottleneck is the 100 MiB–10 GiB pages file (§9.5), so this is a real limit,
  not a tuning gap.
- `[VERIFY]` Restoring one checkpoint image **N times** (fork semantics, B39).
  gVisor docs are explicit that Docker cannot restore into a new container and
  that image paths must be unique, but quiet on the general `runsc restore`
  case. Modal restores a given snapshot repeatedly across requests, which
  suggests it works — confirm directly before designing fork-on-resume
  (spec §10, deferred to v1alpha2).

### 13.5 Consequences for the specs

1. **`TemplateRef` becomes a `oneof`** — OCI ref (`runsc`) vs `rootfs.ext4` +
   runtime bundle (`firecracker`). §12.2 template identity forks with it; for
   `runsc` the hash is (OCI digest + runsc version + resources + arch).
2. **`RuntimeCapabilities` gains `hardware_isolation`** — and per spec §5's own
   rule ("explicit downgrade, never silent"), isolation tier surfaces in the
   `Session` manifest the way `SnapshotKind` does (B32).
3. **Contract C is no longer vsock-based** for `runsc`. An in-sandbox agent is
   still required (hooks, entropy reseed, readiness, TTL activity), but the
   transport is a unix socket; gVisor additionally offers
   application-driven checkpointing via `/proc/gvisor/checkpoint`, which
   Firecracker has no equivalent of.
4. **Bundle pinning (B35) survives, renamed**: a gVisor snapshot is restorable
   only by a matching **runsc version** — Modal reports snapshots are sensitive
   to container-runtime *and* NVIDIA driver version.
5. **CPU-class keying (B13/B27) survives unchanged** — gVisor verifies the
   target host has every CPU feature present at checkpoint time; Modal's
   `pclmulqdq` example is the same failure mode the spec already models as
   `CPU_CLASS_MISMATCH`.
6. **Entropy reseed (T9) remains a genuine differentiator** — Modal documents
   RNG duplication as the user's problem, verbatim (§9.5).
7. **Add cold-boot fallback on restore failure (B42)** to the `Resume` error
   path — snapshots are an optimization, never a correctness dependency.
8. **Acceptance tests T2/T3/T7/T8/T9 stop requiring a bare-metal box** and run
   on any Linux CI runner, which is what makes the Phase 1 DoD achievable.


### 13.6 Alternatives considered

| Option | Verdict |
|---|---|
| Firecracker only (status quo ante) | Rejected as *first* runtime: blocks two of three consumers, keeps the differentiator untestable in dev, and rests on a premise (§10) now refuted. Retained as tier 2. |
| gVisor only — drop Firecracker | Rejected: forecloses the hardware-isolation tier before OQ2 (multi-tenancy) is answered, for no near-term gain. FR-11 exists precisely so this stays open. |
| Kata Containers | Not re-examined here; it was only ever a proxy for "K8s-compatible isolation", a need `runsc` now meets without the KVM requirement. |
| gVisor *inside* Firecracker (defence in depth) | The standard answer at the highest tenancy bar. Deferred — it is a composition of tiers 1 and 2, not a fourth runtime. |

### 13.7 `[AMENDED v2 — ratified 2026-08-06]` Adopt the substrate; do not build it

**What changed.** `hypeman` (§9.10) removes *both* reasons Firecracker was
ranked 2: it needs no KVM on Apple Silicon, and it runs any OCI image as a VM via
initrd + per-guest overlay, so no rootfs CONVERT pipeline is required. It also
supplies hardware isolation — which `runsc` structurally cannot — and `fork`.
The hardware-isolation tier therefore stops being a demand-driven future cost and
becomes available now, on a laptop, by **adopting** rather than building.

**Ratified ranking:**

| Rank | Runtime | Role | Basis |
|---|---|---|---|
| 1 | **`hypeman`** (firecracker · cloud-hypervisor · qemu · vz) | Production + dev + CI. `Pause`/`Resume` with exact memory, hardware isolation, fork. **Adopted, not built.** | Gates 1.1–1.4 pass; T7 semantics verified on macOS; realistic 1.5 GiB session resumes in ~2 s; idle cost free in compute |
| 2 | **`runsc`** (gVisor) | The two things hypeman cannot do: **live checkpoint** (`Checkpoint`, T2, B31) and a **shared-kernel density tier** | No live checkpoint in hypeman — snapshot-from-running is standby→copy→restore (evaluation §2.1) |
| 3 | `fake` (Docker) | API/CP/tooling development only — never snapshot semantics | unchanged |

**Vindicated, not superseded:** §13.2's load-bearing corollary — pitch the Phase 1
contract at the **OCI-bundle** level rather than the Firecracker level — is
exactly what makes this amendment cheap. hypeman consumes OCI images natively, so
the abstraction chosen for the wrong runtime turned out to be the right one.

**Retired as Barista's problems:** the CONVERT pipeline (§12.1) and spec §10 OQ5
(writable-layer CoW) — hypeman does initrd + per-guest overlay for any OCI image.
Spec §10 OQ4 (N-times restore) is answered yes: `fork` works, so the v1alpha2
deferral of fork may be unnecessary.

**T11 is demoted.** It was the ratification gate for this ADR. The ADR is now
ratified on substrate evidence, so T11 gates only whether the `runsc` tier is
viable for agent sessions — a rank-2 question, not a foundational one.

**What stays Barista's, regardless of substrate:** Contract A and C, the journaled
operations model, session identity and policy, `cpu_class` (needed only for the
cross-host remote tier, R-SNAP-2), the guest-agent component of
`runtime_bundle_ref`, the machine-readable reasons of spec §8, and the cold-boot
fallback (B42). hypeman keys the image by digest and pins hypervisor/kernel
version per instance; the rest is ours (evaluation §2.3).

**Known cost accepted at ratification:** an OpenAPI 3.1 → Rust client (no Rust
SDK upstream), a second daemon per node, and reconciling two records of one
instance. The daemon is control plane only — `SIGKILL` on it leaves sessions
running and it re-adopts them on restart (evaluation §5) — so it is not a
single point of failure for running sessions.
