# Change: nap-017-fleet-coordination

> Ratified 2026-08-08 (constitution V) — implementation may start.
>
> Task 1.1 (one real cloud backend) remains its own merge gate and is unaffected
> by this ratification: it needs credentials the environment does not have, and
> today's nap-014 finding is the argument for keeping it a gate rather than
> trusting the documented rows — a substrate that documents a feature, validates
> its request, and implements none of it is no longer hypothetical here.

## Why

ADR-002 is ratified: Phase 2's coordination is bucket CAS leases with ETag
fencing — no Control Plane service, no scheduler service. The spike proved the
mechanism (fencing holds under ±3 s clock skew; the wake path fits the latency
allowance same-region; the critical protocol is ~150 lines); what exists is
throwaway code in `work/` and a ratified `fleet-coordination` spec with four
requirements. This change is the promotion: the protocol becomes a crate, the
Node Agent becomes a fleet member, and BRD §4 row 2's DoD becomes a passing
test — *two nodes, one bucket, a contended name: exactly-one-owner holds under
kill -9, and a session resumes on its owning node*.

The premises it implements are v0.10's: the session **name** is the public
handle (the lease object *is* the addressing table), and a single node keeps
working with **no bucket at all** (laptop mode is a ratified requirement, not
a fallback).

## What Changes

- **`crates/nap-fleet`**: the lease protocol from the spike, hardened —
  acquire/renew/release with epoch + ETag fencing over `object_store`
  (S3/R2/MinIO/Azure behind one API), typed conflicts, jittered retries. The
  gateway (Phase 5) will consume this same crate for resolution; that seam is
  the reason it is a crate and not a module.
- **Desired state lives in the bucket**: `desired/<name>` carries the
  serialized `InstanceSpec` plus fleet policy (schema-first: the proto is the
  contract; the object wraps it). Writing it is what "creating a session
  fleet-wide" means — the north-star's first half.
- **The Node Agent pulls**: the reconciler tick, when a bucket is configured,
  renews owned leases, scans for unowned desired sessions, acquires with fit
  (B16 minimal placement: capacity check, nothing cleverer), and materialises
  what it acquires (create + start from the desired spec).
- **Self-fencing on lease loss** — the split-brain answer: a node whose
  renewal is superseded (epoch advanced past it) stops treating the session as
  its own — it stops the local instance (disk kept, snapshot kept), events
  why, and does not reacquire until it can win the lease honestly. The
  substrate-level fence (stale ETag) already made its writes to the record
  harmless; this makes the *workload* single too.
- **Locality is lease retention** (B45 under pull): the owner keeps renewing
  while its session is `PAUSED` — a node-local pause pins the resume by
  construction. Only a *lapsed* lease (node death) frees the name, and the
  acquiring node cold-boots from the desired spec with a loud degradation
  event (B42's semantics, fleet-level).
- **CLI**: `nap fleet apply <name> --spec …` (write desired state),
  `nap fleet ls` (prefix listing — the inventory ADR-002 measured at 12 ms),
  `nap fleet resolve <name>` (name → owner, the gateway's future first hop).
- **First task, inherited from nap-012 §3.4**: the spike's matrix binary
  against one real cloud backend before any of the above merges.

## Capabilities

### Modified Capabilities
- `fleet-coordination`: the ratified obligations gain their concrete forms —
  desired-state objects, acquisition-materialises, self-fencing, locality by
  lease retention.
- `node-agent-api`: `GetNodeInfo` reports fleet membership (bucket configured,
  leases held) — additive.

## Impact

- New crate `crates/nap-fleet` (object_store dependency enters the workspace).
- `crates/nap-node-agent`: config (optional bucket URL + credential chain),
  `reconcile` (the pull/renew/fence loop rides the existing tick), `service`
  (node info), events (acquisition, fencing, cold-boot-on-takeover).
- `crates/nap-cli`: the three `fleet` verbs.
- **No Contract A break**: `instance_id` stays node-scoped; the name→instance
  binding lives in the lease record. Laptop mode: no bucket configured means
  none of this code runs — Phase 1 behaviour bit-for-bit.
- Depends on: ADR-002 (ratified), cloud credentials for task 1.1 (human
  provides). Independent of nap-011/013/014/015/016.

## Constitution Check

- **Adopt the substrate, own the session layer**: coordination *is* session
  layer — the one place ADR-002 accepted owning distributed-systems code, and
  sized it first (~150 critical lines, measured).
- **Crash-safe by construction**: every fleet action lands in the same op
  journal; the kill -9 acceptance test is the row-2 DoD, not an afterthought.
- **Honest capabilities**: takeover cold boots are degradation-evented; an
  unreachable bucket refuses acquisitions explicitly and never touches what
  runs (the ratified outage requirement).
- **Simple by default**: placement is "first node with fit"; no read-model
  service, no event log in the bucket (fan-out v1 per ADR-002 §3.6); desired
  state is one object per session.

## Acceptance

BRD §4 row 2's DoD, as an integration test against MinIO + two node agents:
a contended name yields exactly one owner; `kill -9` of the owner frees the
name after lease expiry; the survivor acquires, cold-boots, and events the
takeover; the paused-session locality rule keeps a pause pinned to its owner.
Plus: the cloud matrix row (task 1.1) recorded in ADR-002's table, and
`make check` green with laptop mode provably unchanged (the existing suite
*is* that proof — it runs bucketless).
