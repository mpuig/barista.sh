# ADR-002 — Fleet coordination: bucket CAS leases instead of a Control Plane

> Status: **ratified 2026-08-08** (constitution V — human ratification, same
> session that reviewed the evidence). Effects executed: BRD §4 rows 2–3
> rewritten, OQ9 closed, the north-star milestone restated; implementation is
> `nap-017`'s to propose, inheriting §5(3)'s cloud-backend check as its first
> task.
> Evidence gathered 2026-08-08 by `nap-012-bucket-coordination-spike`; probe
> code in `work/bucket-spike/` (gitignored, like nap-004's), reproducible with
> a local MinIO (`docker run … minio/minio`) and `cargo run`.

## 1. The question

BRD OQ9, sharpened by §1's deployment tiers and §9.12's premises: should
Phase 2's coordination be **object-store CAS leases with epoch fencing**
(celld's B1) rather than the roadmap's Minimal Control Plane + scheduler?
Framed by the one asymmetry that matters: the object-store bucket is *already*
a Phase 2 dependency (the remote snapshot tier), so the choice is
"CP + bucket" versus "bucket".

## 2. The mechanism measured

One object per session, `sessions/<name>` → `{owner, epoch, expires, instance}`.
Acquire/renew is read + conditional write; fencing is the object's ETag — a
stale owner's ETag is stale by construction, so **no clock has to be right**.
The session **name** is the key, which makes the coordination table and the
addressing table the same object (BRD v0.10 premise, demonstrated in §3.3).

## 3. Evidence

### 3.1 Conditional-write matrix (task 2.1)

| backend | create-if-absent | update-if-match (fence) | verdict |
|---|---|---|---|
| MinIO (local, RELEASE-2025) | ✓ clean `AlreadyExists` on conflict | ✓ clean `Precondition` on stale ETag | **measured, works** |
| AWS S3 | documented (`If-None-Match` PUT, 2024-08) | documented (`If-Match` PUT, 2024-11) | **documented, unmeasured — no credentials in this environment** |
| Cloudflare R2 | ✓ clean `AlreadyExists` on conflict | ✓ clean `Precondition` on stale ETag | **measured 2026-08-08, works** |
| Azure Blob | native (`If-None-Match: *`) | native ETag `If-Match`, plus native leases | **documented, unmeasured — no credentials** |

The named failure for the two remaining cloud rows is credentials, not
mechanism: both document the exact primitives the protocol uses, and the
spike's trait boundary (`object_store`) already speaks to all of them.

**`[CLOSED 2026-08-08 — the R2 row is measured]`** nap-017's task 1.1 ran this
spike against a real Cloudflare R2 bucket. R2 honours both primitives cleanly —
`AlreadyExists` on a create conflict, `Precondition` on a stale ETag — and the
fencing property held there exactly as it did on MinIO: 14 acquisitions across
6 epochs under ±3 s of clock skew, **zero epochs with two owners, zero stale
writes accepted**, and 500 contended attempts producing 51 ownerships, 449
clean conflicts and 0 errors with monotonic epochs.

That is the gate the change shipped with open, and it is now shut: the
single-writer guarantee rests on measured behaviour of a second, independent
implementation rather than on one vendor's documentation. AWS S3 and Azure Blob
remain documented-and-unmeasured; the protocol is the same three calls, so the
risk they carry is now "another vendor's S3 API", not "the design".

### 3.2 Fencing property (task 1.2)

8 concurrent nodes, clocks lying by −3 s…+3 s, lease TTL 400 ms, 5-second
runs, four runs: **~180 successful acquisitions per run, zero epochs with two
owners, zero stale fenced writes accepted.** The property held on every run.
The reason it cannot break: expiry only decides *when a node tries*; whether
it wins is the backend's serialized CAS, and a superseded owner's ETag is
already stale. Clock skew changes contention, never safety.

### 3.3 Latency against the wake budget (tasks 2.2, 2.3)

Localhost MinIO — the **protocol floor**; WAN adds RTT per operation:

| op | p50 | p99 |
|---|---|---|
| acquire (read + create) | 1.6 ms | 3.8 ms |
| renew (read + CAS) | 2.2 ms | 5.7 ms |
| resolve + TCP dial to owner | 0.6 ms | 0.8 ms |

The wake path costs at most one resolve + one CAS ≈ 2 round trips. Against
the ~100 ms allowance (NFR-1 p50 budget minus the measured ~370 ms restore):
same-region object storage (5–30 ms RTT) lands at 10–60 ms — **inside the
allowance with room**. Cross-region would eat it; the deployment rule is
"nodes and bucket share a region", which tier A/C consumers satisfy trivially.
Renewals are heartbeat-cadence and never on the wake path.

#### `[MEASURED 2026-08-08]` The same operations against real R2, from a laptop

| op | p50 | p99 | localhost MinIO, for scale |
|---|---|---|---|
| acquire (read + create) | **301.5 ms** | 416.9 ms | 1.6 ms |
| renew (read + CAS) | 361.4 ms | 523.7 ms | 2.2 ms |
| resolve + TCP dial to owner | 87.8 ms | 171.9 ms | 0.6 ms |
| list `sessions/` at 503 keys | **372.9 ms** | — | 12 ms |

**Read this as the ceiling, not the floor.** It is a laptop in Europe against
R2's public endpoint — precisely the deployment the rule above excludes — so it
does not contradict the 10–60 ms same-region estimate. It measures the case
nobody should run.

What it does do is turn that rule from advice into load-bearing structure. A
wake path of ~390 ms p50 (resolve + CAS) is **four times the whole allowance and
longer than the restore it precedes**: a node placed away from its bucket spends
more time asking who owns the session than restoring its memory. "Nodes and
bucket share a region" is therefore a constraint the deployment must satisfy,
not a preference — and the 10–60 ms figure it rests on is still an estimate,
because no node has yet been run beside its bucket.

Second, quieter finding: **the inventory listing moved from 12 ms to 373 ms for
the same 503 keys.** §3.5 concludes a prefix list *is* the inventory query and a
read model is a when-it-hurts optimisation. Over a WAN it starts hurting sooner
than that reading suggests.

### 3.4 Contention (task 2.4)

10 nodes × 50 attempts on one name, 50 ms TTL: 500 attempts → 32 ownerships,
468 **clean** conflicts, 0 errors, final epoch 3 and monotonic. Losers learn
they lost from a typed conflict, not an error to retry blindly.

### 3.5 Inventory at fleet size (task 2.5)

Listing `sessions/` at 503 keys: **12 ms** (localhost). At the three internal
consumers' scale (NFR-4 has no targets yet), a prefix list *is* the inventory
query. A read-model service is a when-it-hurts optimization, not a day-one
component.

### 3.6 Cross-fleet events (task 3.1 — the gap with no borrowed answer)

Options, priced:

1. **Per-node `WatchEvents` fan-out** (recommended v1): consumers are three
   and internal; the CLI already speaks per-node Contract A; a fleet view is a
   client-side merge over the owner set the bucket already knows. Zero new
   infrastructure, zero new consistency questions.
2. **Bucket-append event log**: durable and CP-free but pays an object write
   per event (or batching latency) and invents a compaction problem — cost
   without a consumer asking for it.
3. **Read-model service**: subsumes inventory too, but it is exactly the
   optional, rebuildable-from-the-bucket CP remnant — build when fan-out
   measurably hurts.

Recommendation: option 1 for v1, with the bucket's owner list as the fan-out
directory.

### 3.7 Single-node degenerate case (task 3.2)

Phase 1 has no bucket dependency anywhere in `crates/` — a lone node's journal
is already its truth, and the delta spec's "laptop mode" requirement encodes
that coordination exists only where a second node could contend. Confirmed by
construction; nothing to measure.

### 3.8 The owned-code ledger (task 3.3)

The protocol Barista would own measures **150 lines** (lease + fencing, the part
that must be correct) — call it 400–600 with retry/jitter, a heartbeat task,
and configuration, plus the `object_store` dependency speaking to every
backend §1 implies. The alternative it replaces is roadmap rows 2–3 whole: a
Control Plane service (registration, orders, inventory, its own store, its own
deployment) and a scheduler — plus the fact, fatal on its own, that **tiers A
and C have nowhere to run them**. This is the inverse of the ADR-001 ledger
(~35,600 lines to reimplement a substrate): here the *bucket* path is the one
that owns hundreds of lines instead of thousands.

## 4. What defaulting to the bucket costs (the OQ9 question, answered)

- **Placement quality**: nothing Barista planned to have — B16 mandates minimal
  placement, B20 a scalar load signal, and B45's locality pin is *stronger*
  under pull (only the node holding the local snapshot can resume cheaply).
- **Inventory**: a 12 ms prefix list at 500 sessions; read-model later if it
  hurts.
- **Cross-fleet events**: fan-out v1 (§3.6) — the one real design debt, taken
  knowingly.
- **A familiar architecture**: no operational answer needed on tiers A/C,
  which is the point.

## 5. Recommendation

**Adopt bucket CAS leases with ETag fencing as Phase 2's coordination layer.**
Concretely:

1. Rewrite BRD §4 rows 2–3: Phase 2 = the coordination layer (leases, name
   resolution, node pull loop) + fleet inventory as prefix listing; Phase 3 =
   placement polish (B16/B20/B45) and reconciliation across nodes — no
   Control Plane service, no scheduler service. The north-star milestone
   becomes: *a manifest written to the bucket materialises on some node*.
2. The `fleet-coordination` delta spec (nap-012) carries the obligations;
   implementation is a new change (nap-017?) that also decides the manifest
   object schema.
3. First Phase 2 task inherits the unmeasured rows: run this spike's binary
   against one real cloud backend (R2 or S3) before the protocol code is
   promoted out of `work/`.
4. Events: per-node fan-out v1 (§3.6); revisit with a consumer in hand.

**Stop: this ADR takes effect only on human ratification**, which also
authorises the roadmap rewrite in (1).
